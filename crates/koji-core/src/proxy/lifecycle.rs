use anyhow::{Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use super::process::{force_kill_process, is_process_alive, kill_process, override_arg};
use super::types::{ModelState, ProxyState};

impl ProxyState {
    /// Load a model by starting its backend process.
    pub async fn load_model(
        &self,
        model_name: &str,
        _model_card: Option<&crate::models::card::ModelCard>,
    ) -> Result<String> {
        debug!("Loading model: {}", model_name);

        let config = self.config.read().await.clone();

        // Resolve the server name for this model
        let model_configs = self.model_configs.read().await;
        let servers = config.resolve_servers_for_model(&model_configs, model_name);
        let server_name = servers
            .first()
            .map(|(name, _, _)| name.clone())
            .ok_or_else(|| anyhow::anyhow!("Failed to resolve server for model {}", model_name))?;

        // Get server and backend config from config
        let (server_config, backend_config) =
            config.resolve_server(&model_configs, &server_name)?;

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

        // Resolve the backend binary path: DB takes priority, config.path is fallback.
        let backend_path = if let Some(db_conn) = self.open_db() {
            config.resolve_backend_path(&server_config.backend, &db_conn)?
        } else {
            let fallback_result =
                crate::db::open_in_memory().context("Failed to open in-memory DB")?;
            config.resolve_backend_path(&server_config.backend, &fallback_result.conn)?
        };

        // Find a free port for this backend.
        // Note: there is a small race window between dropping the listener and the
        // backend binding to the port. This is an accepted trade-off for local use;
        // in practice port collisions are extremely rare.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener); // Free the port for the backend to use

        // Build full args (including -m, -c, -ngl from model card) and override host/port
        let mut args = config.build_full_args(server_config, backend_config, None)?;
        override_arg(&mut args, "--host", "127.0.0.1");
        override_arg(&mut args, "--port", &port.to_string());

        let health_url = format!("http://127.0.0.1:{}/health", port);
        let backend_url = format!("http://127.0.0.1:{}", port);

        info!(
            "Starting backend '{}' for server '{}' (model '{}')",
            server_config.backend, server_name, model_name
        );

        let mut child = tokio::process::Command::new(&backend_path);
        crate::process::configure_backend_command(&mut child, &backend_path);
        child.args(&args).env("MODEL_NAME", model_name);

        info!(
            "Executing backend: {} {}",
            backend_path.display(),
            args.join(" ")
        );

        let mut child = child.spawn().with_context(|| {
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
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();
        let mut health_ok = false;

        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if start.elapsed() >= timeout {
                // Kill the process on timeout to prevent orphan
                let _ = kill_process(pid).await;
                break;
            }

            if let Ok(response) = super::process::check_health(&health_url, Some(30)).await {
                if response.status().is_success() {
                    debug!("Health check passed for server: {}", server_name);
                    health_ok = true;
                    break;
                }
            }
        }

        if !health_ok {
            // Clean up the Starting entry so future load_model calls don't short-circuit
            let mut models = self.models.write().await;
            models.remove(&server_name);
            return Err(anyhow::anyhow!(
                "Backend '{}' failed to start for server '{}' (timeout after {}s)",
                server_config.backend,
                server_name,
                timeout.as_secs()
            ));
        }

        // Update the loaded model state to Ready, reusing the existing
        // consecutive_failures Arc so external holders keep observing updates.
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut(&server_name) {
                if let ModelState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    ..
                } = state
                {
                    // Reset the counter on successful start, reuse the Arc
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = ModelState::Ready {
                        model_name: model_name.to_string(),
                        backend: server_config.backend.clone(),
                        backend_pid: pid,
                        backend_url: backend_url.clone(),
                        load_time: std::time::SystemTime::now(),
                        last_accessed: Instant::now(),
                        consecutive_failures: cf,
                        failure_timestamp: ft,
                    };
                }
            }
        }

        // Write to DB after model is ready (best-effort)
        if let Some(conn) = self.open_db() {
            let _ = crate::db::queries::insert_active_model(
                &conn,
                &server_name,
                model_name,
                &server_config.backend,
                pid as i64,
                port as i64,
                &backend_url,
            );
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

        // Write to DB after model is unloaded (best-effort)
        if let Some(conn) = self.open_db() {
            let _ = crate::db::queries::remove_active_model(&conn, server_name);
        }

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

        let idle_timeout_secs = self.config.read().await.proxy.idle_timeout_secs;

        // A timeout of 0 means auto-unload is disabled
        if idle_timeout_secs == 0 {
            return Vec::new();
        }

        let timeout = Duration::from_secs(idle_timeout_secs);
        let models = self.models.read().await;
        for (server_name, state) in models.iter() {
            // Skip servers that are still starting — they haven't had a
            // chance to become ready yet so there is nothing to unload.
            if matches!(state, ModelState::Starting { .. }) {
                continue;
            }

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
            let idle_duration = now.saturating_duration_since(last);

            if idle_duration > timeout {
                warn!(
                    "Server '{}' has been idle for {}s (timeout: {}s)",
                    server_name,
                    idle_duration.as_secs(),
                    idle_timeout_secs
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
            if let Err(e) = self.unload_model(server_name).await {
                warn!("Failed to unload server '{}': {}", server_name, e);
            }
        }

        to_unload.extend(failed_to_remove);
        to_unload
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Helper to create a Ready ModelState for testing.
    fn make_ready_state(model_name: &str, backend: &str) -> ModelState {
        ModelState::Ready {
            model_name: model_name.to_string(),
            backend: backend.to_string(),
            backend_pid: 12345,
            backend_url: "http://127.0.0.1:8080".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
        }
    }

    /// Helper to create a Starting ModelState for testing.
    fn make_starting_state(model_name: &str, backend: &str) -> ModelState {
        ModelState::Starting {
            model_name: model_name.to_string(),
            backend: backend.to_string(),
            backend_url: String::new(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
        }
    }

    /// Helper to create a Failed ModelState for testing.
    fn make_failed_state() -> ModelState {
        ModelState::Failed {
            model_name: "failed-model".to_string(),
            backend: "llama-cpp".to_string(),
            error: "test error".to_string(),
        }
    }

    /// Test that idle timeout of 0 disables auto-unload.
    #[tokio::test]
    async fn test_idle_timeout_zero_disables_auto_unload() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        // With default config, idle_timeout_secs is 0 (disabled)
        let result = state.check_idle_timeouts().await;
        assert!(
            result.is_empty(),
            "Idle timeout of 0 should disable auto-unload"
        );
    }

    /// Test that Starting state servers are skipped during idle check.
    #[tokio::test]
    async fn test_starting_state_skipped_in_idle_check() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        state.models.write().await.insert(
            "test-server".to_string(),
            make_starting_state("model.gguf", "llama-cpp"),
        );

        let result = state.check_idle_timeouts().await;
        assert!(
            result.is_empty(),
            "Starting servers should be skipped in idle check"
        );
    }

    /// Test that Ready servers with recent access are not marked for unload.
    #[tokio::test]
    async fn test_recently_accessed_server_not_unloaded() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        state.models.write().await.insert(
            "active-server".to_string(),
            make_ready_state("model.gguf", "llama-cpp"),
        );

        // The server was just created, so last_accessed is now — well within timeout
        let result = state.check_idle_timeouts().await;
        assert!(
            result.is_empty(),
            "Recently accessed servers should not be unloaded"
        );
    }

    /// Test that Failed servers without last_accessed are marked for cleanup.
    #[tokio::test]
    async fn test_failed_server_marked_for_cleanup() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        state
            .models
            .write()
            .await
            .insert("failed-server".to_string(), make_failed_state());

        let result = state.check_idle_timeouts().await;
        assert!(
            result.contains(&"failed-server".to_string()),
            "Failed servers should be marked for cleanup"
        );
    }

    /// Test that the idle timeout value from config is used.
    #[tokio::test]
    async fn test_idle_timeout_from_config() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        state.models.write().await.insert(
            "test-server".to_string(),
            make_ready_state("model.gguf", "llama-cpp"),
        );

        let result = state.check_idle_timeouts().await;
        assert!(result.is_empty());
    }

    /// Test ModelState::is_ready() returns correct values for each variant.
    #[test]
    fn test_model_state_is_ready() {
        let ready = make_ready_state("m", "llama-cpp");
        assert!(ready.is_ready());

        let starting = make_starting_state("m", "llama-cpp");
        assert!(!starting.is_ready());

        let failed = make_failed_state();
        assert!(!failed.is_ready());
    }

    /// Test ModelState::last_accessed() returns correct values.
    #[test]
    fn test_model_state_last_accessed() {
        let ready = make_ready_state("m", "llama-cpp");
        assert!(ready.last_accessed().is_some());

        let starting = make_starting_state("m", "llama-cpp");
        assert!(starting.last_accessed().is_some());

        // Failed state has no last_accessed
        let failed = make_failed_state();
        assert!(failed.last_accessed().is_none());
    }

    /// Test ModelState::backend() returns the correct backend name.
    #[test]
    fn test_model_state_backend() {
        let ready = make_ready_state("m", "llama-cpp-cuda");
        assert_eq!(ready.backend(), "llama-cpp-cuda");

        let starting = make_starting_state("m", "vllm");
        assert_eq!(starting.backend(), "vllm");
    }

    /// Test ModelState::backend_pid() returns the correct PID.
    #[test]
    fn test_model_state_backend_pid() {
        let ready = make_ready_state("m", "llama-cpp");
        assert_eq!(ready.backend_pid(), Some(12345));

        let starting = make_starting_state("m", "llama-cpp");
        assert!(starting.backend_pid().is_none());

        let failed = make_failed_state();
        assert!(failed.backend_pid().is_none());
    }

    /// Test that consecutive_failures counter is accessible.
    #[test]
    fn test_model_state_consecutive_failures() {
        let ready = make_ready_state("m", "llama-cpp");
        let failures = ready.consecutive_failures();
        assert!(failures.is_some());
        assert_eq!(failures.unwrap().load(Ordering::Relaxed), 0);
    }

    /// Test that ModelState::is_ready() distinguishes all variants correctly.
    #[test]
    fn test_model_state_variants() {
        let ready = make_ready_state("m", "llama-cpp");
        assert!(matches!(ready, ModelState::Ready { .. }));

        let starting = make_starting_state("m", "llama-cpp");
        assert!(matches!(starting, ModelState::Starting { .. }));

        let failed = make_failed_state();
        assert!(matches!(failed, ModelState::Failed { .. }));
    }

    /// Test that can_reload() returns true when no failure timestamp is set.
    #[test]
    fn test_can_reload_no_failure_timestamp() {
        let ready = make_ready_state("m", "llama-cpp");
        assert!(ready.can_reload(60));
    }

    /// Test that can_reload() returns true when cooldown has elapsed.
    #[test]
    fn test_can_reload_cooldown_elapsed() {
        let mut ready = make_ready_state("m", "llama-cpp");
        if let ModelState::Ready {
            failure_timestamp, ..
        } = &mut ready
        {
            *failure_timestamp = Some(std::time::SystemTime::now() - Duration::from_secs(120));
        }
        assert!(ready.can_reload(60));
    }

    /// Test that can_reload() returns false when cooldown is active.
    #[test]
    fn test_can_reload_cooldown_active() {
        let mut ready = make_ready_state("m", "llama-cpp");
        if let ModelState::Ready {
            failure_timestamp, ..
        } = &mut ready
        {
            *failure_timestamp = Some(std::time::SystemTime::now());
        }
        assert!(!ready.can_reload(60));
    }
}
