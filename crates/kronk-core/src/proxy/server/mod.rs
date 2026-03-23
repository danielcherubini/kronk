pub mod listener;
pub mod router;

use crate::proxy::ProxyState;
use std::sync::Arc;

/// The proxy server, owning shared state and background tasks.
pub struct ProxyServer {
    state: Arc<ProxyState>,
    idle_timeout_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    /// Create a new proxy server with the given shared state.
    ///
    /// Starts a background task that periodically checks for idle models
    /// and unloads them.
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

    /// Cancel the background idle timeout checker.
    pub fn cancel_idle_timeout_checker(&mut self) {
        if let Some(handle) = self.idle_timeout_handle.take() {
            handle.abort();
        }
    }

    /// Consume the server and return a configured axum Router.
    pub fn into_router(self) -> axum::Router {
        router::build_router(self.state)
    }

    /// Start serving on the given address.
    ///
    /// Builds the router and delegates to the listener module.
    pub async fn run(self, addr: std::net::SocketAddr) -> anyhow::Result<()> {
        let app = self.into_router();
        listener::run(app, addr).await
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
