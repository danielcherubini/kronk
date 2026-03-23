use crate::proxy::{ModelState, ProxyState};
use axum::{
    body::Body,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Json, Response},
};
use std::sync::Arc;
use std::time::SystemTime;
use tracing::info;

pub async fn forward_request(
    state: &Arc<ProxyState>,
    server_name: &str,
    parts: &Parts,
    body_bytes: &[u8],
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
        if failures >= state.config.proxy.circuit_breaker_threshold {
            // Check if cooldown has elapsed
            if !ms.can_reload(state.config.proxy.circuit_breaker_cooldown_seconds) {
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
                        if ms.is_ready() {
                            let new_ts = SystemTime::now();
                            let mut models = state.models.write().await;
                            #[allow(clippy::collapsible_match)]
                            if let Some(existing) = models.get_mut(server_name) {
                                match existing {
                                    ModelState::Ready {
                                        failure_timestamp, ..
                                    }
                                    | ModelState::Starting {
                                        failure_timestamp,
                                        ..
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
                // Skip hop-by-hop headers in response
                if [
                    "connection",
                    "keep-alive",
                    "proxy-authenticate",
                    "proxy-authorization",
                    "te",
                    "transfer-encoding",
                    "upgrade",
                    "trailer",
                ]
                .contains(&key.as_str())
                {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    builder = builder.header(key.as_str(), v);
                }
            }

            let body = Body::from_stream(response.bytes_stream());
            builder.body(body).unwrap().into_response()
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
