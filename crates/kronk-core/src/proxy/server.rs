use crate::proxy::ProxyState;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tracing::info;

use super::handlers::{
    handle_chat_completions, handle_fallback, handle_get_model, handle_health,
    handle_list_models, handle_metrics, handle_status, handle_stream_chat_completions,
};

pub struct ProxyServer {
    state: Arc<ProxyState>,
    idle_timeout_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    pub fn new(state: Arc<ProxyState>) -> Self {
        let handle = Self::start_idle_timeout_checker(state.clone());
        Self {
            state,
            idle_timeout_handle: Some(handle),
        }
    }

    fn start_idle_timeout_checker(state: Arc<ProxyState>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let _ = state.check_idle_timeouts().await;
            }
        })
    }

    pub fn cancel_idle_timeout_checker(&mut self) {
        if let Some(handle) = self.idle_timeout_handle.take() {
            handle.abort();
        }
    }

    pub fn into_router(self) -> Router {
        Router::new()
            .route("/v1/chat/completions", post(handle_chat_completions))
            .route(
                "/v1/chat/completions/stream",
                post(handle_stream_chat_completions),
            )
            .route("/v1/models", get(handle_list_models))
            .route("/v1/models/:model_id", get(handle_get_model))
            .route("/status", get(handle_status))
            .route("/health", get(handle_health))
            .route("/metrics", get(handle_metrics))
            .fallback(handle_fallback)
            .with_state(self.state.clone())
    }

    pub async fn run(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        info!("Starting proxy server on {}", addr);

        let app = self.into_router();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_proxy_routes_exist() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test health endpoint
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{}/health", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        // Test models endpoint
        let response = client
            .get(format!("http://{}/v1/models", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);

        // Test status endpoint
        let response = client
            .get(format!("http://{}/status", bound_addr))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_chat_completions_route() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/v1/chat/completions", bound_addr))
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 500); // Fails to load unknown model
    }

    #[tokio::test]
    async fn test_stream_route() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .post(format!("http://{}/v1/chat/completions/stream", bound_addr))
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "hello"}]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 500); // Fails to load unknown model
    }

    #[tokio::test]
    async fn test_status_endpoint_response_structure() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone());
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{}/status", bound_addr))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);

        let body = response.text().await.unwrap();
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();

        assert!(json.get("idle_timeout_secs").is_some());
        assert!(json.get("models").unwrap().is_object());
        assert!(json.get("metrics").unwrap().is_object());
    }
}
