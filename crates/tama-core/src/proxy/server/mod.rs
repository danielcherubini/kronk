pub mod listener;
pub mod router;

use crate::proxy::ProxyState;
use std::collections::VecDeque;
use std::sync::Arc;

/// The proxy server, owning shared state and background tasks.
pub struct ProxyServer {
    state: Arc<ProxyState>,
    /// Handle for the idle timeout checker task. Kept to prevent task cancellation.
    #[allow(dead_code)]
    idle_timeout_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle for the system metrics collection task. Kept to prevent task cancellation.
    #[allow(dead_code)]
    metrics_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ProxyServer {
    /// Create a new proxy server with the given shared state.
    ///
    /// Starts a background task that periodically checks for idle models
    /// and unloads them.
    pub async fn new(state: Arc<ProxyState>) -> Self {
        // Populate in-memory model registry from DB
        if let Some(conn) = state.open_db() {
            // First, run migration from tama.toml to DB if needed.
            {
                let mut config = state.config.write().await;
                if let Err(e) =
                    crate::config::migrate::model_to_db::migrate_models_to_db(&conn, &mut config)
                {
                    tracing::error!("Failed to migrate models from tama.toml to DB: {}", e);
                }
            }

            // Repair model_configs rows whose model_files were wiped by the
            // v9 FK-cascade bug. No-op when rows are intact.
            {
                let config = state.config.read().await;
                match config.models_dir() {
                    Ok(models_dir) => {
                        if let Err(e) =
                            crate::db::backfill::repair_orphaned_model_files(&conn, &models_dir)
                        {
                            tracing::warn!("repair_orphaned_model_files failed: {}", e);
                        }
                    }
                    Err(e) => tracing::debug!("models_dir unavailable for repair scan: {}", e),
                }
            }

            match crate::db::load_model_configs(&conn) {
                Ok(db_models) if !db_models.is_empty() => {
                    tracing::info!("Loaded {} models from database", db_models.len());
                    *state.model_configs.write().await = db_models;
                }
                Ok(_) => {}
                Err(e) => tracing::error!("Failed to load model configs from database: {}", e),
            }

            // Check if any models need HF metadata backfill (after migration v19).
            // If so, spawn a background task to fetch and populate the columns.
            let needs_backfill = conn
                .query_row(
                    "SELECT COUNT(*) FROM model_configs WHERE hf_format IS NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap_or(0);
            if needs_backfill > 0 {
                let db_dir = state.db_dir.clone();
                // Fire-and-forget background task — doesn't block startup.
                // JoinHandle is intentionally dropped; backfill errors are logged internally.
                let _handle = tokio::spawn(async move {
                    if let Some(dir) = db_dir {
                        if let Err(e) = crate::db::backfill::backfill_hf_metadata(&dir).await {
                            tracing::warn!("HF metadata backfill failed: {}", e);
                        }
                    }
                });
            }
        }

        Self::cleanup_stale_processes(&state).await;
        let idle_timeout_handle = Self::start_idle_timeout_checker(state.clone());

        // Seed in-memory history buffer from SQLite.
        let mut history_buf: VecDeque<crate::gpu::MetricSample> = VecDeque::with_capacity(450);
        if let Some(seed_conn) = state.open_db() {
            if let Ok(rows) = crate::db::queries::get_recent_system_metrics(&seed_conn, 450) {
                for row in rows {
                    history_buf.push_back(Self::row_into_sample(&row));
                }
            }
        }

        // Spawn background task to refresh system metrics every 2s.
        // Each tick: collect metrics, build unified sample (system + inference),
        // persist to SQLite, update in-memory buffer, broadcast full buffer.
        let metrics_state = Arc::clone(&state);
        let metrics_handle = tokio::spawn(async move {
            use std::time::{SystemTime, UNIX_EPOCH};
            let mut sys = sysinfo::System::new();
            loop {
                // 1. Collect system metrics (spawn_blocking, unchanged pattern)
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

                // Update the cached snapshot read by /tama/v1/system/health.
                *metrics_state.system_metrics.write().await = snapshot.clone();

                // 2. Read latest inference stats from watch channel
                let inference = *metrics_state.inference_stats.borrow();

                // 3. Collect model statuses
                let model_statuses = metrics_state.collect_model_statuses().await;
                let models_loaded =
                    model_statuses.iter().filter(|m| m.state == "ready").count() as u64;

                // 4. Build unified MetricSample WITH inference fields
                let sample = crate::gpu::MetricSample {
                    ts_unix_ms: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0),
                    cpu_usage_pct: snapshot.cpu_usage_pct,
                    ram_used_mib: snapshot.ram_used_mib,
                    ram_total_mib: snapshot.ram_total_mib,
                    gpu_utilization_pct: snapshot.gpu_utilization_pct,
                    vram: snapshot.vram.clone(),
                    models_loaded,
                    models: model_statuses,
                    tps: inference.as_ref().and_then(|i| i.tps),
                    prompt_tps: inference.as_ref().and_then(|i| i.prompt_tps),
                    cache_hit_pct: inference.as_ref().and_then(|i| i.cache_hit_pct),
                    spec_accept_pct: inference.as_ref().and_then(|i| i.spec_accept_pct),
                    spec_decoding_active: inference
                        .map(|i| i.spec_decoding_active)
                        .unwrap_or(false),
                    inference_last_updated_ms: inference.as_ref().map(|i| i.last_updated_ms),
                };

                // 5. Persist to SQLite (include inference fields in SystemMetricsRow)
                let row = crate::db::queries::SystemMetricsRow {
                    ts_unix_ms: sample.ts_unix_ms,
                    cpu_usage_pct: sample.cpu_usage_pct,
                    ram_used_mib: sample.ram_used_mib as i64,
                    ram_total_mib: sample.ram_total_mib as i64,
                    gpu_utilization_pct: sample.gpu_utilization_pct.map(|v| v as i64),
                    vram_used_mib: sample.vram.as_ref().map(|v| v.used_mib as i64),
                    vram_total_mib: sample.vram.as_ref().map(|v| v.total_mib as i64),
                    models_loaded: sample.models_loaded as i64,
                    tps: sample.tps.map(|v| v as f64),
                    prompt_tps: sample.prompt_tps.map(|v| v as f64),
                    cache_hit_pct: sample.cache_hit_pct.map(|v| v as f64),
                    spec_accept_pct: sample.spec_accept_pct.map(|v| v as f64),
                };
                // Persist (spawn_blocking, unchanged pattern)
                let retention_secs = metrics_state
                    .config
                    .read()
                    .await
                    .proxy
                    .metrics_retention_secs;
                let cutoff_ms = sample.ts_unix_ms - (retention_secs as i128 * 1000) as i64;
                let db_state = Arc::clone(&metrics_state);
                let _ = tokio::task::spawn_blocking(move || {
                    if let Some(conn) = db_state.open_db() {
                        if let Err(e) =
                            crate::db::queries::insert_system_metric(&conn, &row, cutoff_ms)
                        {
                            tracing::warn!("failed to persist system metric: {}", e);
                        }
                    }
                })
                .await;

                // 6. Update in-memory buffer
                history_buf.push_back(sample);
                while history_buf.len() > 450 {
                    history_buf.pop_front();
                }

                // 7. Broadcast as Arc slice (no deep clone)
                let arc: Arc<[crate::gpu::MetricSample]> = history_buf.make_contiguous().into();
                let _ = metrics_state.metrics_tx.send(arc);

                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        });

        Self {
            state,
            idle_timeout_handle: Some(idle_timeout_handle),
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
                        restart_count: 0,
                    },
                );
            } else {
                tracing::warn!(
                    "Orphaned backend process detected: {} (pid {}). Killing.",
                    entry.server_name,
                    pid
                );
                // Use tokio::process::Command to avoid blocking the async context.
                let _ = tokio::process::Command::new("kill")
                    .arg(pid.to_string())
                    .status()
                    .await;
                let _ = crate::db::queries::remove_active_model(&conn, &entry.server_name);
            }
        }
    }

    /// Spawn the idle timeout checker task.
    /// Always spawns — the task reads config each iteration and respects runtime
    /// changes to auto_unload (e.g., via web UI) without requiring a restart.
    /// check_idle_timeouts is always called so Failed backends get cleaned up
    /// even when auto_unload is disabled; the idle-unload logic inside it is
    /// gated on the auto_unload flag.
    fn start_idle_timeout_checker(state: Arc<ProxyState>) -> tokio::task::JoinHandle<()> {
        use std::time::Duration;

        tokio::spawn(async move {
            loop {
                // Re-read config each iteration so runtime changes (e.g., via web UI)
                // take effect without a restart.
                let idle_timeout_secs = state.config.read().await.proxy.idle_timeout_secs;
                let interval = if idle_timeout_secs > 0 {
                    Duration::from_secs((idle_timeout_secs / 2).max(1))
                } else {
                    Duration::from_secs(30)
                };
                tokio::time::sleep(interval).await;
                // Always called — cleans up Failed backends even when auto_unload is off.
                let _ = state.check_idle_timeouts().await;
            }
        })
    }

    /// Convert a `SystemMetricsRow` from SQLite into a `MetricSample`.
    /// Used to seed the in-memory history buffer on startup.
    fn row_into_sample(row: &crate::db::queries::SystemMetricsRow) -> crate::gpu::MetricSample {
        crate::gpu::MetricSample {
            ts_unix_ms: row.ts_unix_ms,
            cpu_usage_pct: row.cpu_usage_pct,
            ram_used_mib: row.ram_used_mib.max(0) as u64,
            ram_total_mib: row.ram_total_mib.max(0) as u64,
            gpu_utilization_pct: row.gpu_utilization_pct.and_then(|v| {
                if (0..=100).contains(&v) {
                    Some(v as u8)
                } else {
                    None
                }
            }),
            vram: row.vram_used_mib.and_then(|used| {
                row.vram_total_mib.map(|total| crate::gpu::VramInfo {
                    used_mib: used.max(0) as u64,
                    total_mib: total.max(0) as u64,
                })
            }),
            models_loaded: row.models_loaded.max(0) as u64,
            models: vec![], // Not stored in DB — seeded samples have no model status
            tps: row.tps.map(|v| v as f32),
            prompt_tps: row.prompt_tps.map(|v| v as f32),
            cache_hit_pct: row.cache_hit_pct.map(|v| v as f32),
            spec_accept_pct: row.spec_accept_pct.map(|v| v as f32),
            spec_decoding_active: false,     // Transient — not in DB
            inference_last_updated_ms: None, // Transient — not in DB
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
        // Clone state for shutdown cleanup (unloads TTS backends)
        let cleanup_state = Arc::clone(&self.state);
        let app = self.into_router();
        let on_shutdown = async move {
            let models = cleanup_state.models.read().await;
            let tts_backends: Vec<String> = models
                .iter()
                .filter(|(_, ms)| ms.is_tts_backend())
                .map(|(name, _)| name.clone())
                .collect();
            drop(models);
            for name in tts_backends {
                if let Err(e) = cleanup_state.unload_tts_backend(&name).await {
                    tracing::warn!("Failed to unload TTS backend '{}': {}", name, e);
                }
            }
        };
        listener::run(app, addr, Some(on_shutdown)).await
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
            "Expected to receive a MetricSample slice within 4s, but timeout occurred"
        );
        let arc = result.unwrap().unwrap();
        assert!(
            !arc.is_empty(),
            "Expected at least one sample in the broadcast"
        );
        let sample = &arc[0];
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
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        // Manually insert a model into model_configs since it's no longer in Config
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
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
                    num_parallel: Some(1),
                    kv_unified: false,
                    profile: None,
                    api_name: None,
                    gpu_layers: None,
                    cache_type_k: None,
                    cache_type_v: None,
                    quants: BTreeMap::new(),
                    modalities: None,
                    display_name: None,
                    db_id: None,
                    ..Default::default()
                },
            );
        }

        // Subscribe BEFORE starting the server so we don't miss the first tick.
        let mut rx = state.metrics_tx.subscribe();

        let _server = ProxyServer::new(state.clone()).await;

        let arc = tokio::time::timeout(std::time::Duration::from_secs(4), rx.recv())
            .await
            .expect("Expected to receive a MetricSample slice within 4s, but timeout occurred")
            .expect("metrics_tx channel closed before any sample was broadcast");

        // The metrics loop must populate `MetricSample.models` from
        // `ProxyState::collect_model_statuses`, which reflects the current
        // configuration.
        assert!(
            !arc.is_empty(),
            "Expected at least one sample in the broadcast"
        );
        let sample = &arc[0];
        assert_eq!(
            sample.models.len(),
            1,
            "Expected exactly one model in sample.models, got: {:?}",
            sample.models
        );
        assert_eq!(sample.models[0].id, "alpha");
        assert_eq!(sample.models[0].backend, "llama_cpp");
        assert!(
            sample.models[0].state != "ready",
            "Expected the configured model to be reported as not ready since no backend was started, got: {:?}",
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
                "http://{}/tama/v1/system/metrics/stream",
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
        let mut found_snapshot = false;
        while let Some(chunk) =
            tokio::time::timeout(std::time::Duration::from_secs(4), stream.next())
                .await
                .unwrap()
        {
            let chunk: Bytes = chunk.unwrap();
            let data = String::from_utf8_lossy(&chunk);
            if data.contains("event: snapshot") {
                // Parse the data: line to extract data: line
                for line in data.lines() {
                    if let Some(data_line) = line.strip_prefix("data: ") {
                        let samples: Vec<crate::gpu::MetricSample> =
                            serde_json::from_str(data_line).unwrap();
                        assert!(!samples.is_empty());
                        assert!(samples[0].ts_unix_ms > 0);
                        assert!(samples[0].ram_total_mib > 0);
                        found_snapshot = true;
                        break;
                    }
                }
                if found_snapshot {
                    break;
                }
            }
        }

        assert!(
            found_snapshot,
            "Expected to receive a snapshot event within 4s, but none was found"
        );
    }

    /// Round-trip test: the SSE `sample` events emitted by
    /// `/tama/v1/system/metrics/stream` must serialize the new
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
        let config = crate::config::Config::default();
        let state = Arc::new(crate::proxy::ProxyState::new(
            config,
            Some(tmp.path().to_path_buf()),
        ));

        // Manually insert a model into model_configs since it's no longer in Config
        {
            let mut mc = state.model_configs.write().await;
            mc.insert(
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
                    num_parallel: Some(1),
                    kv_unified: false,
                    profile: None,
                    api_name: None,
                    gpu_layers: None,
                    cache_type_k: None,
                    cache_type_v: None,
                    quants: BTreeMap::new(),
                    modalities: None,
                    display_name: None,
                    db_id: None,
                    ..Default::default()
                },
            );
        }

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
                "http://{}/tama/v1/system/metrics/stream",
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

                if event_name == Some("snapshot") {
                    let data_line = data_line.expect(
                        "snapshot event must include a data: line carrying the JSON payload",
                    );
                    // The critical assertion: the JSON produced by the
                    // server must deserialize cleanly into Vec<MetricSample>,
                    // including the `models` field.
                    let samples: Vec<crate::gpu::MetricSample> = serde_json::from_str(data_line)
                        .expect(
                        "MetricSample array JSON from SSE stream must deserialize without error",
                    );
                    assert!(!samples.is_empty());
                    parsed_sample = Some(samples[0].clone());
                    break;
                }
            }

            if parsed_sample.is_some() {
                break;
            }
        }

        let sample = parsed_sample
            .expect("Expected to receive a snapshot event within 4s, but none was found");

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
            models[0].state != "ready",
            "Expected the configured model to be reported as not ready since no backend was started, got: {:?}",
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
        let model_configs = state.model_configs.read().await;
        assert!(
            model_configs.contains_key("db-model-key"),
            "Expected model 'db-model-key' to be loaded from DB"
        );
        let model = model_configs.get("db-model-key").unwrap();
        assert_eq!(model.display_name.as_deref(), Some("DB Model"));
    }
}
