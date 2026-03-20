pub mod server;

use crate::config::Config;
use crate::models::card::ModelCard;
use anyhow::{Context, Result};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};

use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command as TokioCommand;
use tokio::sync::RwLock;
use tokio::time::interval;
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

    pub fn last_accessed(&self) -> Instant {
        match self {
            ModelState::Ready { last_accessed, .. } => *last_accessed,
            ModelState::Starting { last_accessed, .. } => *last_accessed,
            ModelState::Failed { .. } => Instant::now(),
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
        Self {
            config,
            models: Arc::new(RwLock::new(HashMap::new())),
            client: Client::new(),
            metrics: Arc::new(ProxyMetrics::default()),
        }
    }

    /// Get the backend URL for a server name.
    pub async fn get_backend_url(&self, server_name: &str) -> Result<String> {
        let config = self.config.clone();
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
    ) -> Option<(ModelState, Instant)> {
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
                        > self.config.proxy.circuit_breaker_threshold
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

        let config = self.config.clone();

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
                    debug!(
                        "Server '{}' already loaded for model '{}'",
                        server_name, model_name
                    );
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
                            last_accessed: Instant::now(),
                            consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                            failure_timestamp: None,
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
            .with_context(|| format!("No backend URL resolved for server: {}", server_name))?;

        info!(
            "Starting backend '{}' for server '{}' (model '{}')",
            server_config.backend, server_name, model_name
        );

        let child = tokio::process::Command::new(&backend_path)
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

        // Kill the process if we have the PID
        info!("Sending SIGTERM to backend process {}", pid);
        let _ = kill_process(pid).await;

        // Wait up to 5 seconds for graceful shutdown
        tokio::time::sleep(Duration::from_secs(5)).await;

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

        let models = self.models.read().await;
        for (server_name, state) in models.iter() {
            let idle_duration = now.duration_since(state.last_accessed());
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

    /// Start the idle reaper background task.
    pub fn start_idle_reaper(&self) {
        let state = self.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let _ = state.check_idle_timeouts().await;
            }
        });
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

/// List all available models (OpenAI API compatible).
async fn list_models(State(state): State<ProxyState>) -> impl IntoResponse {
    let mut data = Vec::new();
    for name in state.config.servers.keys() {
        data.push(serde_json::json!({
            "id": name,
            "object": "model",
            "created": 0,
            "owned_by": "kronk"
        }));
    }
    (
        StatusCode::OK,
        axum::Json(serde_json::json!({ "object": "list", "data": data })),
    )
}

/// Get details for a single model (OpenAI API compatible).
async fn get_model(
    Path(model_name): Path<String>,
    State(state): State<ProxyState>,
) -> Result<impl IntoResponse, StatusCode> {
    if state.config.servers.contains_key(&model_name) {
        Ok(axum::Json(serde_json::json!({
            "id": model_name,
            "object": "model",
            "created": 0,
            "owned_by": "kronk"
        })))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// Proxy handler for all other /v1/* paths.
async fn proxy_request(
    State(state): State<ProxyState>,
    req: axum::extract::Request,
) -> Result<Response<Body>, (StatusCode, String)> {
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();

    // 1. Read the entire body into memory to parse it and forward it
    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // 2. Parse JSON just to find the "model" key
    let json_body: Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)))?;

    let model_name = json_body.get("model").and_then(|m| m.as_str()).ok_or((
        StatusCode::BAD_REQUEST,
        "Missing 'model' field in JSON payload".to_string(),
    ))?;

    // 3. Ensure model is running and get its local port
    let server_name = state
        .load_model(model_name, None)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 4. Construct upstream URL
    let backend_url = state
        .get_backend_url(&server_name)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut target_url = format!("http://{}{}", backend_url, path);
    if !query.is_empty() {
        target_url.push_str(&query);
    }

    // 5. Forward the request using Reqwest
    let reqwest_res = state
        .client
        .post(&target_url)
        .header("Content-Type", "application/json")
        .body(reqwest::Body::from(body_bytes))
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    // 6. Convert the Reqwest response (SSE stream) directly into an Axum body
    let mut response_builder = Response::builder().status(reqwest_res.status());
    for (key, value) in reqwest_res.headers() {
        response_builder = response_builder.header(key, value);
    }
    let axum_body = Body::from_stream(reqwest_res.bytes_stream());

    Ok(response_builder.body(axum_body).unwrap())
}

/// Start the OpenAI proxy server.
pub async fn start_server(config: Config) -> Result<()> {
    let state = ProxyState::new(config.clone());

    state.start_idle_reaper();

    let app = Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/models/:model", get(get_model))
        // Catch-all POST for the proxy handler
        .route("/v1/*path", post(proxy_request))
        .with_state(state);

    let bind_addr = format!("{}:{}", config.proxy.host, config.proxy.port);
    info!("Starting OpenAI proxy server on http://{}", bind_addr);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_models() {
        let config = Config::default();
        let state = ProxyState::new(config);

        let response = list_models(State(state)).await;
        let body_bytes = axum::body::to_bytes(response.into_response().into_body(), usize::MAX)
            .await
            .unwrap();
        let body = serde_json::from_slice::<serde_json::Value>(&body_bytes).unwrap();
        assert_eq!(body["object"], "list");
    }
}
