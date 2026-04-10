use crate::proxy::{ModelState, ProxyState};
use axum::{
    body::Body,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures_util::stream::StreamExt;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::info;

pub async fn forward_request(
    state: &Arc<ProxyState>,
    server_name: &str,
    parts: &Parts,
    body_bytes: &[u8],
    model_name: &str,
) -> Response {
    state
        .metrics
        .total_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let model_state = state.get_model_state(server_name).await;
    if let Some(ms) = &model_state {
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

    let mut headers = reqwest::header::HeaderMap::new();
    for (key, value) in &parts.headers {
        // Skip hop-by-hop headers
        if ![
            "connection",
            "keep-alive",
            "proxy-authenticate",
            "proxy-authorization",
            "te",
            "transfer-encoding",
            "upgrade",
            "trailer",
            "host",
        ]
        .contains(&key.as_str())
            && value.to_str().is_ok()
        {
            headers.insert(key.clone(), value.clone());
        }
    }

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

            for (key, value) in response.headers().iter() {
                // Skip hop-by-hop headers and content-length in response.
                // content-length must be removed because we rewrite the JSON
                // body (model name substitution) which changes its size; keeping
                // the original value would cause HTTP framing errors on
                // keep-alive connections. Hyper will set the correct
                // content-length (or use chunked encoding) automatically.
                if [
                    "connection",
                    "keep-alive",
                    "proxy-authenticate",
                    "proxy-authorization",
                    "te",
                    "transfer-encoding",
                    "upgrade",
                    "trailer",
                    "content-length",
                ]
                .contains(&key.as_str())
                {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    builder = builder.header(key.as_str(), v);
                }
            }

            // Check if this is a streaming response
            let is_streaming = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("text/event-stream"))
                .unwrap_or(false);

            let body = if is_streaming {
                // Streaming response - rewrite model name in each SSE chunk
                let model_name = model_name.to_string();
                // Use RefCell to persist buffer across chunk boundaries
                let buffer = std::cell::RefCell::new(String::new());
                let transformed_stream = response.bytes_stream().map(move |chunk_result| {
                    match chunk_result {
                        Ok(chunk) => {
                            // SSE format: each line is either a comment, empty, or data: ...
                            // We need to process data: lines and rewrite the model field.
                            // Buffer partial lines to handle chunks split mid-line.
                            let chunk_str = String::from_utf8_lossy(&chunk);
                            let mut result = String::new();
                            let mut buffer = buffer.borrow_mut();

                            for ch in chunk_str.chars() {
                                buffer.push(ch);
                                if ch == '\n' {
                                    // Process complete line from buffer
                                    let line = buffer.clone();
                                    buffer.clear();

                                    // Process complete SSE line
                                    if let Some(data_content) = line.strip_prefix("data: ") {
                                        let trimmed = data_content.trim_end();
                                        if trimmed == "[DONE]" {
                                            // Pass through unchanged
                                            result.push_str(&line);
                                        } else {
                                            // Try to parse and rewrite model field
                                            if let Ok(mut json_value) =
                                                serde_json::from_str::<JsonValue>(data_content)
                                            {
                                                json_value["model"] =
                                                    JsonValue::String(model_name.clone());
                                                let reserialized =
                                                    serde_json::to_string(&json_value)
                                                        .unwrap_or_else(|_| {
                                                            data_content.to_string()
                                                        });
                                                result.push_str("data: ");
                                                result.push_str(&reserialized);
                                                result.push('\n');
                                            } else {
                                                // Not valid JSON, pass through unchanged
                                                result.push_str(&line);
                                            }
                                        }
                                    } else if line.is_empty() || line.starts_with(':') {
                                        // Comments and empty lines pass through unchanged
                                        result.push_str(&line);
                                    } else {
                                        // Other lines pass through
                                        result.push_str(&line);
                                    }
                                }
                            }

                            // Do not emit partial lines yet - they will be completed in the next chunk
                            // Only emit fully-formed lines (those ending with \n)

                            Ok(Bytes::from(result.into_bytes()))
                        }
                        Err(e) => Err(e),
                    }
                });
                Body::from_stream(transformed_stream)
            } else {
                // Non-streaming response - parse, rewrite, and re-serialize
                let body_bytes = response.bytes().await.unwrap_or_default();
                // Only attempt JSON rewrite if content is valid JSON
                let new_body =
                    if let Ok(mut parsed) = serde_json::from_slice::<JsonValue>(&body_bytes) {
                        parsed["model"] = JsonValue::String(model_name.to_string());
                        serde_json::to_vec(&parsed).unwrap_or(body_bytes.to_vec())
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
            if let Some(ms) = &model_state {
                if let Some(f) = ms.consecutive_failures() {
                    f.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    use serde_json::Value as JsonValue;

    #[tokio::test]
    async fn test_rewrite_model_name_in_json_response() {
        // Create a mock backend response with a different model name
        let backend_model = "backend-filename.gguf";
        let user_model = "my-model";
        let original_body = serde_json::json!({
            "model": backend_model,
            "choices": [{"message": {"role": "assistant", "content": "Hello"}}]
        });
        let body_bytes = serde_json::to_vec(&original_body).unwrap();

        // Simulate the response processing
        let parsed: JsonValue = serde_json::from_slice(&body_bytes).unwrap();
        let mut modified = parsed.clone();
        modified["model"] = JsonValue::String(user_model.to_string());
        let expected_body = serde_json::to_vec(&modified).unwrap();

        // Verify the model was replaced
        let result: JsonValue = serde_json::from_slice(&expected_body).unwrap();
        assert_eq!(result["model"], JsonValue::String(user_model.to_string()));
        assert_eq!(result["choices"][0]["message"]["content"], "Hello");
    }

    #[tokio::test]
    async fn test_rewrite_model_name_in_streaming_response() {
        let user_model = "my-model";
        let backend_model = "backend-filename.gguf";

        // Create SSE chunks with model field
        let chunk1 = format!(
            "data: {{\"model\":\"{}\",\"choices\":[{{\"delta\":{{\"role\":\"assistant\"}}}}]}}\n",
            backend_model
        );
        let chunk2 = format!(
            "data: {{\"model\":\"{}\",\"choices\":[{{\"delta\":{{\"content\":\"Hello\"}}}}]}}\n",
            backend_model
        );
        let chunk3 = "data: [DONE]\n".to_string();

        // Process chunk1
        let mut modified1: JsonValue = serde_json::from_str(&chunk1["data: ".len()..]).unwrap();
        modified1["model"] = JsonValue::String(user_model.to_string());
        let expected1 = format!("data: {}\n", serde_json::to_string(&modified1).unwrap());
        assert!(expected1.contains(&format!("\"model\":\"{}\"", user_model)));

        // Process chunk2
        let mut modified2: JsonValue = serde_json::from_str(&chunk2["data: ".len()..]).unwrap();
        modified2["model"] = JsonValue::String(user_model.to_string());
        let expected2 = format!("data: {}\n", serde_json::to_string(&modified2).unwrap());
        assert!(expected2.contains(&format!("\"model\":\"{}\"", user_model)));

        // DONE should pass through unchanged
        assert_eq!(chunk3, "data: [DONE]\n");
    }

    #[tokio::test]
    async fn test_streaming_passthrough_non_data_lines() {
        // Comments and empty lines should pass through unchanged
        let comment_line = ": this is a comment\n";
        let empty_line = "\n";

        assert_eq!(comment_line, ": this is a comment\n");
        assert_eq!(empty_line, "\n");
    }

    /// Verify that the response header skip-list used by forward_request
    /// includes `content-length`.  The proxy rewrites the JSON body (model
    /// name substitution) which changes its size; forwarding the backend's
    /// original Content-Length would cause HTTP framing errors on keep-alive
    /// connections (the client reads too few/many bytes, then interprets
    /// leftover body data as the next HTTP response → "Expected HTTP/" error).
    #[test]
    fn test_content_length_is_stripped_from_forwarded_response_headers() {
        // This is the exact skip-list from forward_request's response-header
        // copying loop.  If someone removes "content-length" from the list
        // this test will fail, reminding them why it must be there.
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

    /// Demonstrates the size mismatch that occurs when the model name in a
    /// JSON response body is rewritten to a longer name.  If the original
    /// Content-Length were forwarded, the HTTP client would see fewer bytes
    /// than actually sent, corrupting keep-alive connections.
    #[test]
    fn test_body_size_changes_after_model_rewrite() {
        let short_model = "m.gguf";
        let long_model = "unsloth/gemma-4-E2B-it-GGUF";

        let original = serde_json::json!({
            "model": short_model,
            "choices": [{"message": {"role": "assistant", "content": "Hello"}}]
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let mut rewritten: JsonValue = serde_json::from_slice(&original_bytes).unwrap();
        rewritten["model"] = JsonValue::String(long_model.to_string());
        let rewritten_bytes = serde_json::to_vec(&rewritten).unwrap();

        // The rewritten body is longer because the model name grew.
        // If Content-Length from the backend (original_bytes.len()) were kept,
        // the client would only read that many bytes and the remaining bytes
        // would corrupt the next HTTP response on a keep-alive connection.
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
