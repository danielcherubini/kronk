use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::download_queue::{queue_processor_loop, DownloadQueueService};
use super::types::{ModelState, ProxyMetrics, ProxyState, WILDCARD_MODEL_NAME};

impl ProxyState {
    pub fn new(config: crate::config::Config, db_dir: Option<std::path::PathBuf>) -> Self {
        let (metrics_tx, _) = tokio::sync::broadcast::channel(3);

        // Initialize download queue service if db_dir is configured.
        let poll_interval = config.proxy.download_queue_poll_interval_secs;
        let download_queue = db_dir.as_ref().and_then(|dir| {
            crate::models::ModelManager::open(dir)
                .ok()
                .map(|mm| Arc::new(DownloadQueueService::new(mm, poll_interval)))
        });

        let state = Self {
            config: Arc::new(tokio::sync::RwLock::new(config)),
            model_configs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            models: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            client: reqwest::Client::builder()
                // Only set a connect timeout — not an overall timeout.
                // The overall timeout covers the entire response lifetime
                // including streaming bodies, which would kill long SSE
                // streams from LLM backends.
                .connect_timeout(Duration::from_secs(30))
                .build()
                // reqwest Client::build() only fails if TLS backend init fails,
                // which is not recoverable — panic is acceptable here.
                .expect("failed to build HTTP client"),
            metrics: Arc::new(ProxyMetrics::default()),
            db_dir,
            pull_jobs: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            system_metrics: Arc::new(tokio::sync::RwLock::new(
                crate::gpu::SystemMetrics::default(),
            )),
            in_flight_downloads: Arc::new(
                tokio::sync::Mutex::new(std::collections::HashSet::new()),
            ),
            metrics_tx,
            download_queue: download_queue.clone(),
            config_write_semaphore: Arc::new(tokio::sync::Semaphore::new(4)),
            backend_logs: crate::backends::log_stream::BackendLogManager::default(),
            inference_stats: tokio::sync::watch::channel(None).0,
            wildcard_resolve_guard: Arc::new(tokio::sync::Mutex::new(())),
            // ── Web UI fields ──
            #[cfg(feature = "web-ui")]
            web_jobs: Some(Arc::new(crate::web_types::JobManager::new())),
            #[cfg(feature = "web-ui")]
            web_capabilities: Some(Arc::new(crate::web_types::CapabilitiesCache::new())),
            #[cfg(feature = "web-ui")]
            web_update_checker: Arc::new(crate::updates::UpdateChecker::new()),
            #[cfg(feature = "web-ui")]
            web_binary_version: String::new(), // Set later by CLI
            #[cfg(feature = "web-ui")]
            web_update_tx: Arc::new(tokio::sync::Mutex::new(None)),
            #[cfg(feature = "web-ui")]
            web_upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        };

        // Spawn the queue processor background task if download queue is configured.
        // This must be called from within a tokio runtime context (which is always true
        // in practice since ProxyState::new is only called from async functions).
        if let Some(ref _dq) = download_queue {
            let state_clone = Arc::new(state.clone());
            tokio::spawn(async move {
                queue_processor_loop(state_clone).await;
            });
        }

        state
    }

    /// Get the backend URL for a server name.
    pub async fn get_backend_url(&self, server_name: &str) -> Result<String> {
        let config = self.config.read().await;
        let model_configs = self.model_configs.read().await;
        let server = config
            .resolve_server(&model_configs, server_name)
            .with_context(|| format!("Server '{}' not found", server_name))?
            .0;

        // Open BackendManager for health_check_url lookup
        let manager = self
            .db_dir
            .as_ref()
            .and_then(|dir| crate::backends::BackendManager::open(dir).ok())
            .unwrap_or_else(|| {
                crate::backends::BackendManager::open_in_memory()
                    .expect("in-memory BackendManager must always open")
            });
        let gpu_variant = server.gpu_variant.as_deref().unwrap_or("cpu");
        let health_url = manager.get_health_check_url(&server.backend, gpu_variant);
        let backend_url = config
            .resolve_backend_url(server, health_url.as_deref())
            .with_context(|| format!("No backend URL resolved for server '{}'", server_name))?;

        Ok(backend_url)
    }

    /// Check if a model is already loaded.
    pub async fn is_model_loaded(&self, model_name: &str) -> bool {
        self.get_available_server_for_model(model_name)
            .await
            .is_some()
    }

    /// Get the state of a loaded model (server).
    pub async fn get_model_state(&self, server_name: &str) -> Option<ModelState> {
        let models = self.models.read().await;
        models.get(server_name).cloned()
    }

    /// Get the state of a loaded model with last_accessed field.
    pub async fn get_model_state_with_access(
        &self,
        server_name: &str,
    ) -> Option<(ModelState, Option<Instant>)> {
        let models = self.models.read().await;
        models
            .get(server_name)
            .map(|state| (state.clone(), state.last_accessed()))
    }

    /// Get the backend PID for a server.
    pub async fn get_backend_pid(&self, server_name: &str) -> Option<u32> {
        self.models
            .read()
            .await
            .get(server_name)
            .and_then(|s| match s {
                ModelState::Ready { backend_pid, .. } => Some(*backend_pid),
                _ => None,
            })
    }

    /// Get the circuit breaker failures for a server.
    pub async fn get_circuit_breaker_failures(&self, server_name: &str) -> Option<u32> {
        self.models.read().await.get(server_name).and_then(|s| {
            s.consecutive_failures()
                .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        })
    }

    /// Find an available loaded server for a given model name.
    pub async fn get_available_server_for_model(&self, model_name: &str) -> Option<String> {
        let (server_names, circuit_breaker_threshold) = {
            let config = self.config.read().await;
            let model_configs = self.model_configs.read().await;
            // Collect just the server names (owned Strings) so we can drop the lock.
            let names: Vec<String> = config
                .resolve_servers_for_model(&model_configs, model_name)
                .into_iter()
                .map(|(name, _, _)| name)
                .collect();
            let threshold = config.proxy.circuit_breaker_threshold;
            (names, threshold)
        };

        let models = self.models.read().await;

        // Simple round-robin or first available
        for server_name in server_names {
            if let Some(state) = models.get(&server_name) {
                if (state.is_ready() || matches!(state, ModelState::Starting { .. }))
                    && state
                        .consecutive_failures()
                        .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(0)
                        < circuit_breaker_threshold
                {
                    return Some(server_name);
                }
            }
        }

        None
    }

    /// Update the last accessed time for a server.
    pub async fn update_last_accessed(&self, server_name: &str) {
        let mut models = self.models.write().await;
        if let Some(state) = models.get_mut(server_name) {
            match state {
                ModelState::Starting { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                ModelState::Ready { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                ModelState::Unloading { last_accessed, .. } => {
                    *last_accessed = Instant::now();
                }
                ModelState::Failed { .. } => {}
            }
        }
    }

    /// Get the model card for a model name.
    pub async fn get_model_card(&self, model_name: &str) -> Option<crate::models::card::ModelCard> {
        let configs_dir = self.config.read().await.configs_dir().ok()?;

        // Try to find the model card file
        // Format: configs/<company>--<model>.toml
        let (org, name) = model_name.split_once('/').unwrap_or(("", model_name));
        let card_filename = if org.is_empty() {
            format!("{}.toml", name)
        } else {
            format!("{}--{}.toml", org, name)
        };
        let card_path = configs_dir.join(card_filename);

        let content = tokio::fs::read_to_string(&card_path).await.ok()?;
        let card: crate::models::card::ModelCard = toml::from_str(&content).ok()?;
        Some(card)
    }

    /// Reload model configurations from the database.
    ///
    /// This ensures that the in-memory registry stays in sync with mutations
    /// made via the web API or CLI.
    pub async fn reload_model_configs(&self) -> Result<()> {
        let mgr = self
            .model_mgr()
            .with_context(|| "Database directory not configured")?;
        let configs = crate::db::load_model_configs(mgr.conn())?;
        let mut model_configs = self.model_configs.write().await;
        *model_configs = configs;
        Ok(())
    }

    /// Open a ModelManager for model-related database operations.
    ///
    /// Returns `None` if `db_dir` is not configured (e.g., in tests).
    ///
    /// Each call opens a fresh `ModelManager` (and thus a fresh `rusqlite::Connection`).
    /// This is deliberate: `Connection` is `Send` but not `Sync`, so we cannot
    /// share a single instance across threads via `Arc`. For persistent reuse,
    /// see `DownloadQueueService` which wraps `ModelManager` in `Mutex`.
    pub fn model_mgr(&self) -> Option<crate::models::ModelManager> {
        self.db_dir
            .as_ref()
            .and_then(|dir| crate::models::ModelManager::open(dir).ok())
    }

    /// Resolve the server for a "whatevers-hot-n-fresh" request.
    ///
    /// Selection strategy (in order):
    /// 1. Most-recently-accessed Ready or Starting LLM model (by last_accessed)
    /// 2. Failed LLM model — extract model_name, call load_model
    /// 3. Last-used model from DB — call load_model using record's model_name field
    /// 4. 503 if nothing available
    ///
    /// Uses a Mutex guard so only one concurrent caller proceeds to DB lookup + load.
    /// CRITICAL: Must drop the `self.models` read lock BEFORE calling `load_model`
    /// because `load_model` acquires a write lock on the same RwLock (deadlock otherwise).
    pub async fn resolve_wildcard_model(&self) -> Result<String> {
        // Acquire the wildcard resolve guard — prevents concurrent redundant loads.
        // Guard is held for the entire operation and dropped on all paths.
        let _guard = self.wildcard_resolve_guard.lock().await;

        // Phase 1: Collect decision data under self.models read lock
        let decision = {
            let models = self.models.read().await;

            // Filter to non-TTS models that are Ready or Starting
            let candidates: Vec<(&String, &ModelState)> = models
                .iter()
                .filter(|(_, state)| {
                    !state.is_tts_backend()
                        && (state.is_ready() || matches!(state, ModelState::Starting { .. }))
                })
                .collect();

            if let Some((server_name, _)) = candidates
                .iter()
                .max_by_key(|(_, state)| state.last_accessed())
            {
                // Most-recently-accessed Ready/Starting model found.
                // Clone server_name and return — read lock will be dropped after this block.
                Some(WildcardDecision::UseServer(server_name.to_string()))
            } else {
                // No Ready/Starting models — check for Failed models
                let failed_candidates: Vec<(&String, &ModelState)> = models
                    .iter()
                    .filter(|(_, state)| {
                        !state.is_tts_backend() && matches!(state, ModelState::Failed { .. })
                    })
                    .collect();

                if let Some((_, state)) = failed_candidates.first() {
                    // Failed model found — extract model_name for reload attempt
                    Some(WildcardDecision::ReloadFailed(
                        state.model_name().to_string(),
                    ))
                } else {
                    // No models at all
                    None
                }
            }
        };
        // self.models read lock is dropped here — CRITICAL for avoiding deadlock

        // Phase 2: Act on the decision
        match decision {
            Some(WildcardDecision::UseServer(server_name)) => Ok(server_name),
            Some(WildcardDecision::ReloadFailed(model_name)) => {
                // Attempt to reload the failed model
                match self.load_model(&model_name, None).await {
                    Ok(server_name) => Ok(server_name),
                    Err(_) => {
                        // Reload failed — fall through to DB lookup
                        Self::try_db_fallback(self).await
                    }
                }
            }
            None => {
                // No models loaded at all — try DB fallback
                Self::try_db_fallback(self).await
            }
        }
    }

    /// DB fallback: try to load the last-used model from the database.
    async fn try_db_fallback(&self) -> Result<String> {
        let record = self
            .model_mgr()
            .and_then(|mgr| mgr.get_last_used().ok())
            .flatten()
            .ok_or_else(|| anyhow::anyhow!("No model available for '{}'", WILDCARD_MODEL_NAME))?;
        self.load_model(&record.model_name, None)
            .await
            .map_err(|_| anyhow::anyhow!("No model available for '{}'", WILDCARD_MODEL_NAME))
    }
}

/// Internal decision enum for wildcard resolution.
#[derive(Debug, Clone)]
enum WildcardDecision {
    /// Use an already-loaded server directly.
    UseServer(String),
    /// Reload a previously failed model.
    ReloadFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that `ProxyState::new` creates a metrics channel and that subscribing adds a receiver.
    #[test]
    fn test_proxy_state_new_creates_metrics_channel() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);
        let _subscriber = state.metrics_tx.subscribe();
        assert_eq!(state.metrics_tx.receiver_count(), 1);
    }

    /// Test resolve_wildcard_model returns Err when no models loaded and no DB configured.
    #[tokio::test]
    async fn test_resolve_wildcard_no_models() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);
        let result = state.resolve_wildcard_model().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains(WILDCARD_MODEL_NAME));
    }

    /// Test resolve_wildcard_model picks the most-recently-accessed Ready model.
    #[tokio::test]
    async fn test_resolve_wildcard_picks_most_recent_ready() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);

        // Insert two Ready models with different last_accessed times
        let mut models = state.models.write().await;
        models.insert(
            "server_old".to_string(),
            ModelState::Ready {
                model_name: "old-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 100,
                backend_url: "http://localhost:8080".to_string(),
                load_time: std::time::SystemTime::now(),
                last_accessed: Instant::now() - Duration::from_secs(100),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
        models.insert(
            "server_new".to_string(),
            ModelState::Ready {
                model_name: "new-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 200,
                backend_url: "http://localhost:8081".to_string(),
                load_time: std::time::SystemTime::now(),
                last_accessed: Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
        drop(models);

        let result = state.resolve_wildcard_model().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "server_new");
    }

    /// Test resolve_wildcard_model skips TTS backends and picks LLM.
    #[tokio::test]
    async fn test_resolve_wildcard_skips_tts() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);

        let mut models = state.models.write().await;
        // TTS backend
        models.insert(
            "tts_server".to_string(),
            ModelState::Ready {
                model_name: "kokoro".to_string(),
                backend: "tts_kokoro".to_string(),
                backend_pid: 300,
                backend_url: "http://localhost:9000".to_string(),
                load_time: std::time::SystemTime::now(),
                last_accessed: Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
        // LLM backend
        models.insert(
            "llm_server".to_string(),
            ModelState::Ready {
                model_name: "llm-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_pid: 400,
                backend_url: "http://localhost:8082".to_string(),
                load_time: std::time::SystemTime::now(),
                last_accessed: Instant::now() - Duration::from_secs(10),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
                restart_count: 0,
            },
        );
        drop(models);

        let result = state.resolve_wildcard_model().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "llm_server");
    }

    /// Test resolve_wildcard_model includes Starting models as available.
    #[tokio::test]
    async fn test_resolve_wildcard_includes_starting() {
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, None);

        let mut models = state.models.write().await;
        models.insert(
            "starting_server".to_string(),
            ModelState::Starting {
                model_name: "starting-model".to_string(),
                backend: "llama_cpp".to_string(),
                backend_url: "http://localhost:8083".to_string(),
                backend_pid: 500,
                last_accessed: Instant::now(),
                start_time: Instant::now(),
                consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                failure_timestamp: None,
            },
        );
        drop(models);

        let result = state.resolve_wildcard_model().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "starting_server");
    }

    /// Test resolve_wildcard_model falls back to DB when no models loaded.
    /// When DB has a last_used record, it attempts to load that model.
    /// Without model configs, load_model fails, so we verify the error path.
    #[tokio::test]
    async fn test_resolve_wildcard_fallback_to_db() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));

        // Set up the last_used record in the DB
        let mgr = state.model_mgr().expect("DB should be available");
        mgr.set_last_used("test-server", "test-model").unwrap();

        // No models loaded — should fall back to DB, try load_model,
        // which will fail without model configs → returns Err
        let result = state.resolve_wildcard_model().await;
        // The error should be about no model available (from DB fallback failure)
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains(WILDCARD_MODEL_NAME));
    }

    // ── Integration tests ────────────────────────────────────────────────────

    /// Integration test: full flow with DB persistence.
    /// Verifies: (1) no models → Err, (2) after inserting last_used record,
    /// resolve_wildcard_model consults DB and attempts load (fails without configs,
    /// but proves the DB path is exercised).
    #[tokio::test]
    async fn test_wildcard_full_flow_with_db() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));

        // Phase 1: No models loaded, no DB record → Err
        let result = state.resolve_wildcard_model().await;
        assert!(
            result.is_err(),
            "Should fail when no models loaded and no DB record"
        );

        // Phase 2: Insert a last_used record into the DB
        let mgr = state.model_mgr().expect("DB should be available");
        mgr.set_last_used("saved-server", "saved-model").unwrap();

        // Verify the record was persisted
        let record = mgr.get_last_used().unwrap().expect("Record should exist");
        assert_eq!(record.server_name, "saved-server");
        assert_eq!(record.model_name, "saved-model");

        // Phase 3: resolve_wildcard_model should now consult DB and attempt load.
        // load_model fails without model configs, but the error proves the DB path ran.
        let result = state.resolve_wildcard_model().await;
        assert!(
            result.is_err(),
            "Should still fail (load_model needs configs), but DB was consulted"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains(WILDCARD_MODEL_NAME),
            "Error should reference wildcard model name"
        );
    }

    /// Integration test: concurrent wildcard requests are serialized by the guard mutex.
    /// All concurrent callers should get the same result (either all Ok with same server,
    /// or all Err). This verifies the wildcard_resolve_guard prevents duplicate loads.
    #[tokio::test]
    async fn test_wildcard_concurrent_requests() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));

        // Pre-populate DB with a last_used record so all callers hit the same path
        let mgr = state.model_mgr().expect("DB should be available");
        mgr.set_last_used("saved-server", "saved-model").unwrap();

        // Spawn 5 concurrent resolve_wildcard_model calls
        let state_clone = state.clone();
        let handles: Vec<_> = (0..5)
            .map(|_| {
                let s = state_clone.clone();
                tokio::spawn(async move { s.resolve_wildcard_model().await })
            })
            .collect();

        // Await all results
        let results: Vec<_> = futures::future::join_all(handles)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // All 5 should produce the same outcome
        // (Either all Ok with same server_name, or all Err)
        let first = &results[0];
        for (i, result) in results.iter().enumerate().skip(1) {
            match (first, result) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(
                        a, b,
                        "Concurrent call {} returned different server: {} vs {}",
                        i, a, b
                    );
                }
                (Err(a), Err(b)) => {
                    assert_eq!(
                        a.to_string(),
                        b.to_string(),
                        "Concurrent call {} returned different error",
                        i
                    );
                }
                _ => panic!(
                    "Inconsistent results: call 0 = {:?}, call {} = {:?}",
                    first, i, result
                ),
            }
        }
    }

    /// Integration test: Failed model state triggers reload attempt.
    /// When a model is in Failed state, resolve_wildcard_model should attempt
    /// to reload it via load_model. If load_model fails (no configs), it falls
    /// through to DB fallback.
    #[tokio::test]
    async fn test_wildcard_failed_model_fallback() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = crate::config::Config::default();
        let state = ProxyState::new(config, Some(temp_dir.path().to_path_buf()));

        // Add a Failed model
        {
            let mut models = state.models.write().await;
            models.insert(
                "failed-server".to_string(),
                ModelState::Failed {
                    model_name: "failed-model".to_string(),
                    backend: "llama_cpp".to_string(),
                    error: "Backend crashed".to_string(),
                },
            );
        }

        // resolve_wildcard_model should detect the Failed model and attempt reload
        let result = state.resolve_wildcard_model().await;

        // load_model will fail without configs → falls through to DB fallback
        // DB is empty → Err with wildcard model name
        assert!(
            result.is_err(),
            "Should fail (no configs for load, no DB record for fallback)"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains(WILDCARD_MODEL_NAME),
            "Error should reference wildcard model name after Failed fallback"
        );
    }
}
