pub mod listener;
pub mod router;

use crate::proxy::ProxyState;
use std::sync::Arc;

/// The proxy server, owning shared state and background tasks.
pub struct ProxyServer {
    state: Arc<ProxyState>,
    #[allow(dead_code)]
    idle_timeout_handle: Option<tokio::task::JoinHandle<()>>,
    #[allow(dead_code)]
    metrics_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    /// Create a new proxy server with the given shared state.
    ///
    /// Starts a background task that periodically checks for idle models
    /// and unloads them.
    pub fn new(state: Arc<ProxyState>) -> Self {
        Self::cleanup_stale_processes(&state);
        let handle = Self::start_idle_timeout_checker(state.clone());

        // Spawn background task to refresh system metrics every 5s.
        // `sysinfo::System` is created once and moved into the closure so that
        // CPU-usage deltas are computed correctly across iterations without the
        // per-call MINIMUM_CPU_UPDATE_INTERVAL sleep.
        let metrics_arc = Arc::clone(&state.system_metrics);
        let metrics_handle = tokio::spawn(async move {
            let mut sys = sysinfo::System::new();
            loop {
                let (snapshot, returned_sys) = tokio::task::spawn_blocking(move || {
                    let snapshot = crate::gpu::collect_system_metrics_with(&mut sys);
                    (snapshot, sys)
                })
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("system metrics collection panicked: {}", e);
                    (crate::gpu::SystemMetrics::default(), sysinfo::System::new())
                });
                sys = returned_sys;
                *metrics_arc.write().await = snapshot;
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        });

        Self {
            state,
            idle_timeout_handle: Some(handle),
            metrics_handle: Some(metrics_handle),
        }
    }

    fn cleanup_stale_processes(state: &ProxyState) {
        if let Some(conn) = state.open_db() {
            if let Ok(active) = crate::db::queries::get_active_models(&conn) {
                for entry in &active {
                    let pid = entry.pid as u32;
                    if !super::process::is_process_alive(pid) {
                        tracing::info!(
                            "Cleaning up stale process entry: {} (pid {})",
                            entry.server_name,
                            pid
                        );
                        let _ = crate::db::queries::remove_active_model(&conn, &entry.server_name);
                    } else {
                        tracing::warn!(
                            "Orphaned backend process detected: {} (pid {}). Killing.",
                            entry.server_name,
                            pid
                        );
                        #[cfg(unix)]
                        let _ = std::process::Command::new("kill")
                            .arg(pid.to_string())
                            .status();
                        #[cfg(windows)]
                        let _ = std::process::Command::new("taskkill")
                            .args(["/PID", &pid.to_string(), "/F"])
                            .status();
                        let _ = crate::db::queries::remove_active_model(&conn, &entry.server_name);
                    }
                }
            }
            // Belt-and-suspenders: clear any entries that survived the loop
            // (e.g. if get_active_models failed mid-way and the loop was skipped).
            let _ = crate::db::queries::clear_active_models(&conn);
        }
    }

    fn start_idle_timeout_checker(state: Arc<ProxyState>) -> tokio::task::JoinHandle<()> {
        use std::time::Duration;
        tokio::spawn(async move {
            let interval =
                Duration::from_secs((state.config.read().await.proxy.idle_timeout_secs / 2).max(1));
            loop {
                tokio::time::sleep(interval).await;
                let _ = state.check_idle_timeouts().await;
            }
        })
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
        let state = Arc::new(crate::proxy::ProxyState::new(config, None));

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
}
