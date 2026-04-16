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
    pub async fn new(state: Arc<ProxyState>) -> Self {
        // Auto-migrate koji.toml model entries to DB on first startup.
        if let Some(conn) = state.open_db() {
            let mut config = state.config.write().await;
            match crate::config::migrate::model_to_db::migrate_models_to_db(&conn, &mut config) {
                Ok(count) => {
                    if count > 0 {
                        tracing::info!("Auto-migrated {} models from koji.toml to database", count);
                    }
                }
                Err(e) => tracing::error!("Automatic model migration failed: {}", e),
            }

            // Populate in-memory model registry from DB
            match crate::db::load_model_configs(&conn) {
                Ok(db_models) if !db_models.is_empty() => {
                    tracing::info!("Loaded {} models from database", db_models.len());
                    config.models = db_models;
                }
                Ok(_) => {}
                Err(e) => tracing::error!("Failed to load model configs from database: {}", e),
            }
        }

        Self::cleanup_stale_processes(&state).await;
        let handle = Self::start_idle_timeout_checker(state.clone());

        // Spawn background task to refresh system metrics every 2s.
        // Each tick: collect metrics, update the cached snapshot, persist to SQLite
        // (best-effort, with inline pruning), and broadcast to SSE subscribers.
        let metrics_state = Arc::clone(&state);
        let metrics_handle = tokio::spawn(async move {
            use std::time::{SystemTime, UNIX_EPOCH};
            let mut sys = sysinfo::System::new();
            loop {
                // Collect metrics on a blocking thread.
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

                // Update the cached snapshot read by /koji/v1/system/health.
                *metrics_state.system_metrics.write().await = snapshot.clone();

                // Build a timestamped MetricSample.
                let ts_unix_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                let model_statuses = metrics_state.collect_model_statuses().await;
                let models_loaded = model_statuses.iter().filter(|m| m.loaded).count() as u64;
                let sample = crate::gpu::MetricSample {
                    ts_unix_ms,
                    cpu_usage_pct: snapshot.cpu_usage_pct,
                    ram_used_mib: snapshot.ram_used_mib,
                    ram_total_mib: snapshot.ram_total_mib,
                    gpu_utilization_pct: snapshot.gpu_utilization_pct,
                    vram: snapshot.vram.clone(),
                    models_loaded,
                    models: model_statuses,
                };

                // Persist to SQLite (best-effort). Read retention from config.
                let retention_secs = metrics_state
                    .config
                    .read()
                    .await
                    .proxy
                    .metrics_retention_secs;
                if let Some(conn) = metrics_state.open_db() {
                    let row = crate::db::queries::SystemMetricsRow {
                        ts_unix_ms: sample.ts_unix_ms,
                        cpu_usage_pct: sample.cpu_usage_pct,
                        ram_used_mib: sample.ram_used_mib as i64,
                        ram_total_mib: sample.ram_total_mib as i64,
                        gpu_utilization_pct: sample.gpu_utilization_pct.map(|v| v as i64),
                        vram_used_mib: sample.vram.as_ref().map(|v| v.used_mib as i64),
                        vram_total_mib: sample.vram.as_ref().map(|v| v.total_mib as i64),
                        models_loaded: sample.models_loaded as i64,
                    };
                    let cutoff_ms = sample.ts_unix_ms - (retention_secs as i128 * 1000) as i64;
                    // Run the blocking SQLite call off the runtime.
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) =
                            crate::db::queries::insert_system_metric(&conn, &row, cutoff_ms)
                        {
                            tracing::warn!("failed to persist system metric: {}", e);
                        }
                    })
                    .await;
                }

                // Broadcast to any live SSE subscribers. SendError just means there are
                // no subscribers; that is the normal idle case.
                let _ = metrics_state.metrics_tx.send(sample);

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        });

        Self {
            state,
            idle_timeout_handle: Some(handle),
            metrics_handle: Some(metrics_handle),
        }
    }

    async fn cleanup_stale_processes(state: &ProxyState) {
        let conn = match state.open_db() {
            Some(c) => c,
            None => return,
        };
        let active = match crate::db::queries::get_active_models(&conn) {
            Ok(a) => a,
            Err(_) => return,
        };

        for entry in &active {
            let pid = entry.pid as u32;
            if !super::process::is_process_alive(pid) {
                tracing::info!(
                    "Cleaning up stale process entry: {} (pid {})",
                    entry.server_name,
                    pid
                );
                let _ = crate::db::queries::remove_active_model(&conn, &entry.server_name);
                continue;
            }

            // Process is alive — try to reconnect by health-checking it
            let health_url = format!("http://127.0.0.1:{}/health", entry.port);
            let healthy = match super::process::check_health(&health_url, Some(5)).await {
                Ok(resp) => resp.status().is_success(),
                Err(_) => false,
            };

            if healthy {
                tracing::info!(
                    "Reconnecting to existing backend: {} (pid {}, port {})",
                    entry.server_name,
                    pid,
                    entry.port
                );
                let mut models = state.models.write().await;
                models.insert(
                    entry.server_name.clone(),
                    super::types::ModelState::Ready {
                        model_name: entry.model_name.clone(),
                        backend: entry.backend.clone(),
                        backend_pid: pid,
                        backend_url: entry.backend_url.clone(),
                        load_time: std::time::SystemTime::now(),
                        last_accessed: std::time::Instant::now(),
                        consecutive_failures: std::sync::Arc::new(
                            std::sync::atomic::AtomicU32::new(0),
                        ),
                        failure_timestamp: None,
                    },
                );
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
    use futures_util::StreamExt;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_proxy_routes_exist() {
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, None));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone()).await;
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
    async fn test_metrics_task_persists_to_db() {
        let tmp = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        let _server = ProxyServer::new(state.clone()).await;

        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        let conn = state.open_db().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM system_metrics_history", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(
            count >= 1,
            "Expected at least 1 row in system_metrics_history after 2s, got {}",
            count
        );
    }

    #[tokio::test]
    async fn test_metrics_task_broadcasts_samples() {
        let tmp = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        let mut rx = state.metrics_tx.subscribe();

        let _server = ProxyServer::new(state.clone()).await;

        let result = tokio::time::timeout(std::time::Duration::from_secs(4), rx.recv()).await;
        assert!(
            result.is_ok(),
            "Expected to receive a MetricSample within 4s, but timeout occurred"
        );
        let sample = result.unwrap().unwrap();
        assert!(sample.ts_unix_ms > 0, "ts_unix_ms should be positive");
        assert!(
            sample.cpu_usage_pct >= 0.0,
            "cpu_usage_pct should be non-negative"
        );
        assert!(sample.ram_total_mib > 0, "ram_total_mib should be positive");
    }

    #[tokio::test]
    async fn test_metric_sample_broadcast_populates_models_field() {
        use crate::config::ModelConfig;
        use std::collections::BTreeMap;

        let tmp = tempfile::tempdir().unwrap();

        // Build a Config with exactly one known model so the assertions are
        // deterministic. We clear the default fixtures shipped by
        // `Config::default()` first.
        let mut config = crate::config::Config::default();
        config.models.clear();
        config.models.insert(
            "alpha".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,

                mmproj: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                api_name: None,
                gpu_layers: None,
                quants: BTreeMap::new(),
                modalities: None,
                display_name: None,
            },
        );

        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        // Subscribe BEFORE starting the server so we don't miss the first tick.
        let mut rx = state.metrics_tx.subscribe();

        let _server = ProxyServer::new(state.clone()).await;

        let sample = tokio::time::timeout(std::time::Duration::from_secs(4), rx.recv())
            .await
            .expect("Expected to receive a MetricSample within 4s, but timeout occurred")
            .expect("metrics_tx channel closed before any sample was broadcast");

        // The metrics loop must populate `MetricSample.models` from
        // `ProxyState::collect_model_statuses`, which reflects the current
        // configuration.
        assert_eq!(
            sample.models.len(),
            1,
            "Expected exactly one model in sample.models, got: {:?}",
            sample.models
        );
        assert_eq!(sample.models[0].id, "alpha");
        assert_eq!(sample.models[0].backend, "llama_cpp");
        assert!(
            !sample.models[0].loaded,
            "Expected the configured model to be reported as loaded == false since no backend was started, got: {:?}",
            sample.models[0]
        );
        assert_eq!(
            sample.models_loaded, 0,
            "Expected models_loaded counter to be 0 when no model is loaded"
        );
    }

    #[tokio::test]
    async fn test_system_metrics_stream_emits_samples() {
        use bytes::Bytes;

        let tmp = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone()).await;
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "http://{}/koji/v1/system/metrics/stream",
                bound_addr
            ))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("text/event-stream"));

        let mut stream = response.bytes_stream();
        let mut found_sample = false;
        while let Some(chunk) =
            tokio::time::timeout(std::time::Duration::from_secs(4), stream.next())
                .await
                .unwrap()
        {
            let chunk: Bytes = chunk.unwrap();
            let data = String::from_utf8_lossy(&chunk);
            if data.contains("event: sample") {
                // Parse the data: line to extract data: line
                for line in data.lines() {
                    if let Some(data_line) = line.strip_prefix("data: ") {
                        let sample: crate::gpu::MetricSample =
                            serde_json::from_str(data_line).unwrap();
                        assert!(sample.ts_unix_ms > 0);
                        assert!(sample.ram_total_mib > 0);
                        found_sample = true;
                        break;
                    }
                }
                if found_sample {
                    break;
                }
            }
        }

        assert!(
            found_sample,
            "Expected to receive a sample event within 4s, but none was found"
        );
    }

    /// Round-trip test: the SSE `sample` events emitted by
    /// `/koji/v1/system/metrics/stream` must serialize the new
    /// `MetricSample.models` field in a wire format that the client-side
    /// `crate::gpu::MetricSample` Deserialize impl can read back without
    /// error.
    ///
    /// We configure the proxy with exactly one known model so the assertions
    /// over the deserialized `Vec<ModelStatus>` are deterministic, then
    /// connect to the SSE endpoint, wait for an `event: sample`, parse the
    /// `data:` payload as a `MetricSample`, and assert that
    /// `sample.models` is a `Vec<crate::gpu::ModelStatus>` carrying the
    /// configured model.
    #[tokio::test]
    async fn test_system_metrics_stream_sample_models_round_trip() {
        use crate::config::ModelConfig;
        use bytes::Bytes;
        use std::collections::BTreeMap;

        let tmp = tempfile::tempdir().unwrap();

        // Build a Config with exactly one known model so the deserialized
        // `sample.models` Vec has a deterministic shape we can assert on.
        let mut config = crate::config::Config::default();
        config.models.clear();
        config.models.insert(
            "alpha".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,

                mmproj: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                api_name: None,
                gpu_layers: None,
                quants: BTreeMap::new(),
                modalities: None,
                display_name: None,
            },
        );

        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bound_addr = listener.local_addr().unwrap();

        let server = ProxyServer::new(state.clone()).await;
        let app = server.into_router();
        let _handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let client = reqwest::Client::new();
        let response = client
            .get(format!(
                "http://{}/koji/v1/system/metrics/stream",
                bound_addr
            ))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("text/event-stream"));

        let mut stream = response.bytes_stream();
        let mut parsed_sample: Option<crate::gpu::MetricSample> = None;
        let mut buf = String::new();
        while let Some(chunk) =
            tokio::time::timeout(std::time::Duration::from_secs(4), stream.next())
                .await
                .unwrap()
        {
            let chunk: Bytes = chunk.unwrap();
            buf.push_str(&String::from_utf8_lossy(&chunk));

            // SSE events are delimited by a blank line. Iterate over each
            // complete event currently in the buffer.
            while let Some(idx) = buf.find("\n\n") {
                let event_block = buf[..idx].to_string();
                buf = buf[idx + 2..].to_string();

                let mut event_name: Option<&str> = None;
                let mut data_line: Option<&str> = None;
                for line in event_block.lines() {
                    if let Some(rest) = line.strip_prefix("event: ") {
                        event_name = Some(rest);
                    } else if let Some(rest) = line.strip_prefix("data: ") {
                        data_line = Some(rest);
                    }
                }

                if event_name == Some("sample") {
                    let data_line = data_line
                        .expect("sample event must include a data: line carrying the JSON payload");
                    // The critical assertion: the JSON produced by the
                    // server must deserialize cleanly into MetricSample,
                    // including the new `models` field.
                    let sample: crate::gpu::MetricSample = serde_json::from_str(data_line)
                        .expect("MetricSample JSON from SSE stream must deserialize without error");
                    parsed_sample = Some(sample);
                    break;
                }
            }

            if parsed_sample.is_some() {
                break;
            }
        }

        let sample = parsed_sample
            .expect("Expected to receive a sample event within 4s, but none was found");

        // Statically prove `sample.models` is a `Vec<crate::gpu::ModelStatus>`.
        // If the field's type ever changes, this binding will fail to
        // type-check, which is exactly the regression we want to catch.
        let models: &Vec<crate::gpu::ModelStatus> = &sample.models;

        // The configured model must round-trip through JSON serialization
        // unchanged. We picked a deterministic single-model config above so
        // we can assert on the exact contents.
        assert_eq!(
            models.len(),
            1,
            "Expected exactly one model in sample.models after JSON round-trip, got: {:?}",
            models
        );
        assert_eq!(models[0].id, "alpha");
        assert_eq!(models[0].backend, "llama_cpp");
        assert!(
            !models[0].loaded,
            "Expected the configured model to be reported as loaded == false since no backend was started, got: {:?}",
            models[0]
        );
        assert_eq!(
            sample.models_loaded, 0,
            "Expected models_loaded counter to be 0 when no model is loaded"
        );
    }

    #[tokio::test]
    async fn test_proxy_loads_models_from_db_on_startup() {
        use crate::config::ModelConfig;
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        // Pre-populate DB with a model config
        {
            let open_res = crate::db::open(&db_dir).unwrap();
            let conn = open_res.conn;
            let mc = ModelConfig {
                backend: "llama_cpp".to_string(),
                display_name: Some("DB Model".to_string()),
                ..Default::default()
            };
            crate::db::save_model_config(&conn, "db-model-key", &mc).unwrap();
        }

        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(config, Some(db_dir)));

        // Start the server (which should load models from DB)
        let _server = ProxyServer::new(state.clone()).await;

        // Verify that the model from DB is now in the proxy state
        let config = state.config.read().await;
        assert!(
            config.models.contains_key("db-model-key"),
            "Expected model 'db-model-key' to be loaded from DB"
        );
        let model = config.models.get("db-model-key").unwrap();
        assert_eq!(model.display_name.as_deref(), Some("DB Model"));
    }
}
