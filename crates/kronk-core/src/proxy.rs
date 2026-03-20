pub mod server;

use crate::config::Config;
use anyhow::{Context, Result};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command as TokioCommand;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// State for a model backend.
#[derive(Debug, Clone)]
pub enum ModelState {
    /// Backend is starting up (placeholder during initialization)
    Starting {
        model_name: String,
        backend: String,
        backend_url: String,
        last_accessed: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
    },
    /// Backend is ready and accepting traffic
    Ready {
        model_name: String,
        backend: String,
        backend_pid: u32,
        backend_url: String,
        load_time: std::time::SystemTime,
        last_accessed: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
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

    pub fn consecutive_failures(&self) -> Option<&Arc<std::sync::atomic::AtomicU32>> {
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

    pub fn load_time(&self) -> Option<std::time::SystemTime> {
        match self {
            ModelState::Ready { load_time, .. } => Some(*load_time),
            _ => None,
        }
    }

    pub fn last_accessed(&self) -> Option<Instant> {
        match self {
            ModelState::Ready { last_accessed, .. } => Some(*last_accessed),
            ModelState::Starting { last_accessed, .. } => Some(*last_accessed),
            ModelState::Failed { .. } => None,
        }
    }

    /// Check if the server has failed and the cooldown has elapsed.
    pub fn can_reload(&self, cooldown_seconds: u64) -> bool {
        match self {
            ModelState::Failed { .. } => false,
            ModelState::Starting {
                failure_timestamp, ..
            }
            | ModelState::Ready {
                failure_timestamp, ..
            } => failure_timestamp
                .map(|ts| {
                    std::time::SystemTime::now()
                        .duration_since(ts)
                        .map(|d| d.as_secs() >= cooldown_seconds)
                        .unwrap_or(false)
                })
                .unwrap_or(true),
        }
    }
}

/// Metrics for the proxy server.
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    pub total_requests: std::sync::atomic::AtomicU64,
    pub successful_requests: std::sync::atomic::AtomicU64,
    pub failed_requests: std::sync::atomic::AtomicU64,
    pub models_loaded: std::sync::atomic::AtomicU64,
    pub models_unloaded: std::sync::atomic::AtomicU64,
}

/// Manages proxy state and model lifecycle.
#[derive(Clone)]
pub struct ProxyState {
    pub config: Config,
    pub models: Arc<RwLock<HashMap<String, ModelState>>>,
    pub client: Client,
    pub metrics: Arc<ProxyMetrics>,
}

impl ProxyState {
    pub fn new(config: Config) -> Self {
        let config_clone = config.clone();
        Self {
            config,
            models: Arc::new(RwLock::new(HashMap::new())),
            client: Client::builder()
                .timeout(Duration::from_secs(
                    config_clone.proxy.idle_timeout_secs + 30,
                ))
                .build()
                .expect("failed to build HTTP client"),
            metrics: Arc::new(ProxyMetrics::default()),
        }
    }

    /// Get the backend URL for a server name.
    pub async fn get_backend_url(&self, server_name: &str) -> Result<String> {
        let config = self.config.clone();
        let server = config
            .models
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
        let config = self.config.clone();
        let servers = config.resolve_servers_for_model(model_name);

        let models = self.models.read().await;

        // Simple round-robin or first available
        for (server_name, _, _) in servers {
            if let Some(state) = models.get(&server_name) {
                if (state.is_ready() || matches!(state, ModelState::Starting { .. }))
                    && state
                        .consecutive_failures()
                        .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
                        .unwrap_or(0)
                        <= self.config.proxy.circuit_breaker_threshold
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
        _model_card: Option<&crate::models::card::ModelCard>,
    ) -> Result<String> {
        debug!("Loading model: {}", model_name);

        let config = self.config.clone();

        // Resolve the server name for this model
        let servers = config.resolve_servers_for_model(model_name);
        let server_name = servers
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve server for model {}", model_name))?;

        // Get server and backend config from config
        let (server_config, backend_config) = config.resolve_server(&server_name)?;

        // Atomically check if already loaded and reserve if not (single write lock)
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get(&server_name) {
                if state.is_ready() || matches!(state, ModelState::Starting { .. }) {
                    debug!(
                        "Server '{}' already loaded/starting for model '{}'",
                        server_name, model_name
                    );
                    return Ok(server_name);
                }
            }

            // Reserve this server with Starting state
            models.insert(
                server_name.clone(),
                ModelState::Starting {
                    model_name: model_name.to_string(),
                    backend: server_config.backend.clone(),
                    backend_url: String::new(),
                    last_accessed: Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                },
            );
        }

        let backend_path = backend_config.path.clone();

        // Find a free port for this backend
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener); // Free the port for the backend to use

        // Build args and override --host/--port so backends always bind to localhost
        let mut args = config.build_args(server_config, backend_config);
        override_arg(&mut args, "--host", "127.0.0.1");
        override_arg(&mut args, "--port", &port.to_string());

        let health_url = format!("http://127.0.0.1:{}/health", port);
        let backend_url = format!("http://127.0.0.1:{}", port);

        info!(
            "Starting backend '{}' for server '{}' (model '{}')",
            server_config.backend, server_name, model_name
        );

        let mut child = tokio::process::Command::new(&backend_path)
            .args(&args)
            .env("MODEL_NAME", model_name)
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to execute backend process '{}'",
                    server_config.backend
                )
            })?;

        let pid = child.id().ok_or_else(|| {
            anyhow::anyhow!("Failed to get PID for backend '{}'", server_config.backend)
        })?;
        info!(
            "Backend '{}' started for server '{}' (pid: {:?})",
            server_config.backend, server_name, pid
        );

        // Spawn a reaper task so the child process is waited on and doesn't become a zombie
        let reaper_server = server_name.clone();
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    debug!(
                        "Backend process {} for server '{}' exited with {}",
                        pid, reaper_server, status
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to wait on backend process {} for server '{}': {}",
                        pid, reaper_server, e
                    );
                }
            }
        });

        // Wait for health check to pass
        let timeout = Duration::from_secs(30);
        let start = Instant::now();

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                // Kill the process on timeout to prevent orphan
                let _ = kill_process(pid).await;
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
                        load_time: std::time::SystemTime::now(),
                        last_accessed: Instant::now(),
                        consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                        failure_timestamp: None,
                    };
                }
            }
        }

        info!("Server '{}' loaded successfully", server_name);
        self.metrics
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

        // Send SIGTERM for graceful shutdown
        info!("Sending SIGTERM to backend process {}", pid);
        let _ = kill_process(pid).await;

        // Wait up to 5 seconds for the process to exit, polling every 250ms
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_process_alive(pid) {
                debug!("Backend process {} exited gracefully", pid);
                break;
            }
            if Instant::now() >= deadline {
                warn!(
                    "Backend process {} did not exit after SIGTERM, sending SIGKILL",
                    pid
                );
                let _ = force_kill_process(pid).await;
                // Brief wait for SIGKILL to take effect
                tokio::time::sleep(Duration::from_millis(500)).await;
                break;
            }
        }

        // Remove from models
        let mut models = self.models.write().await;
        models.remove(server_name);

        info!("Server '{}' unloaded", server_name);
        self.metrics
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Check if any server has been idle for longer than the timeout.
    pub async fn check_idle_timeouts(&self) -> Vec<String> {
        let now = Instant::now();
        let mut to_unload = Vec::new();
        let mut failed_to_remove = Vec::new();

        let models = self.models.read().await;
        for (server_name, state) in models.iter() {
            // Failed models have no last_accessed; always mark them for cleanup
            let last = match state.last_accessed() {
                Some(t) => t,
                None => {
                    warn!(
                        "Server '{}' is in Failed state, marking for cleanup",
                        server_name,
                    );
                    failed_to_remove.push(server_name.clone());
                    continue;
                }
            };
            let idle_duration = now.duration_since(last);
            let timeout = Duration::from_secs(self.config.proxy.idle_timeout_secs);

            if idle_duration > timeout {
                warn!(
                    "Server '{}' has been idle for {}s (timeout: {}s)",
                    server_name,
                    idle_duration.as_secs(),
                    self.config.proxy.idle_timeout_secs
                );
                to_unload.push(server_name.clone());
            }
        }

        drop(models);

        // Remove Failed models directly (no process to kill)
        if !failed_to_remove.is_empty() {
            let mut models = self.models.write().await;
            for server_name in &failed_to_remove {
                models.remove(server_name);
                info!("Removed failed server '{}' from model map", server_name);
            }
        }

        // Unload Ready models via the normal shutdown path
        for server_name in &to_unload {
            let _ = self.unload_model(server_name).await;
        }

        to_unload.extend(failed_to_remove);
        to_unload
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
        let configs_dir = self.config.configs_dir().ok()?;

        // Try to find the model card file
        // Format: configs.d/<company>--<model>.toml
        let (org, name) = model_name.split_once('/').unwrap_or(("", model_name));
        let card_filename = if org.is_empty() {
            format!("{}.toml", name)
        } else {
            format!("{}--{}.toml", org, name)
        };
        let card_path = configs_dir.join(card_filename);

        if card_path.exists() {
            let content = std::fs::read_to_string(&card_path).ok()?;
            let card: crate::models::card::ModelCard = toml::from_str(&content).ok()?;
            Some(card)
        } else {
            None
        }
    }
}

/// Override a CLI flag's value in an argument list (e.g. --host, --port).
/// If the flag exists, replaces its value. If not, appends the flag and value.
fn override_arg(args: &mut Vec<String>, flag: &str, value: &str) {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        if pos + 1 < args.len() {
            args[pos + 1] = value.to_string();
        } else {
            args.push(value.to_string());
        }
    } else {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

/// Check if a process is still alive by PID.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Check /proc/<pid> existence (Linux) as a no-dependency check
        std::path::Path::new(&format!("/proc/{}", pid)).exists()
    }
    #[cfg(windows)]
    {
        // On Windows, use tasklist to check if PID is running
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// Kill a process by PID (cross-platform).
async fn kill_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .spawn()
            .with_context(|| format!("Failed to execute kill command for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!("Failed to send SIGTERM to PID {}", pid));
        }
    }
    #[cfg(windows)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .spawn()
            .with_context(|| format!("Failed to execute taskkill command for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "Failed to terminate process with PID {}",
                pid
            ));
        }
    }
    Ok(())
}

/// Forcefully kill a process by PID (SIGKILL on Unix, taskkill /F on Windows).
async fn force_kill_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .spawn()
            .with_context(|| format!("Failed to execute kill -KILL for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!("Failed to send SIGKILL to PID {}", pid));
        }
    }
    #[cfg(windows)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .spawn()
            .with_context(|| format!("Failed to execute taskkill /F for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "Failed to forcefully terminate process with PID {}",
                pid
            ));
        }
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
    async fn test_proxy_state_new() {
        let config = Config::default();
        let state = ProxyState::new(config.clone());
        assert!(state.models.read().await.is_empty());
        assert_eq!(
            state.config.proxy.idle_timeout_secs,
            config.proxy.idle_timeout_secs
        );
    }

    #[tokio::test]
    async fn test_no_available_server_for_unknown_model() {
        let config = Config::default();
        let state = ProxyState::new(config);
        let result = state.get_available_server_for_model("nonexistent").await;
        assert!(result.is_none());
    }
}
