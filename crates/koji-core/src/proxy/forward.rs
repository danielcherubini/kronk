use crate::proxy::{ModelState, ProxyState};
use axum::{
    body::Body,
    http::{request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures_util::stream::StreamExt;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::info;

/// Hop-by-hop headers that should be stripped from forwarded requests.
const REQUEST_SKIP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "transfer-encoding",
    "upgrade",
    "trailer",
    "host",
];

/// Hop-by-hop headers (plus content-length) that should be stripped from forwarded responses.
const RESPONSE_SKIP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "transfer-encoding",
    "upgrade",
    "trailer",
    "content-length",
];

/// Filter request headers, removing hop-by-hop headers.
pub fn filter_request_headers(headers: &HeaderMap) -> HeaderMap {
    let mut filtered = HeaderMap::new();
    for (key, value) in headers {
        if !REQUEST_SKIP_HEADERS.contains(&key.as_str()) && value.to_str().is_ok() {
            filtered.insert(key.clone(), value.clone());
        }
    }
    filtered
}

/// Strip hop-by-hop and content-length headers from a response.
pub fn strip_response_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for (key, value) in headers {
        if RESPONSE_SKIP_HEADERS.contains(&key.as_str()) {
            continue;
        }
        if let Ok(v) = value.to_str() {
            result.push((key.as_str().to_string(), v.to_string()));
        }
    }
    result
}

/// Rewrite the `model` field in a JSON value. Only rewrites if model_name is provided and non-empty.
pub fn rewrite_json_model_name(mut json: JsonValue, model_name: Option<&str>) -> JsonValue {
    if let Some(name) = model_name {
        if !name.is_empty() {
            json["model"] = JsonValue::String(name.to_string());
        }
    }
    json
}

/// Build a forward request target URI from the backend URL and request path/query.
#[allow(dead_code)]
pub fn build_forward_uri(backend_url: &str, parts: &Parts) -> Option<String> {
    let path_and_query = parts.uri.path_and_query()?;
    let (path, query) = path_and_query
        .as_str()
        .split_once('?')
        .unwrap_or((path_and_query.as_str(), ""));

    let mut uri = format!("{}{}", backend_url, path);
    if !query.is_empty() {
        uri.push('?');
        uri.push_str(query);
    }
    Some(uri)
}

/// Process a complete SSE line, rewriting the `model` field in JSON data lines.
fn process_sse_line(line: &str, model_name: Option<&str>, out: &mut String) {
    if let Some(data_content) = line.strip_prefix("data: ") {
        let trimmed = data_content.trim_end();
        if trimmed == "[DONE]" {
            out.push_str(line);
        } else if let Ok(mut json_value) = serde_json::from_str::<JsonValue>(trimmed) {
            if let Some(name) = model_name {
                if !name.is_empty() {
                    json_value["model"] = JsonValue::String(name.to_string());
                }
            }
            out.push_str("data: ");
            out.push_str(
                &serde_json::to_string(&json_value).unwrap_or_else(|_| trimmed.to_string()),
            );
            out.push('\n');
        } else {
            out.push_str(line);
        }
    } else {
        // Comments, empty lines, and other lines pass through unchanged
        out.push_str(line);
    }
}

pub async fn forward_request(
    state: &Arc<ProxyState>,
    server_name: &str,
    parts: &Parts,
    body_bytes: &[u8],
    model_name: Option<&str>,
) -> Response {
    state
        .metrics
        .total_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let model_state = state.get_model_state(server_name).await;
    if let Some(ms) = &model_state {
        // If the backend process has died, clean up immediately and let the
        // caller's auto-load logic restart it. Skip the circuit breaker
        // entirely — it is meant for live backends returning errors, not
        // crashed processes.
        let process_dead = ms
            .backend_pid()
            .map(|pid| !super::process::is_process_alive(pid))
            .unwrap_or(false);
        if process_dead {
            info!(
                "Backend process for server '{}' is dead (detected at request entry), cleaning up",
                server_name
            );
            let mut models = state.models.write().await;
            models.remove(server_name);
            if let Some(conn) = state.open_db() {
                let _ = crate::db::queries::remove_active_model(&conn, server_name);
            }
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Backend process for server '{}' has crashed, reloading", server_name),
                        "type": "BackendCrashedError"
                    }
                })),
            )
                .into_response();
        }

        let failures = ms
            .consecutive_failures()
            .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0);
        let config = state.config.read().await;
        if failures >= config.proxy.circuit_breaker_threshold {
            // Check if cooldown has elapsed
            if !ms.can_reload(config.proxy.circuit_breaker_cooldown_seconds) {
                info!(
                    "Circuit breaker cooldown active for server '{}' ({} failures). Waiting for cooldown.",
                    server_name, failures
                );
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Server {} is in cooldown due to repeated failures", server_name),
                            "type": "ServiceUnavailableError"
                        }
                    })),
                )
                    .into_response();
            }
            info!(
                "Circuit breaker tripped for server '{}' ({} failures). Unloading server.",
                server_name, failures
            );
            // Unload the server using PID from backend_pid
            if let Some(_pid) = ms.backend_pid() {
                let _ = state.unload_model(server_name).await;
            }
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Server {} is currently unavailable due to repeated failures", server_name),
                        "type": "ServiceUnavailableError"
                    }
                })),
            )
                .into_response();
        }
    }

    let backend_url = {
        let models = state.models.read().await;
        match models.get(server_name).and_then(|ms| ms.backend_url()) {
            Some(url) => url.to_string(),
            None => {
                info!("No backend URL for model '{}' (not loaded?)", server_name);
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Model '{}' is not loaded", server_name),
                            "type": "BackendUrlError"
                        }
                    })),
                )
                    .into_response();
            }
        }
    };

    // Combine backend_url with the request path and query
    let path_and_query = match parts.uri.path_and_query() {
        Some(pq) => pq,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": {
                        "message": "Invalid request URI",
                        "type": "BadRequestError"
                    }
                })),
            )
                .into_response();
        }
    };

    let (path, query) = path_and_query
        .as_str()
        .split_once('?')
        .unwrap_or((path_and_query.as_str(), ""));

    let target_uri = format!("{}{}", backend_url, path);

    info!("Forwarding request to: {}", target_uri);

    let method = parts.method.clone();

    let headers = filter_request_headers(&parts.headers);

    let mut query_string = query.to_string();
    if !query_string.is_empty() {
        query_string = format!("?{}", query_string);
    }

    match state
        .client
        .request(method, format!("{}{}", target_uri, query_string))
        .headers(headers)
        .body(body_bytes.to_vec())
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                state
                    .metrics
                    .successful_requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if let Some(ms) = &model_state {
                    if let Some(f) = ms.consecutive_failures() {
                        f.store(0, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            } else {
                state
                    .metrics
                    .failed_requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if status.is_server_error() {
                    if let Some(ms) = &model_state {
                        if let Some(f) = ms.consecutive_failures() {
                            f.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                        // Set failure timestamp for cooldown
                        if ms.is_ready() || matches!(ms, ModelState::Starting { .. }) {
                            let new_ts = SystemTime::now();
                            let mut models = state.models.write().await;
                            #[allow(clippy::collapsible_match)]
                            if let Some(existing) = models.get_mut(server_name) {
                                match existing {
                                    ModelState::Ready {
                                        failure_timestamp, ..
                                    }
                                    | ModelState::Starting {
                                        failure_timestamp, ..
                                    } => {
                                        *failure_timestamp = Some(new_ts);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
            }

            let mut builder = Response::builder().status(status);

            for (key, value) in strip_response_headers(response.headers()) {
                builder = builder.header(&key, value);
            }

            // Check if this is a streaming response
            let is_streaming = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("text/event-stream"))
                .unwrap_or(false);

            let body = if is_streaming {
                // Streaming response — rewrite the model name in each SSE chunk.
                // Uses unfold to own the partial-line buffer across chunks (Send-safe).
                let model_name: Option<String> = model_name.map(|s| s.to_string());
                let byte_stream = response.bytes_stream();
                let transformed_stream = futures_util::stream::unfold(
                    (byte_stream, String::new()),
                    move |(mut stream, mut line_buf)| {
                        let model_name = model_name.clone();
                        async move {
                            let chunk_result = stream.next().await?;
                            let result: Result<Bytes, reqwest::Error> = match chunk_result {
                                Ok(chunk) => {
                                    let chunk_str = String::from_utf8_lossy(&chunk);
                                    let mut out = String::new();

                                    for ch in chunk_str.chars() {
                                        line_buf.push(ch);
                                        if ch == '\n' {
                                            let line = line_buf.clone();
                                            line_buf.clear();
                                            process_sse_line(
                                                &line,
                                                model_name.as_deref(),
                                                &mut out,
                                            );
                                        }
                                    }

                                    Ok(Bytes::from(out.into_bytes()))
                                }
                                Err(e) => Err(e),
                            };
                            Some((result, (stream, line_buf)))
                        }
                    },
                );
                Body::from_stream(transformed_stream)
            } else {
                // Non-streaming response - parse, rewrite, and re-serialize
                let body_bytes = response.bytes().await.unwrap_or_default();
                // Only attempt JSON rewrite if content is valid JSON
                let new_body = if let Ok(parsed) = serde_json::from_slice::<JsonValue>(&body_bytes)
                {
                    let rewritten = rewrite_json_model_name(parsed, model_name);
                    serde_json::to_vec(&rewritten).unwrap_or(body_bytes.to_vec())
                } else {
                    // Not JSON, pass through unchanged
                    body_bytes.to_vec()
                };
                Body::from(new_body)
            };

            match builder.body(body) {
                Ok(resp) => resp.into_response(),
                Err(e) => {
                    tracing::error!("Failed to build response body: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": {
                                "message": "Internal error building response",
                                "type": "InternalError"
                            }
                        })),
                    )
                        .into_response()
                }
            }
        }
        Err(e) => {
            state
                .metrics
                .failed_requests
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Check if the backend process is still alive. If it crashed,
            // clean up immediately instead of letting the circuit breaker
            // accumulate failures and impose a cooldown. The next request
            // will trigger a fresh auto-load.
            let process_dead = model_state
                .as_ref()
                .and_then(|ms| ms.backend_pid())
                .map(|pid| !super::process::is_process_alive(pid))
                .unwrap_or(false);

            if process_dead {
                info!(
                    "Backend process for server '{}' is dead, cleaning up model state",
                    server_name
                );
                let mut models = state.models.write().await;
                models.remove(server_name);
                // Best-effort DB cleanup
                if let Some(conn) = state.open_db() {
                    let _ = crate::db::queries::remove_active_model(&conn, server_name);
                }
            } else {
                // Process is alive — this is a transient error (timeout, busy, etc.)
                // Increment the circuit breaker counter.
                if let Some(ms) = &model_state {
                    if let Some(f) = ms.consecutive_failures() {
                        f.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }

            info!("Failed to forward request: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Backend error: {}", e),
                        "type": "BadGatewayError"
                    }
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{header::HeaderName, request::Parts, HeaderMap, HeaderValue};

    // ── filter_request_headers tests ──────────────────────────────────────

    #[test]
    fn test_filter_request_headers_strips_dangerous_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("host", HeaderValue::from_static("localhost:8080"));
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("upgrade", HeaderValue::from_static("websocket"));
        headers.insert("proxy-authenticate", HeaderValue::from_static("Basic"));
        headers.insert(
            "proxy-authorization",
            HeaderValue::from_static("Bearer token"),
        );
        headers.insert("te", HeaderValue::from_static("trailers"));
        headers.insert("trailer", HeaderValue::from_static("X-Signature"));

        let filtered = filter_request_headers(&headers);

        assert!(!filtered.contains_key("host"));
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("keep-alive"));
        assert!(!filtered.contains_key("transfer-encoding"));
        assert!(!filtered.contains_key("upgrade"));
        assert!(!filtered.contains_key("proxy-authenticate"));
        assert!(!filtered.contains_key("proxy-authorization"));
        assert!(!filtered.contains_key("te"));
        assert!(!filtered.contains_key("trailer"));
    }

    #[test]
    fn test_filter_request_headers_passes_safe_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", HeaderValue::from_static("Mozilla/5.0"));
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        headers.insert("accept", HeaderValue::from_static("text/event-stream"));

        let filtered = filter_request_headers(&headers);

        assert_eq!(filtered.get("user-agent").unwrap(), "Mozilla/5.0");
        assert_eq!(filtered.get("content-type").unwrap(), "application/json");
        assert_eq!(filtered.get("authorization").unwrap(), "Bearer secret");
        assert_eq!(filtered.get("accept").unwrap(), "text/event-stream");
    }

    #[test]
    fn test_filter_request_headers_skips_invalid_utf8() {
        let mut headers = HeaderMap::new();
        // Insert a header with invalid UTF-8 value — should be skipped
        headers.insert(
            HeaderName::from_static("x-custom"),
            HeaderValue::from_bytes(b"\xff\xfe").unwrap(),
        );
        headers.insert("content-type", HeaderValue::from_static("text/plain"));

        let filtered = filter_request_headers(&headers);

        // Invalid UTF-8 header should be skipped, valid one should pass
        assert!(!filtered.contains_key("x-custom"));
        assert_eq!(filtered.get("content-type").unwrap(), "text/plain");
    }

    #[test]
    fn test_filter_request_headers_empty_input() {
        let headers = HeaderMap::new();
        let filtered = filter_request_headers(&headers);
        assert!(filtered.is_empty());
    }

    // ── strip_response_headers tests ──────────────────────────────────────

    #[test]
    fn test_strip_response_headers_removes_hop_by_hop() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("content-length", HeaderValue::from_static("1234"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("x-custom", HeaderValue::from_static("value"));

        let stripped = strip_response_headers(&headers);

        let keys: Vec<&str> = stripped.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"connection"));
        assert!(!keys.contains(&"content-length"));
        assert!(!keys.contains(&"transfer-encoding"));
        assert!(keys.contains(&"x-custom"));
        assert_eq!(
            stripped.iter().find(|(k, _)| k == "x-custom").unwrap().1,
            "value"
        );
    }

    #[test]
    fn test_strip_response_headers_passes_safe_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        headers.insert("x-request-id", HeaderValue::from_static("abc123"));

        let stripped = strip_response_headers(&headers);

        assert_eq!(stripped.len(), 2);
        assert!(stripped
            .iter()
            .any(|(k, v)| k == "content-type" && v == "application/json"));
        assert!(stripped
            .iter()
            .any(|(k, v)| k == "x-request-id" && v == "abc123"));
    }

    #[test]
    fn test_strip_response_headers_empty_input() {
        let headers = HeaderMap::new();
        let stripped = strip_response_headers(&headers);
        assert!(stripped.is_empty());
    }

    // ── rewrite_json_model_name tests ─────────────────────────────────────

    #[test]
    fn test_rewrite_json_model_name_replaces_existing() {
        let json = serde_json::json!({"model": "old-model", "choices": [{"message": {"content": "Hello"}}]});
        let result = rewrite_json_model_name(json, Some("new-model"));

        assert_eq!(result["model"], "new-model");
        assert_eq!(result["choices"][0]["message"]["content"], "Hello");
    }

    #[test]
    fn test_rewrite_json_model_name_adds_missing_field() {
        let json = serde_json::json!({"choices": [{"delta": {"content": "Hi"}}]});
        let result = rewrite_json_model_name(json, Some("my-model"));

        assert_eq!(result["model"], "my-model");
        assert!(result["choices"].is_array());
    }

    #[test]
    fn test_rewrite_json_model_name_preserves_other_fields() {
        let json = serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1234567890,
            "model": "old",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Test"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 20}
        });
        let result = rewrite_json_model_name(json, Some("new-model"));

        assert_eq!(result["model"], "new-model");
        assert_eq!(result["id"], "chatcmpl-123");
        assert_eq!(result["object"], "chat.completion");
        assert_eq!(result["created"], 1234567890);
        assert_eq!(result["usage"]["prompt_tokens"], 10);
    }

    #[test]
    fn test_rewrite_json_model_name_empty_string_ignored() {
        let json = serde_json::json!({"model": "old", "choices": []});
        let result = rewrite_json_model_name(json, Some(""));

        // Empty string should NOT rewrite the model field
        assert_eq!(result["model"], "old");
    }

    #[test]
    fn test_rewrite_json_model_name_none_skips_rewrite() {
        let json = serde_json::json!({"model": "old", "choices": []});
        let result = rewrite_json_model_name(json, None);

        // None should NOT rewrite the model field
        assert_eq!(result["model"], "old");
    }

    #[test]
    fn test_rewrite_json_model_name_long_model_name() {
        let json = serde_json::json!({"model": "m", "choices": []});
        let long_name = "unsloth/gemma-4-E2B-it-GGUF:q4_k_m";
        let result = rewrite_json_model_name(json, Some(long_name));

        assert_eq!(result["model"], long_name);
    }

    // ── build_forward_uri tests ───────────────────────────────────────────

    fn make_parts(path: &str) -> Parts {
        let req = axum::http::Request::get(path).body(()).unwrap();
        let (parts, _) = req.into_parts();
        parts
    }

    #[test]
    fn test_build_forward_uri_simple_path() {
        let parts = make_parts("http://localhost/v1/chat/completions");

        let uri = build_forward_uri("http://backend:8080", &parts).unwrap();
        assert_eq!(uri, "http://backend:8080/v1/chat/completions");
    }

    #[test]
    fn test_build_forward_uri_with_query_string() {
        let parts = make_parts("http://localhost/v1/models?limit=10");

        let uri = build_forward_uri("http://backend:8080", &parts).unwrap();
        assert_eq!(uri, "http://backend:8080/v1/models?limit=10");
    }

    #[test]
    fn test_build_forward_uri_no_query_returns_path_only() {
        let parts = make_parts("http://localhost/v1/completions");

        let uri = build_forward_uri("http://backend:8080", &parts).unwrap();
        assert_eq!(uri, "http://backend:8080/v1/completions");
    }

    // ── process_sse_line tests (existing but expanded) ────────────────────

    #[test]
    fn test_process_sse_line_rewrites_model_in_data() {
        let mut out = String::new();
        process_sse_line(
            "data: {\"model\": \"backend-model\", \"choices\": []}",
            Some("user-model"),
            &mut out,
        );
        // serde_json serializes without spaces by default
        assert!(out.contains("\"model\""), "output: {}", out);
        assert!(out.contains("user-model"), "output: {}", out);
    }

    #[test]
    fn test_process_sse_line_skips_rewrite_when_none() {
        let mut out = String::new();
        process_sse_line(
            "data: {\"model\": \"backend-model\", \"choices\": []}",
            None,
            &mut out,
        );
        // Model should NOT be rewritten when model_name is None
        assert!(out.contains("backend-model"), "output: {}", out);
        assert!(!out.contains("user-model"), "output: {}", out);
    }

    #[test]
    fn test_process_sse_line_passes_done_unchanged() {
        let mut out = String::new();
        process_sse_line("data: [DONE]", Some("any-model"), &mut out);
        // DONE is pushed as-is (no trailing newline added by this function)
        assert_eq!(out, "data: [DONE]");
    }

    #[test]
    fn test_process_sse_line_passes_comment_unchanged() {
        let mut out = String::new();
        process_sse_line(": heartbeat", Some("any-model"), &mut out);
        assert_eq!(out, ": heartbeat");
    }

    #[test]
    fn test_process_sse_line_passes_empty_line_unchanged() {
        let mut out = String::new();
        process_sse_line("", Some("any-model"), &mut out);
        assert_eq!(out, "");
    }

    #[test]
    fn test_process_sse_line_handles_invalid_json() {
        let mut out = String::new();
        process_sse_line("data: not valid json {", Some("any-model"), &mut out);
        assert_eq!(out, "data: not valid json {");
    }

    #[test]
    fn test_process_sse_line_handles_non_data_lines() {
        let mut out = String::new();
        process_sse_line("event: message", Some("any-model"), &mut out);
        assert_eq!(out, "event: message");
    }

    #[test]
    fn test_process_sse_line_multiline_buffer() {
        // A single call to process_sse_line processes one line at a time.
        // Lines without trailing newline are not processed as complete SSE lines.
        let mut out = String::new();
        // First line with newline - should be processed
        process_sse_line("data: {\"model\": \"a\"}\n", Some("user"), &mut out);
        assert!(out.contains("user"), "output: {}", out);
    }

    // ── Integration: header skip list consistency ─────────────────────────

    #[test]
    fn test_content_length_is_stripped_from_forwarded_response_headers() {
        let skip_list: &[&str] = &[
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "transfer-encoding",
            "upgrade",
            "trailer",
            "content-length",
        ];

        assert!(
            skip_list.contains(&"content-length"),
            "content-length MUST be stripped from forwarded response headers \
             because the proxy rewrites the JSON body, changing its size"
        );
    }

    #[test]
    fn test_body_size_changes_after_model_rewrite() {
        let short_model = "m.gguf";
        let long_model = "unsloth/gemma-4-E2B-it-GGUF";

        let original = serde_json::json!({
            "model": short_model,
            "choices": [{"message": {"role": "assistant", "content": "Hello"}}]
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let rewritten = rewrite_json_model_name(original, Some(long_model));
        let rewritten_bytes = serde_json::to_vec(&rewritten).unwrap();

        assert_ne!(
            original_bytes.len(),
            rewritten_bytes.len(),
            "Body size should differ after model name rewrite"
        );
        assert!(
            rewritten_bytes.len() > original_bytes.len(),
            "Rewritten body with longer model name should be larger"
        );
    }
}
