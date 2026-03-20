pub mod server;

use crate::backends::registry::BackendRegistry;
use crate::config::ProxyConfig;
use crate::models::card::ModelCard;
use anyhow::{Context, Result};
use reqwest::Client;
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

/// State for a model backend.
#[derive(Debug, Clone)]
pub enum ModelState {
    /// Backend is starting up (placeholder during initialization)
    Starting {
        model_name: String,
        backend: String,
        backend_url: String,
        last_accessed: SystemTime,
        consecutive_failures: Arc<AtomicU32>,
    },
    /// Backend is ready and accepting traffic
    Ready {
        model_name: String,
        backend: String,
        backend_pid: u32,
        backend_url: String,
        load_time: SystemTime,
        last_accessed: SystemTime,
        consecutive_failures: Arc<AtomicU32>,
    },
    /// Backend failed to start
    Failed {
        model_name: String,
        backend: String,
        error: String,
    },
}

impl ModelState {
    pub fn model_name(&self) -> &str {
        match self {
            ModelState::Starting { model_name, .. } => model_name,
            ModelState::Ready { model_name, .. } => model_name,
            ModelState::Failed { model_name, .. } => model_name,
        }
    }

    pub fn backend(&self) -> &str {
        match self {
            ModelState::Starting { backend, .. } => backend,
            ModelState::Ready { backend, .. } => backend,
            ModelState::Failed { backend, .. } => backend,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, ModelState::Ready { .. })
    }

    pub fn backend_pid(&self) -> Option<u32> {
        match self {
            ModelState::Ready { backend_pid, .. } => Some(*backend_pid),
            _ => None,
        }
    }

    pub fn consecutive_failures(&self) -> Option<&Arc<AtomicU32>> {
        match self {
            ModelState::Starting {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            ModelState::Ready {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            ModelState::Failed { .. } => None,
        }
    }

    pub fn load_time(&self) -> Option<SystemTime> {
        match self {
            ModelState::Ready { load_time, .. } => Some(*load_time),
            _ => None,
        }
    }

    pub fn last_accessed(&self) -> SystemTime {
        match self {
            ModelState::Ready { last_accessed, .. } => *last_accessed,
            ModelState::Starting { last_accessed, .. } => *last_accessed,
            ModelState::Failed { .. } => SystemTime::now(),
        }
    }
}

/// Metrics for the proxy server.
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    pub total_requests: AtomicU64,
    pub successful_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub models_loaded: AtomicU64,
    pub models_unloaded: AtomicU64,
}

/// Manages proxy state and model lifecycle.
pub struct ProxyState {
    pub config: ProxyConfig,
    pub models: Arc<RwLock<HashMap<String, ModelState>>>,
    pub registry: Arc<RwLock<BackendRegistry>>,
    pub config_data: Arc<RwLock<crate::config::Config>>,
    pub process_map: Arc<Mutex<HashMap<u32, String>>>,
    pub process_handles: Arc<Mutex<HashMap<u32, tokio::process::Child>>>,
    pub client: Arc<Client>,
    pub metrics: Arc<ProxyMetrics>,
}

impl ProxyState {
    pub fn new(
        config: ProxyConfig,
        registry: BackendRegistry,
        config_data: crate::config::Config,
    ) -> Self {
        Self {
            config,
            models: Arc::new(RwLock::new(HashMap::new())),
            registry: Arc::new(RwLock::new(registry)),
            config_data: Arc::new(RwLock::new(config_data)),
            process_map: Arc::new(Mutex::new(HashMap::new())),
            process_handles: Arc::new(Mutex::new(HashMap::new())),
            client: Arc::new(Client::new()),
            metrics: Arc::new(ProxyMetrics::default()),
        }
    }

    /// Get the backend URL for a server name.
    pub async fn get_backend_url(&self, server_name: &str) -> Result<String> {
        let config = self.config_data.read().await;
        let server = config
            .servers
            .get(server_name)
            .with_context(|| format!("Server '{}' not found", server_name))?;

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
    ) -> Option<(ModelState, SystemTime)> {
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
        self.models
            .read()
            .await
            .get(server_name)
            .and_then(|s| s.consecutive_failures().map(|f| f.load(Ordering::Relaxed)))
    }

    /// Find an available loaded server for a given model name.
    pub async fn get_available_server_for_model(&self, model_name: &str) -> Option<String> {
        let config = self.config_data.read().await;
        let servers = config.resolve_servers_for_model(model_name);

        let models = self.models.read().await;

        // Simple round-robin or first available
        // For simplicity, we just pick the first one that is loaded and hasn't tripped the circuit breaker
        for (server_name, _, _) in servers {
            if let Some(state) = models.get(&server_name) {
                if state.is_ready()
                    && state
                        .consecutive_failures()
                        .map(|f| f.load(Ordering::Relaxed))
                        .unwrap_or(0)
                        < self.config.circuit_breaker_threshold
                {
                    return Some(server_name);
                }
            }
        }

        None
    }

    /// Load a model by starting its backend process.
    pub async fn load_model(
        &self,
        model_name: &str,
        _model_card: Option<&ModelCard>,
    ) -> Result<String> {
        debug!("Loading model: {}", model_name);

        let config = self.config_data.read().await;

        // Find a server that provides this model
        let server_name = match self.get_available_server_for_model(model_name).await {
            Some(name) => name,
            None => {
                return Err(anyhow::anyhow!(
                    "Failed to resolve server for model {}",
                    model_name
                ));
            }
        };

        // Check if the server is already loaded and ready - if so, just use it
        {
            let models = self.models.read().await;
            if let Some(state) = models.get(&server_name) {
                if state.is_ready() {
                    debug!("Server '{}' already loaded for model '{}'", server_name, model_name);
                    drop(models);
                    drop(config);
                    return Ok(server_name);
                }
            }
        }

        // Get server and backend config from config
        let (server_config, backend_config) = match config.resolve_server(&server_name) {
            Ok(sc) => sc,
            Err(e) => {
                return Err(e);
            }
        };

        // Reserve a server immediately to prevent race conditions
        {
            let mut models = self.models.write().await;
            for (server_name, _, _) in config.resolve_servers_for_model(model_name) {
                if !models.contains_key(&server_name) {
                    // Reserve this server with Starting state
                    models.insert(
                        server_name.clone(),
                        ModelState::Starting {
                            model_name: model_name.to_string(),
                            backend: server_config.backend.clone(),
                            backend_url: String::new(),
                            last_accessed: SystemTime::now(),
                            consecutive_failures: Arc::new(AtomicU32::new(0)),
                        },
                    );
                    break;
                }
            }
        }

        let backend_path = backend_config.path.clone();

        let args = config.build_args(server_config, backend_config);
        let health_url = config
            .resolve_health_url(server_config)
            .with_context(|| format!("No health URL resolved for server: {}", server_name))?;
        let backend_url = config
            .resolve_backend_url(server_config)
            .with_context(|| format!("No health URL resolved for server: {}", server_name))?;

        info!(
            "Starting backend '{}' for server '{}' (model '{}')",
            server_config.backend, server_name, model_name
        );

        let child = tokio::process::Command::new(&backend_path)
            .args(&args)
            .env("MODEL_NAME", model_name)
            .spawn()
            .with_context(|| format!("Failed to start backend '{}'", server_config.backend))?;

        let pid = child.id().unwrap();
        info!(
            "Backend '{}' started for server '{}' (pid: {:?})",
            server_config.backend, server_name, pid
        );

        // Register PID and Child handle in process maps
        {
            let mut processes = self.process_map.lock().await;
            processes.insert(pid, server_name.clone());
            let mut handles = self.process_handles.lock().await;
            handles.insert(pid, child);
        }

        // Wait for health check to pass
        let timeout = Duration::from_secs(30);
        let start = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                break;
            }

            if let Ok(response) = check_health(&health_url, Some(30)).await {
                if response.status().is_success() {
                    debug!("Health check passed for server: {}", server_name);
                    break;
                }
            }
        }

        if start.elapsed() >= timeout {
// Remove Child handle and wait
        let child = self.process_handles.lock().await.remove(&pid);
        if let Some(mut child) = child {
            tokio::task::spawn_blocking(move || {
                std::mem::drop(child.wait());
            })
            .await
            .unwrap();
        }
            let mut processes = self.process_map.lock().await;
            processes.remove(&pid);

            // Remove from models (cleanup failed reservation)
            {
                let mut models = self.models.write().await;
                models.remove(&server_name);
            }

            return Err(anyhow::anyhow!(
                "Backend '{}' failed to start for server '{}' (timeout after {}s)",
                server_config.backend,
                server_name,
                timeout.as_secs()
            ));
        }

        // Update the loaded model state to Ready
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut(&server_name) {
                if let ModelState::Starting { .. } = state {
                    *state = ModelState::Ready {
                        model_name: model_name.to_string(),
                        backend: server_config.backend.clone(),
                        backend_pid: pid,
                        backend_url,
                        load_time: SystemTime::now(),
                        last_accessed: SystemTime::now(),
                        consecutive_failures: Arc::new(AtomicU32::new(0)),
                    };
                }
            }
        }

        info!("Server '{}' loaded successfully", server_name);
        self.metrics.models_loaded.fetch_add(1, Ordering::Relaxed);
        Ok(server_name)
    }

    /// Unload a server by stopping its backend process.
    pub async fn unload_model(&self, server_name: &str) -> Result<()> {
        debug!("Unloading server: {}", server_name);

        let state = self
            .get_model_state(server_name)
            .await
            .with_context(|| format!("Server '{}' not loaded", server_name))?;

        if !state.is_ready() {
            return Err(anyhow::anyhow!(
                "Server '{}' is not ready (state: {:?})",
                server_name,
                state
            ));
        }

        let backend_name = state.backend().to_string();
        let pid = state
            .backend_pid()
            .with_context(|| format!("No backend PID for server: {}", server_name))?;

        info!(
            "Stopping backend '{}' for server '{}'",
            backend_name, server_name
        );

        // Kill the process if we have the PID
        info!("Sending SIGTERM to backend process {}", pid);
        let _ = kill_process(pid).await;

        // Wait up to 5 seconds for graceful shutdown
        tokio::time::sleep(Duration::from_secs(5)).await;

        // Remove Child handle and wait
        let child = self.process_handles.lock().await.remove(&pid);
        if let Some(mut child) = child {
            tokio::task::spawn_blocking(move || {
                std::mem::drop(child.wait());
            })
            .await
            .unwrap();
        }
        let mut processes = self.process_map.lock().await;
        processes.remove(&pid);

        // Remove from models
        let mut models = self.models.write().await;
        models.remove(server_name);

        info!("Server '{}' unloaded", server_name);
        self.metrics.models_unloaded.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Check if any server has been idle for longer than the timeout.
    pub async fn check_idle_timeouts(&self) -> Vec<String> {
        let now = SystemTime::now();
        let mut to_unload = Vec::new();

        let models = self.models.read().await;
        for (server_name, state) in models.iter() {
            let idle_duration = now
                .duration_since(state.last_accessed())
                .unwrap_or(Duration::ZERO);
            let timeout = Duration::from_secs(self.config.idle_timeout_secs);

            if idle_duration > timeout {
                warn!(
                    "Server '{}' has been idle for {}s (timeout: {}s)",
                    server_name,
                    idle_duration.as_secs(),
                    self.config.idle_timeout_secs
                );
                to_unload.push(server_name.clone());
            }
        }

        drop(models);

        // Actually unload the models
        for server_name in &to_unload {
            let _ = self.unload_model(server_name).await;
        }

        to_unload
    }

    /// Update the last accessed time for a server.
    pub async fn update_last_accessed(&self, server_name: &str) {
        let mut models = self.models.write().await;
        if let Some(state) = models.get_mut(server_name) {
            match state {
                ModelState::Starting { last_accessed, .. } => {
                    *last_accessed = SystemTime::now();
                }
                ModelState::Ready { last_accessed, .. } => {
                    *last_accessed = SystemTime::now();
                }
                ModelState::Failed { .. } => {}
            }
        }
    }

    /// Get the model card for a model name.
    pub async fn get_model_card(&self, model_name: &str) -> Option<crate::models::card::ModelCard> {
        let configs_dir = self.config_data.read().await.configs_dir().ok()?;

        // Try to find the model card file
        // Format: configs.d/<company>--<model>.toml
        let (org, name) = model_name.split_once('/').unwrap_or(("", model_name));
        let card_path = configs_dir.join(format!("{}--{}.toml", org, name));

        if card_path.exists() {
            let content = std::fs::read_to_string(&card_path).ok()?;
            let card: crate::models::card::ModelCard = toml::from_str(&content).ok()?;
            Some(card)
        } else {
            None
        }
    }
}

/// Kill a process by PID (cross-platform).
async fn kill_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .spawn();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .spawn();
    }
    Ok(())
}

/// Check the health of a backend by making a request to its health endpoint.
async fn check_health(url: &str, timeout: Option<u64>) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout.unwrap_or(10)))
        .build()?;
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to check health: {}", url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_is_model_loaded() {
        let config = ProxyConfig::default();
        let registry = BackendRegistry::default();
        let config_data = crate::config::Config::default();
        let state = ProxyState::new(config, registry, config_data);

        assert!(!state.is_model_loaded("test-model").await);
    }

    #[tokio::test]
    async fn test_get_model_state() {
        let config = ProxyConfig::default();
        let registry = BackendRegistry::default();
        let config_data = crate::config::Config::default();
        let state = ProxyState::new(config, registry, config_data);

        assert!(state.get_model_state("test-model").await.is_none());
    }

    #[tokio::test]
    async fn test_get_model_card() {
        use tempfile::TempDir;

        let config = ProxyConfig::default();
        let registry = BackendRegistry::default();

        // Create a temporary directory for configs to make test hermetic
        let temp_dir = TempDir::new().unwrap();
        let config_data = crate::config::Config {
            general: crate::config::General {
                log_level: "info".to_string(),
                models_dir: None,
                logs_dir: None,
            },
            backends: HashMap::new(),
            servers: HashMap::new(),
            supervisor: crate::config::Supervisor {
                restart_policy: "always".to_string(),
                max_restarts: 0,
                restart_delay_ms: 0,
                health_check_interval_ms: 0,
            },
            custom_profiles: None,
            proxy: config.clone(),
            loaded_from: Some(temp_dir.path().to_path_buf()),
        };
        let state = ProxyState::new(config, registry, config_data);

        let card = state.get_model_card("test-model").await;
        assert!(card.is_none());
    }
}
