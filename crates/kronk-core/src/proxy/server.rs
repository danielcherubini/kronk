use crate::proxy::ProxyState;
use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use reqwest::Client;
use std::sync::Arc;
use tracing::info;

const MAX_REQUEST_BODY_SIZE: usize = 16 * 1024 * 1024; // 16 MB

fn json_error_response() -> Response {
    Json(serde_json::json!({
        "error": {
            "message": "Bad Request",
            "type": "BadRequestError"
        }
    }))
    .into_response()
}

pub struct ProxyServer {
    state: Arc<ProxyState>,
}

impl ProxyServer {
    pub fn new(state: Arc<ProxyState>) -> Self {
        Self { state }
    }

    pub async fn run(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        info!("Starting proxy server on {}", addr);

        let state_clone = self.state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let _ = state_clone.check_idle_timeouts().await;
            }
        });

        let app = Router::new()
            .route("/chat/completions", post(handle_chat_completions))
            .route("/v1/chat/completions", post(handle_chat_completions))
            .route(
                "/chat/completions/stream",
                post(handle_stream_chat_completions),
            )
            .route("/models", get(handle_list_models))
            .route("/models/:model_id", get(handle_get_model))
            .route("/health", get(handle_health))
            .route("/metrics", get(handle_metrics))
            .fallback(handle_fallback)
            .with_state(self.state.clone());

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

#[axum::debug_handler]
async fn handle_chat_completions(state: State<Arc<ProxyState>>, req: Request<Body>) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return JSON_ERROR_RESPONSE,
    };

    let request: serde_json::Value =
        match serde_json::from_slice(&body_bytes).context("Failed to parse request body") {
            Ok(r) => r,
            Err(_) => {
                return Json(serde_json::json!({
                    "error": {
                        "message": "Bad Request",
                        "type": "BadRequestError"
                    }
                }))
                .into_response();
            }
        };

    let model_name = request
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    info!("Routing request for model: {}", model_name);

    let server_name = match state.get_available_server_for_model(model_name).await {
        Some(name) => name,
        None => {
            let model_card = state.get_model_card(model_name).await;
            match state.load_model(model_name, model_card.as_ref()).await {
                Ok(name) => name,
                Err(e) => {
                    info!("Failed to load model {}: {}", model_name, e);
                    return Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to load model: {}", e),
                            "type": "LoadModelError"
                        }
                    }))
                    .into_response();
                }
            }
        }
    };

    state.update_last_accessed(&server_name).await;

    forward_request(&state, &server_name, &parts, &body_bytes).await
}

#[axum::debug_handler]
async fn handle_stream_chat_completions(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let (parts, body) = req.into_parts();
    let body_bytes = match to_bytes(body, MAX_REQUEST_BODY_SIZE).await {
        Ok(b) => b,
        Err(_) => return JSON_ERROR_RESPONSE,
    };

    let request: serde_json::Value =
        match serde_json::from_slice(&body_bytes).context("Failed to parse request body") {
            Ok(r) => r,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };

    let model_name = request
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    info!("Streaming request for model: {}", model_name);

    let server_name = match state.get_available_server_for_model(model_name).await {
        Some(name) => name,
        None => {
            let model_card = state.get_model_card(model_name).await;
            match state.load_model(model_name, model_card.as_ref()).await {
                Ok(name) => name,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Failed to load model: {}", e),
                    )
                        .into_response();
                }
            }
        }
    };

    state.update_last_accessed(&server_name).await;

    forward_request(&state, &server_name, &parts, &body_bytes).await
}

#[axum::debug_handler]
async fn handle_get_model(
    state: State<Arc<ProxyState>>,
    model_id: String,
) -> Json<serde_json::Value> {
    let model_state = state.get_model_state(&model_id).await;

    if let Some(state) = model_state {
        Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": state.load_time.elapsed().as_secs(),
            "owned_by": state.backend,
            "ready": true
        }))
    } else {
        Json(serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        }))
    }
}

#[axum::debug_handler]
async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "kronk-proxy"
    }))
}

#[axum::debug_handler]
async fn handle_metrics(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let metrics = &state.metrics;
    Json(serde_json::json!({
        "total_requests": metrics.total_requests.load(std::sync::atomic::Ordering::Relaxed),
        "successful_requests": metrics.successful_requests.load(std::sync::atomic::Ordering::Relaxed),
        "failed_requests": metrics.failed_requests.load(std::sync::atomic::Ordering::Relaxed),
        "models_loaded": metrics.models_loaded.load(std::sync::atomic::Ordering::Relaxed),
        "models_unloaded": metrics.models_unloaded.load(std::sync::atomic::Ordering::Relaxed),
        "active_models": state.models.read().await.len(),
    }))
}

#[axum::debug_handler]
async fn handle_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models: Vec<String> = state.models.read().await.keys().cloned().collect();

    Json(serde_json::json!({
        "object": "list",
        "data": models
    }))
}

#[axum::debug_handler]
async fn handle_fallback() -> StatusCode {
    StatusCode::NOT_FOUND
}

async fn forward_request(
    state: &Arc<ProxyState>,
    server_name: &str,
    parts: &axum::http::request::Parts,
    body_bytes: &[u8],
) -> Response {
    state
        .metrics
        .total_requests
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let model_state = state.get_model_state(server_name).await;
    if let Some(ms) = &model_state {
        let failures = ms
            .consecutive_failures
            .load(std::sync::atomic::Ordering::Relaxed);
        if failures >= state.config.circuit_breaker_threshold {
            info!(
                "Circuit breaker tripped for server '{}' ({} failures). Unloading server.",
                server_name, failures
            );
            let _ = state.unload_model(server_name).await;
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

    let backend_url = state
        .get_backend_url(server_name)
        .await
        .unwrap_or_else(|e| {
            info!("Failed to get backend URL for {}: {}", server_name, e);
            "http://127.0.0.1:8080".to_string()
        });

    // Combine backend_url with the request path
    let target_uri = if parts.uri.path().starts_with('/') {
        format!("{}{}", backend_url, parts.uri.path())
    } else {
        format!("{}{}", &backend_url[..backend_url.len()-1], parts.uri.path())
    };

    info!("Forwarding request to: {}", target_uri);

    let client = Client::new();
    let method = parts.method.clone();

    let mut headers = reqwest::header::HeaderMap::new();
    for (key, value) in &parts.headers {
        // Skip hop-by-hop headers: connection, keep-alive, proxy-authenticate,
        // proxy-authorization, te, transfer-encoding, upgrade, trailer
        if key.as_str() != "connection"
            && key.as_str() != "keep-alive"
            && key.as_str() != "proxy-authenticate"
            && key.as_str() != "proxy-authorization"
            && key.as_str() != "te"
            && key.as_str() != "transfer-encoding"
            && key.as_str() != "upgrade"
            && key.as_str() != "trailer"
        {
            if let Ok(v) = value.to_str() {
                headers.insert(key.clone(), value.clone());
            }
        }
    }

    match client
        .request(method, &target_uri)
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
                    ms.consecutive_failures
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                }
            } else {
                state
                    .metrics
                    .failed_requests
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if status.is_server_error() {
                    if let Some(ms) = &model_state {
                        ms.consecutive_failures
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }

            let mut builder = Response::builder().status(status);

            for (key, value) in response.headers().iter() {
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
                ms.consecutive_failures
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            info!("Failed to forward request: {}", e);
            (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Backend error: {}", e),
                        "type": "BadRequestError"
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
    use axum::http::Request;
    use reqwest::Method;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_sse_streaming_response() {
        let config = crate::proxy::ProxyConfig::default();
        let registry = crate::backends::registry::BackendRegistry::default();
        let config_data = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, registry, config_data));

        let socket = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            0,
        );

        let server = ProxyServer::new(state.clone());
        tokio::spawn(async move {
            server.run(socket).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let response: Request<Body> = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header("content-type", "text/event-stream; charset=utf-8")
            .body(Body::from("test"))
            .unwrap();

        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/event-stream; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_sse_streaming_timeout() {
        let config = crate::proxy::ProxyConfig::default();
        let registry = crate::backends::registry::BackendRegistry::default();
        let config_data = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, registry, config_data));

        let socket = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            0,
        );

        let server = ProxyServer::new(state.clone());
        tokio::spawn(async move {
            server.run(socket).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let response_body = String::from("data: {\"id\":\"1\"}\n\n");
        let response_bytes = response_body.into_bytes();

        let response: Request<Body> = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header("content-type", "text/event-stream; charset=utf-8")
            .body(response_bytes.into())
            .unwrap();

        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/event-stream; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn test_sse_streaming_error_response() {
        let config = crate::proxy::ProxyConfig::default();
        let registry = crate::backends::registry::BackendRegistry::default();
        let config_data = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, registry, config_data));

        let socket = std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            0,
        );

        let server = ProxyServer::new(state.clone());
        tokio::spawn(async move {
            server.run(socket).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let response_body = String::from("data: {\"error\":{\"message\":\"Backend error\",\"type\":\"InternalServerError\"}}\n\n");
        let response_bytes = response_body.into_bytes();

        let response: Request<Body> = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header("content-type", "text/event-stream; charset=utf-8")
            .body(response_bytes.into())
            .unwrap();

        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/event-stream; charset=utf-8"
        );
    }
}
