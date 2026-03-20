use crate::proxy::ProxyState;
use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{MatchedPath, OriginalUri},
    http::{header, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};

pub struct ProxyServer {
    state: Arc<ProxyState>,
}

impl ProxyServer {
    pub fn new(state: Arc<ProxyState>) -> Self {
        Self { state }
    }

    pub async fn run(self, addr: SocketAddr) -> Result<()> {
        info!("Starting proxy server on {}", addr);

        let app = Router::new()
            .route("/chat/completions", post(handle_chat_completions))
            .route("/v1/chat/completions", post(handle_chat_completions))
            .route("/models", get(handle_list_models))
            .route("/models/:model_id", get(handle_get_model))
            .route("/health", get(handle_health))
            .fallback(handle_fallback)
            .layer(TraceLayer::new_for_http())
            .with_state(self.state);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

async fn handle_chat_completions(
    state: Arc<ProxyState>,
    body: Bytes,
    matched_path: MatchedPath,
    uri: OriginalUri,
) -> Result<impl IntoResponse> {
    debug!(
        "Received chat/completions request to {}",
        matched_path.as_str()
    );

    // Parse the request body to extract model name
    let request: serde_json::Value =
        serde_json::from_slice(&body).context("Failed to parse request body")?;

    let model_name = request
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing 'model' field in request"))?;

    info!("Routing request for model: {}", model_name);

    // Check if model is already loaded
    let is_loaded = state.is_model_loaded(model_name).await;

    if !is_loaded {
        // Try to load the model
        // For now, we'll just return an error since we don't have the model card
        // In a full implementation, we would load the model card and start the backend
        warn!("Model '{}' not loaded, skipping", model_name);
    }

    // Update last accessed time
    state.update_last_accessed(model_name).await;

    // Forward the request to the backend
    // For now, we'll just return a placeholder response
    // In a full implementation, we would:
    // 1. Find the correct backend URL
    // 2. Forward the request
    // 3. Stream the response back

    let response = serde_json::json!({
        "error": {
            "message": "Proxy not fully implemented yet",
            "type": "InternalServerError"
        }
    });

    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        response,
    ))
}

async fn handle_list_models(state: Arc<ProxyState>) -> impl IntoResponse {
    let models: Vec<String> = state
        .models
        .read()
        .await
        .keys()
        .cloned()
        .collect();

    let response = serde_json::json!({
        "object": "list",
        "data": models
    });

    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        response,
    ))
}

async fn handle_get_model(state: Arc<ProxyState>, model_id: String) -> impl IntoResponse {
    let model_state = state.get_model_state(&model_id).await;

    let response = if let Some(state) = model_state {
        serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": state.load_time.elapsed().as_secs(),
            "owned_by": state.backend,
            "ready": true
        })
    } else {
        serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        })
    };

    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        response,
    ))
}

async fn handle_health() -> impl IntoResponse {
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "status": "ok",
            "service": "kronk-proxy"
        }),
    ))
}

async fn handle_fallback() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        "Not Found",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_server_creation() {
        let state = Arc::new(ProxyState {
            config: ProxyConfig::default(),
            models: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            registry: Arc::new(tokio::sync::RwLock::new(crate::backends::registry::BackendRegistry::default())),
            config_data: Arc::new(tokio::sync::RwLock::new(crate::config::Config::default())),
        });

        let server = ProxyServer::new(state);
        assert!(true);
    }
}