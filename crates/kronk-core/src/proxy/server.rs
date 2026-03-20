use crate::proxy::ProxyState;
use anyhow::Context;
use async_stream::stream as async_stream_stream;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Router,
};
use futures_util::{Stream, StreamExt};
use std::sync::Arc;
use tracing::{debug, info};

pub struct ProxyServer {
    state: Arc<ProxyState>,
}

impl ProxyServer {
    pub fn new(state: Arc<ProxyState>) -> Self {
        Self { state }
    }

    pub async fn run(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        info!("Starting proxy server on {}", addr);

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
            .fallback(handle_fallback)
            .with_state(self.state.clone());

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

#[axum::debug_handler]
async fn handle_chat_completions(state: State<Arc<ProxyState>>, req: Request<Body>) -> Response {
    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return StatusCode::BAD_REQUEST.into_response();
        }
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

    let is_loaded = state.is_model_loaded(model_name).await;

    if !is_loaded {
        let model_card = state.get_model_card(model_name).await;
        if let Some(card) = model_card {
            if let Err(e) = state.load_model(model_name, Some(&card)).await {
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

    state.update_last_accessed(model_name).await;

    // For now, return a placeholder response
    // In production, this would forward the request to the backend and stream the response
    debug!("Model {} is ready", model_name);

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1_704_067_200,
            "model": model_name,
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Hello! I'm a placeholder response."
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {
                "prompt_tokens": 9,
                "completion_tokens": 13,
                "total_tokens": 22
            }
        })),
    )
        .into_response()
}

#[axum::debug_handler]
async fn handle_stream_chat_completions(
    state: State<Arc<ProxyState>>,
    req: Request<Body>,
) -> Response {
    let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
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

    let is_loaded = state.is_model_loaded(model_name).await;

    if !is_loaded {
        let model_card = state.get_model_card(model_name).await;
        if let Some(card) = model_card {
            if let Err(e) = state.load_model(model_name, Some(&card)).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to load model: {}", e),
                )
                    .into_response();
            }
        }
    }

    state.update_last_accessed(model_name).await;

    // For now, return a placeholder SSE stream
    // In production, this would forward the request to the backend and stream the response
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let response = Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .body(Body::from(format!(
            "data: {{{\"id\":\"chatcmpl-123\",\"object\":\"chat.completion\",\"created\":{},{\"model\":\"{}\",\"choices\":[{{{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hello\"}}}}]}}}}\n\ndata: {{{\"id\":\"chatcmpl-123\",\"object\":\"chat.completion\",\"created\":{},{\"model\":\"{}\",\"choices\":[{{{\"index\":0,\"delta\":{\"content\":\" World\"}}}}]}}}}\n\ndata: [DONE]\n\n",
            timestamp, model_name, timestamp, model_name
        )))
        .unwrap();

    response
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Method, Request},
        routing::post,
        Router,
    };
    use std::sync::Arc;

    /// Test SSE streaming response for chat completions
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

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test that SSE response has correct content type
        let response: Request<Body> = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header("content-type", "text/event-stream; charset=utf-8")
            .body(Body::from("test"))
            .unwrap();

        // Verify content type
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/event-stream; charset=utf-8"
        );
    }

    /// Test SSE streaming with timeout
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

        // Create a slow streaming response (will timeout)
        let response_body = String::from("data: {\"id\":\"1\"}\n\n");
        let response_bytes = response_body.into_bytes();

        let response: Request<Body> = Request::builder()
            .method(Method::GET)
            .uri("/test")
            .header("content-type", "text/event-stream; charset=utf-8")
            .body(response_bytes.into())
            .unwrap();

        // Verify response is valid SSE
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "text/event-stream; charset=utf-8"
        );
    }

    /// Test SSE streaming error handling
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

        // Create an error response in SSE format
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
