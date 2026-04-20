use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::download_queue::{queue_processor_loop, DownloadQueueService};
use super::types::{ModelState, ProxyMetrics, ProxyState};

impl ProxyState {
    pub fn new(config: crate::config::Config, db_dir: Option<std::path::PathBuf>) -> Self {
        let (metrics_tx, _) = tokio::sync::broadcast::channel(64);

        // Initialize download queue service if db_dir is configured.
        let poll_interval = config.proxy.download_queue_poll_interval_secs;
        let download_queue = db_dir
            .as_ref()
            .map(|dir| Arc::new(DownloadQueueService::new(Some(dir.clone()), poll_interval)));

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

        let backend_url = config
            .resolve_backend_url(server)
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
        let conn = self
            .open_db()
            .with_context(|| "Database directory not configured")?;
        let configs = crate::db::load_model_configs(&conn)?;
        let mut model_configs = self.model_configs.write().await;
        *model_configs = configs;
        Ok(())
    }
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
}
