use anyhow::{Context, Result};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncBufReadExt;
use tracing::{debug, info, warn};

use super::process::{force_kill_process, is_process_alive, kill_process, override_arg};
use super::types::{BackendKind, ModelState, ProxyState};
use crate::backends::BackendRegistry;
use crate::logging;

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
                    start_time: Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    backend_type: BackendKind::Local,
                    container_id: None,
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

        // Resolve logs directory for backend log file
        let logs_dir = self.config.read().await.logs_dir().ok();

        let mut child = tokio::process::Command::new(&backend_path);
        crate::process::configure_backend_command(&mut child, &backend_path);
        child
            .args(&args)
            .env("MODEL_NAME", model_name)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

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

        // Get the backend log stream for SSE broadcasting — use same key as
        // the dashboard constructs: {backend}_{server_name}.
        let log_key = format!("{}_{}", server_config.backend, server_name);
        let log_stream = self.backend_logs.get_or_create(&log_key).await;

        // Open log file for this backend instance — include server name so
        // multiple models on the same backend get separate log files.
        let log_name = format!("{}_{}", server_config.backend, server_name);
        let log_file = logs_dir
            .as_ref()
            .and_then(|dir| logging::open_log(dir, &log_name).ok());
        let log_file_arc = log_file.map(|f| Arc::new(Mutex::new(f)));

        // Helper to push a line: broadcast + write to file.
        let push_line = Arc::new(move |line: String| {
            let stream = log_stream.clone();
            let file = log_file_arc.clone();
            tokio::spawn(async move {
                let _ = stream.push(line.clone()).await;
                if let Some(ref f) = file {
                    let _ = f.lock().map(|mut fw| {
                        let _ = writeln!(fw, "{line}");
                    });
                }
            });
        });

        // Stream stdout
        if let Some(stdout) = child.stdout.take() {
            let push = push_line.clone();
            tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push(line);
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let push = push_line.clone();
            tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    push(line);
                }
            });
        }

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
                        restart_count: 0,
                        backend_type: BackendKind::Local,
                        container_id: None,
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

    /// Evict the least-recently-used Ready model if the proxy is at capacity.
    ///
    /// This method atomically transitions a Ready model to Unloading (holding
    /// the write lock for only microseconds), then releases the lock before
    /// calling `unload_model()` (which can take up to 5 seconds). This design
    /// prevents both lock contention and race conditions.
    pub async fn evict_lru_if_needed(&self) -> Result<Option<String>> {
        let config = self.config.read().await;
        let max = config.proxy.max_loaded_models;

        // 0 = unlimited (feature disabled)
        if max == 0 {
            return Ok(None);
        }

        // Collect all Ready server names while holding the write lock.
        let models = self.models.write().await;
        let ready_servers: Vec<String> = models
            .iter()
            .filter(|(_, s)| matches!(s, ModelState::Ready { .. }))
            .map(|(name, _)| name.clone())
            .collect();

        // Release the write lock before reading model_configs (avoids deadlock).
        drop(models);

        // Only count LLM (non-TTS) models against the limit.
        let model_configs = self.model_configs.read().await;
        let llm_count = ready_servers
            .iter()
            .filter(|server_name| {
                !model_configs
                    .get(server_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_"))
            })
            .count();

        if llm_count < max as usize {
            return Ok(None);
        }

        // Find LRU Ready model among LLM (non-TTS) models only.
        let mut models = self.models.write().await;
        let lru_name = ready_servers
            .iter()
            .filter(|server_name| {
                !model_configs
                    .get(server_name.as_str())
                    .is_some_and(|mc| mc.backend.starts_with("tts_"))
            })
            .filter_map(|server_name| models.get(server_name).map(|s| (server_name, s)))
            .min_by_key(|(_, s)| s.last_accessed())
            .map(|(name, _)| name.to_string());

        // Atomically transition Ready → Unloading
        if let Some(ref name) = lru_name {
            if let Some(state) = models.get_mut(name) {
                if let ModelState::Ready {
                    model_name,
                    backend,
                    backend_pid,
                    backend_url,
                    last_accessed,
                    consecutive_failures,
                    failure_timestamp,
                    restart_count,
                    load_time: _,
                    backend_type,
                    container_id,
                } = std::mem::take(state)
                {
                    *state = ModelState::Unloading {
                        model_name,
                        backend,
                        backend_pid,
                        backend_url,
                        last_accessed,
                        consecutive_failures,
                        failure_timestamp,
                        restart_count,
                        backend_type,
                        container_id,
                    };
                }
            }
        }

        drop(models); // Release lock BEFORE calling unload_model (can take 5s)

        if let Some(name) = lru_name {
            self.unload_model(&name).await?;
            Ok(Some(name))
        } else {
            // All models are non-Ready (Starting/Failed/Unloading) — can't evict
            Ok(None)
        }
    }

    /// Unload a server by stopping its backend process.
    pub async fn unload_model(&self, server_name: &str) -> Result<()> {
        debug!("Unloading server: {}", server_name);

        let state = self
            .get_model_state(server_name)
            .await
            .with_context(|| format!("Server '{}' not loaded", server_name))?;

        if !matches!(
            state,
            ModelState::Ready { .. } | ModelState::Unloading { .. }
        ) {
            return Err(anyhow::anyhow!(
                "Server '{}' is not ready (state: {:?})",
                server_name,
                state
            ));
        }

        // Handle Docker backends separately
        if state.is_docker() {
            // Docker unload handled by docker::uninstall::stop_container() in Task 4
            let mut models = self.models.write().await;
            models.remove(server_name);
            return Ok(());
        }

        let (backend_name, pid) = match &state {
            ModelState::Ready {
                backend,
                backend_pid,
                ..
            }
            | ModelState::Unloading {
                backend,
                backend_pid,
                ..
            } => (backend.clone(), *backend_pid),
            _ => unreachable!("already checked above"),
        };

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
    ///
    /// Also performs process health monitoring:
    /// - Detects dead PIDs in Ready models and confirms via health endpoint
    /// - Transitions stuck Starting models to Failed
    /// - Auto-restarts dead models (respecting max_restarts and restart_delay_ms)
    /// - Cleans up Failed models
    pub async fn check_idle_timeouts(&self) -> Vec<String> {
        let now = Instant::now();
        let mut to_unload = Vec::new();
        let mut failed_to_remove = Vec::new();
        // (server_name, model_name, backend, restart_count, pid, backend_url)
        let mut dead_pid_candidates: Vec<(String, String, String, u32, u32, String)> = Vec::new();
        // (server_name, model_name, backend, start_time)
        let mut stuck_starting_servers: Vec<(String, String, String, Instant)> = Vec::new();

        let (auto_unload, idle_timeout_secs, startup_timeout_secs, max_restarts, restart_delay_ms) = {
            let cfg = self.config.read().await;
            (
                cfg.proxy.auto_unload,
                cfg.proxy.idle_timeout_secs,
                cfg.proxy.startup_timeout_secs,
                cfg.supervisor.max_restarts,
                cfg.supervisor.restart_delay_ms,
            )
        };

        let idle_timeout = Duration::from_secs(idle_timeout_secs);
        let startup_timeout = Duration::from_secs(startup_timeout_secs);

        // === PHASE 1: Collect candidates under read lock (fast only) ===
        let models = self.models.read().await;
        for (server_name, state) in models.iter() {
            // Check Starting state first (including TTS — they can also get stuck)
            if let ModelState::Starting { start_time, .. } = state {
                if now.saturating_duration_since(*start_time) > startup_timeout {
                    warn!(
                        "Server '{}' stuck in Starting for {}s (timeout: {}s)",
                        server_name,
                        now.saturating_duration_since(*start_time).as_secs(),
                        startup_timeout_secs,
                    );
                    stuck_starting_servers.push((
                        server_name.clone(),
                        state.model_name().to_string(),
                        state.backend().to_string(),
                        *start_time,
                    ));
                }
                continue;
            }

            // Skip Unloading — already being handled
            if matches!(state, ModelState::Unloading { .. }) {
                continue;
            }

            // Skip TTS backends for Ready checks (separate lifecycle)
            // TTS Starting was already checked above
            if state.is_tts_backend() {
                continue;
            }

            // Skip Docker backends — container state is checked separately
            if state.is_docker() {
                continue;
            }

            // Ready models — check PID liveness (fast syscall, OK under lock)
            if let ModelState::Ready {
                backend_pid,
                restart_count,
                ..
            } = state
            {
                let pid = *backend_pid;
                if !super::process::is_process_alive(pid) {
                    dead_pid_candidates.push((
                        server_name.clone(),
                        state.model_name().to_string(),
                        state.backend().to_string(),
                        *restart_count,
                        pid,
                        state
                            .backend_url()
                            .map(|u| u.to_string())
                            .unwrap_or_default(),
                    ));
                    continue; // Skip idle check — process is dead
                }

                // Process alive — check idle timeout (existing logic)
                if let Some(last) = state.last_accessed() {
                    let idle_duration = now.saturating_duration_since(last);
                    if auto_unload && idle_duration > idle_timeout {
                        warn!(
                            "Server '{}' idle for {}s (timeout: {}s)",
                            server_name,
                            idle_duration.as_secs(),
                            idle_timeout_secs
                        );
                        to_unload.push(server_name.clone());
                    }
                }
            }

            // Failed models — mark for cleanup
            if matches!(state, ModelState::Failed { .. }) {
                warn!(
                    "Server '{}' in Failed state, marking for cleanup",
                    server_name
                );
                failed_to_remove.push(server_name.clone());
            }
        }
        drop(models); // Release read lock

        // === PHASE 2: Health confirmation (outside lock) ===
        // (server_name, model_name, backend, restart_count, pid)
        let mut confirmed_dead: Vec<(String, String, String, u32, u32)> = Vec::new();
        for (server_name, model_name, backend, restart_count, pid, backend_url) in
            dead_pid_candidates
        {
            let health_url = format!("{}/health", backend_url);
            let still_dead = match super::process::check_health(&health_url, Some(5)).await {
                Ok(resp) => !resp.status().is_success(),
                Err(_) => true,
            };

            if still_dead {
                info!(
                    "Server '{}' confirmed dead (pid {}, restart_count: {}/{})",
                    server_name, pid, restart_count, max_restarts
                );
                confirmed_dead.push((server_name, model_name, backend, restart_count, pid));
            } else {
                debug!(
                    "Server '{}' PID {} reused, health endpoint responds",
                    server_name, pid
                );
            }
        }

        // === PHASE 3: Mutations ===

        // Remove Failed models
        if !failed_to_remove.is_empty() {
            let mut models = self.models.write().await;
            for server_name in &failed_to_remove {
                models.remove(server_name);
                info!("Removed failed server '{}' from model map", server_name);
            }
        }

        // Handle stuck Starting — transition to Failed
        if !stuck_starting_servers.is_empty() {
            let mut models = self.models.write().await;
            for (server_name, model_name, backend, observed_start) in &stuck_starting_servers {
                // Revalidate: only transition if still in Starting state with matching start_time
                // (could have become Ready between Phase 1 and Phase 3)
                if let Some(existing) = models.get(server_name) {
                    let still_starting = matches!(existing, ModelState::Starting { start_time, .. } if start_time == observed_start);
                    if !still_starting {
                        debug!(
                            "Server '{}' state or start_time changed, skipping stuck transition",
                            server_name
                        );
                        continue;
                    }
                }
                models.insert(
                    server_name.clone(),
                    ModelState::Failed {
                        model_name: model_name.clone(),
                        backend: backend.clone(),
                        error: format!(
                            "Stuck in Starting state for {}s — backend failed to initialize",
                            startup_timeout_secs
                        ),
                    },
                );
                warn!(
                    "Transitioned '{}' to Failed (stuck in Starting)",
                    server_name
                );
            }
        }

        // Handle dead Ready servers — clean up + insert Failed or spawn restart
        if !confirmed_dead.is_empty() {
            // Remove + insert Failed under SAME lock — no race
            // Revalidate state under lock to avoid TOCTOU with forward_request()
            let mut to_restart: Vec<(String, String, u32)> = Vec::new();
            let mut removed_servers: Vec<String> = Vec::new();
            {
                let mut models = self.models.write().await;
                for (server_name, model_name, backend, restart_count, observed_pid) in
                    &confirmed_dead
                {
                    // Revalidate: only act if still Ready with matching PID
                    // (could have been replaced by forward_request() auto-load)
                    let pid_matches = models.get(server_name).and_then(|s| match s {
                        ModelState::Ready { backend_pid, .. } => {
                            if backend_pid == observed_pid {
                                Some(true)
                            } else {
                                // Different PID — process was replaced, skip
                                None
                            }
                        }
                        ModelState::Starting { .. } => {
                            // Already being restarted by another path, skip
                            None
                        }
                        _ => None, // Failed, Unloading, or absent — skip
                    });

                    if pid_matches.unwrap_or(false) {
                        models.remove(server_name);
                        removed_servers.push(server_name.clone());
                        if *restart_count >= max_restarts {
                            models.insert(
                                server_name.clone(),
                                ModelState::Failed {
                                    model_name: model_name.clone(),
                                    backend: backend.clone(),
                                    error: format!(
                                        "Exceeded maximum restart attempts ({}) — manual intervention required",
                                        max_restarts
                                    ),
                                },
                            );
                            warn!(
                                "Server '{}' exceeded max restarts ({}/{})",
                                server_name, restart_count, max_restarts
                            );
                        } else {
                            to_restart.push((
                                server_name.clone(),
                                model_name.clone(),
                                *restart_count,
                            ));
                        }
                    } else {
                        debug!(
                            "Server '{}' state changed during health check, skipping cleanup",
                            server_name
                        );
                    }
                }
            }
            // Clean DB — remove ALL dead entries so cleanup_stale_processes()
            // doesn't rediscover them, regardless of whether they'll be restarted
            if let Some(conn) = self.open_db() {
                for server_name in &removed_servers {
                    let _ = crate::db::queries::remove_active_model(&conn, server_name);
                }
            }

            // Spawn restart tasks (no locks)
            for (server_name, model_name, restart_count) in &to_restart {
                let new_restart_count = restart_count + 1;
                info!(
                    "Auto-restarting '{}' (model '{}', attempt {}/{})",
                    server_name, model_name, new_restart_count, max_restarts
                );

                let state = self.clone();
                let sn = server_name.clone();
                let mn = model_name.clone();
                let rdc = new_restart_count;
                let delay_ms = restart_delay_ms;
                // Total timeout: delay + startup_timeout_secs. Prevents a stuck
                // restart from holding resources forever (also keeps tests from
                // hanging when there's no real backend to load).
                let total_timeout = Duration::from_millis(delay_ms) + startup_timeout;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    match tokio::time::timeout(total_timeout, state.load_model(&mn, None)).await {
                        Ok(Ok(_)) => {
                            let mut models = state.models.write().await;
                            if let Some(ModelState::Ready {
                                restart_count: rc, ..
                            }) = models.get_mut(&sn)
                            {
                                *rc = rdc;
                            }
                            info!("Auto-restart succeeded for '{}' (model '{}')", sn, mn);
                        }
                        Ok(Err(e)) => {
                            warn!("Auto-restart failed for '{}' (model '{}'): {}", sn, mn, e);
                        }
                        Err(_) => {
                            warn!(
                                "Auto-restart timed out for '{}' (model '{}') after {:?}",
                                sn, mn, total_timeout
                            );
                        }
                    }
                });
            }
        }

        // Unload idle models (existing logic)
        for server_name in &to_unload {
            if let Err(e) = self.unload_model(server_name).await {
                warn!("Failed to unload '{}': {}", server_name, e);
            }
        }

        // Build return value
        let mut cleaned = Vec::new();
        cleaned.extend(failed_to_remove);
        cleaned.extend(stuck_starting_servers.iter().map(|(n, _, _, _)| n.clone()));
        cleaned.extend(confirmed_dead.iter().map(|(n, _, _, _, _)| n.clone()));
        cleaned.extend(to_unload);
        cleaned
    }

    /// Load a TTS backend (Kokoro-FastAPI) by spawning its uvicorn server.
    ///
    /// This method opens the backend registry, looks up the requested backend,
    /// derives paths from its install directory, finds a free port, and spawns
    /// the Kokoro-FastAPI uvicorn process with appropriate environment variables.
    /// It then performs a health check (polling every 2s, timeout 60s) before
    /// transitioning the model state to Ready.
    pub async fn load_tts_backend(&self, backend_name: &str) -> Result<String> {
        debug!("Loading TTS backend: {}", backend_name);

        // Open registry and look up backend by name
        let base_dir =
            crate::config::Config::base_dir().with_context(|| "Failed to get config directory")?;
        let registry =
            BackendRegistry::open(&base_dir).with_context(|| "Failed to open backend registry")?;

        let info = registry
            .get(backend_name)
            .with_context(|| format!("Backend '{}' not found in registry", backend_name))?
            .ok_or_else(|| anyhow::anyhow!("Backend '{}' not installed", backend_name))?;

        // Derive paths from BackendInfo.path (base_dir = backends/tts_kokoro/).
        // The repo root is the kokoro-fastapi subdirectory, and venv is a sibling.
        let base_path = info.path.as_path();
        let repo_root = base_path.join("kokoro-fastapi");
        let venv_dir = base_path.join("venv");
        let python_bin = venv_dir.join("bin").join("python");

        // Atomically check if already loaded and reserve if not
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get(backend_name) {
                if state.is_ready() || matches!(state, ModelState::Starting { .. }) {
                    debug!("TTS backend '{}' already loaded/starting", backend_name);
                    return Ok(backend_name.to_string());
                }
            }

            // Reserve with Starting state
            models.insert(
                backend_name.to_string(),
                ModelState::Starting {
                    model_name: backend_name.to_string(),
                    backend: info.name.clone(),
                    backend_url: String::new(),
                    last_accessed: Instant::now(),
                    start_time: Instant::now(),
                    consecutive_failures: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                    failure_timestamp: None,
                    backend_type: BackendKind::Local,
                    container_id: None,
                },
            );
        }

        // Find a free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let backend_url = format!("http://127.0.0.1:{}", port);
        let health_url = format!("http://127.0.0.1:{}/health", port);

        info!("Starting Kokoro-FastAPI TTS backend on port {}", port);

        // Spawn the uvicorn server process
        let mut child = tokio::process::Command::new(&python_bin);
        child
            .args([
                "-m",
                "uvicorn",
                "api.src.main:app",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .current_dir(&repo_root)
            .env("PYTHONPATH", &repo_root)
            .env("MODEL_DIR", "api/src/models")
            .env("VOICES_DIR", "api/src/voices/v1_0");

        let mut child = child.spawn().with_context(|| {
            format!(
                "Failed to spawn Kokoro-FastAPI process: {}",
                python_bin.display()
            )
        })?;

        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Failed to get PID for Kokoro-FastAPI"))?;
        info!("Kokoro-FastAPI started (pid: {:?})", pid);

        // Spawn a reaper task so the child process is waited on
        let reaper_backend = backend_name.to_string();
        tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => {
                    debug!(
                        "Kokoro-FastAPI process {} for backend '{}' exited with {}",
                        pid, reaper_backend, status
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to wait on Kokoro-FastAPI process {} for backend '{}': {}",
                        pid, reaper_backend, e
                    );
                }
            }
        });

        // Health check: poll every 2s, use configured startup timeout
        let timeout = Duration::from_secs(self.config.read().await.proxy.startup_timeout_secs);
        let start = Instant::now();
        let mut health_ok = false;

        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if start.elapsed() >= timeout {
                let _ = kill_process(pid).await;
                break;
            }

            if let Ok(response) = super::process::check_health(&health_url, Some(30)).await {
                if response.status().is_success() {
                    debug!("Health check passed for TTS backend: {}", backend_name);
                    health_ok = true;
                    break;
                }
            }
        }

        if !health_ok {
            let mut models = self.models.write().await;
            models.remove(backend_name);
            return Err(anyhow::anyhow!(
                "Kokoro-FastAPI failed to start for backend '{}' (timeout after {}s)",
                backend_name,
                timeout.as_secs()
            ));
        }

        // Update to Ready state
        {
            let mut models = self.models.write().await;
            if let Some(state) = models.get_mut(backend_name) {
                if let ModelState::Starting {
                    consecutive_failures,
                    failure_timestamp,
                    model_name,
                    ..
                } = state
                {
                    consecutive_failures.store(0, std::sync::atomic::Ordering::Relaxed);
                    let cf = Arc::clone(consecutive_failures);
                    let ft = *failure_timestamp;
                    *state = ModelState::Ready {
                        model_name: model_name.clone(),
                        backend: info.name.clone(),
                        backend_pid: pid,
                        backend_url: backend_url.clone(),
                        load_time: std::time::SystemTime::now(),
                        last_accessed: Instant::now(),
                        consecutive_failures: cf,
                        failure_timestamp: ft,
                        restart_count: 0,
                        backend_type: BackendKind::Local,
                        container_id: None,
                    };
                }
            }
        }

        info!("TTS backend '{}' loaded successfully", backend_name);
        self.metrics
            .models_loaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(backend_name.to_string())
    }

    /// Unload a TTS backend by stopping its subprocess.
    ///
    /// Sends SIGTERM for graceful shutdown, waits up to 5s, then SIGKILL if needed.
    pub async fn unload_tts_backend(&self, backend_name: &str) -> Result<()> {
        debug!("Unloading TTS backend: {}", backend_name);

        let state = self
            .get_model_state(backend_name)
            .await
            .with_context(|| format!("TTS backend '{}' not loaded", backend_name))?;

        if !matches!(
            state,
            ModelState::Ready { .. } | ModelState::Unloading { .. }
        ) {
            return Err(anyhow::anyhow!(
                "TTS backend '{}' is not ready (state: {:?})",
                backend_name,
                state
            ));
        }

        let pid = match &state {
            ModelState::Ready { backend_pid, .. } => *backend_pid,
            ModelState::Unloading { backend_pid, .. } => *backend_pid,
            _ => unreachable!("already checked above"),
        };

        info!("Stopping Kokoro-FastAPI (pid: {})", pid);

        // Send SIGTERM for graceful shutdown
        let _ = kill_process(pid).await;

        // Wait up to 5 seconds for the process to exit, polling every 250ms
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !is_process_alive(pid) {
                debug!("Kokoro-FastAPI exited gracefully");
                break;
            }
            if Instant::now() >= deadline {
                warn!("Kokoro-FastAPI did not exit after SIGTERM, sending SIGKILL",);
                let _ = force_kill_process(pid).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                break;
            }
        }

        // Remove from models
        self.models.write().await.remove(backend_name);

        info!("TTS backend '{}' unloaded", backend_name);
        self.metrics
            .models_unloaded
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Check if a TTS backend is loaded and ready.
    ///
    /// Returns the backend name if found in Ready state, None otherwise.
    pub async fn get_tts_server(&self, backend_name: &str) -> Option<String> {
        let models = self.models.read().await;
        if let Some(state) = models.get(backend_name) {
            if state.is_ready() {
                return Some(backend_name.to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Helper to create a Ready ModelState for testing.
    /// Uses a high PID that won't exist and won't conflict with real processes.
    fn make_ready_state(model_name: &str, backend: &str) -> ModelState {
        ModelState::Ready {
            model_name: model_name.to_string(),
            backend: backend.to_string(),
            backend_pid: 12345, // fake PID — won't be killed by tests
            backend_url: "http://127.0.0.1:8080".to_string(),
            load_time: std::time::SystemTime::now(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            restart_count: 0,
            backend_type: BackendKind::Local,
            container_id: None,
        }
    }

    /// Helper to create a Starting ModelState for testing.
    fn make_starting_state(model_name: &str, backend: &str) -> ModelState {
        ModelState::Starting {
            model_name: model_name.to_string(),
            backend: backend.to_string(),
            backend_url: String::new(),
            last_accessed: Instant::now(),
            start_time: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            backend_type: BackendKind::Local,
            container_id: None,
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

    /// Helper to create an Unloading ModelState for testing.
    fn make_unloading_state(model_name: &str, backend: &str) -> ModelState {
        ModelState::Unloading {
            model_name: model_name.to_string(),
            backend: backend.to_string(),
            backend_pid: 54321,
            backend_url: "http://127.0.0.1:9000".to_string(),
            last_accessed: Instant::now(),
            consecutive_failures: Arc::new(AtomicU32::new(0)),
            failure_timestamp: None,
            restart_count: 0,
            backend_type: BackendKind::Local,
            container_id: None,
        }
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

    /// Test that Unloading state model_name() returns the correct name.
    #[test]
    fn test_unloading_model_name() {
        let unloading = make_unloading_state("unload-model", "llama-cpp");
        assert_eq!(unloading.model_name(), "unload-model");
    }

    /// Test that Unloading state backend() returns the correct backend.
    #[test]
    fn test_unloading_backend() {
        let unloading = make_unloading_state("m", "vllm");
        assert_eq!(unloading.backend(), "vllm");
    }

    /// Test that Unloading state is_ready() returns false.
    #[test]
    fn test_unloading_is_not_ready() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert!(!unloading.is_ready());
    }

    /// Test that Unloading state backend_url() returns None.
    #[test]
    fn test_unloading_backend_url_none() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert!(unloading.backend_url().is_none());
    }

    /// Test that Unloading state backend_pid() returns the PID.
    #[test]
    fn test_unloading_backend_pid() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert_eq!(unloading.backend_pid(), Some(54321));
    }

    /// Test that Unloading state consecutive_failures() returns the counter.
    #[test]
    fn test_unloading_consecutive_failures() {
        let unloading = make_unloading_state("m", "llama-cpp");
        let failures = unloading.consecutive_failures();
        assert!(failures.is_some());
        assert_eq!(failures.unwrap().load(Ordering::Relaxed), 0);
    }

    /// Test that Unloading state load_time() returns None.
    #[test]
    fn test_unloading_load_time_none() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert!(unloading.load_time().is_none());
    }

    /// Test that Unloading state last_accessed() returns Some.
    #[test]
    fn test_unloading_last_accessed() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert!(unloading.last_accessed().is_some());
    }

    /// Test that Unloading state can_reload() returns false.
    #[test]
    fn test_unloading_can_reload_false() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert!(!unloading.can_reload(60));
    }

    /// Test that ModelState::Default produces a Failed state with empty strings.
    #[test]
    fn test_model_state_default_is_failed() {
        let default_state = ModelState::default();
        assert!(!default_state.is_ready());
        assert_eq!(default_state.model_name(), "");
        assert_eq!(default_state.backend(), "");
    }

    /// Test that Unloading state matches correctly.
    #[test]
    fn test_unloading_variant_match() {
        let unloading = make_unloading_state("m", "llama-cpp");
        assert!(matches!(unloading, ModelState::Unloading { .. }));
    }

    /// Test that evict_lru_if_needed returns Ok(None) when max_loaded_models is 0 (unlimited).
    #[tokio::test]
    async fn test_evict_lru_if_needed_zero_is_unlimited() {
        let mut config = Config::default();
        config.proxy.max_loaded_models = 0;
        let state = ProxyState::new(config, None);

        // Add a Ready model to ensure we're not returning None due to empty map
        state.models.write().await.insert(
            "server1".to_string(),
            make_ready_state("model.gguf", "llama-cpp"),
        );

        let result = state.evict_lru_if_needed().await;
        assert!(
            result.is_ok(),
            "evict_lru_if_needed should succeed with unlimited config"
        );
        assert_eq!(
            result.unwrap(),
            None,
            "Should return None when max_loaded_models is 0"
        );
    }

    /// Test that evict_lru_if_needed returns Ok(None) when model count is below the limit.
    #[tokio::test]
    async fn test_evict_lru_if_needed_under_limit_no_eviction() {
        let mut config = Config::default();
        config.proxy.max_loaded_models = 2;
        let state = ProxyState::new(config, None);

        // Add 1 Ready model (below limit of 2)
        state.models.write().await.insert(
            "server1".to_string(),
            make_ready_state("model.gguf", "llama-cpp"),
        );

        let result = state.evict_lru_if_needed().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None, "Should return None when under limit");

        // Verify model count is unchanged
        assert_eq!(
            state.models.read().await.len(),
            1,
            "Model count should be unchanged"
        );
    }

    /// Test that evict_lru_if_needed evicts the LRU Ready model when at capacity.
    #[tokio::test]
    async fn test_evict_lru_if_needed_at_limit_evicts_lru() {
        let mut config = Config::default();
        config.proxy.max_loaded_models = 1;
        let state = ProxyState::new(config, None);

        // Add a Ready model with last_accessed set in the past
        let mut ready_state = make_ready_state("model.gguf", "llama-cpp");
        if let ModelState::Ready { last_accessed, .. } = &mut ready_state {
            *last_accessed = Instant::now() - Duration::from_secs(300);
        }
        state
            .models
            .write()
            .await
            .insert("server1".to_string(), ready_state);

        let result = state.evict_lru_if_needed().await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            Some("server1".to_string()),
            "Should evict the only Ready model when at capacity"
        );

        // Verify model was removed from the map
        assert!(
            !state.models.read().await.contains_key("server1"),
            "Evicted model should be removed from the map"
        );
    }

    /// Test that evict_lru_if_needed skips Starting models.
    #[tokio::test]
    async fn test_evict_lru_if_needed_skips_starting_models() {
        let mut config = Config::default();
        config.proxy.max_loaded_models = 1;
        let state = ProxyState::new(config, None);

        // Add a Starting model (not Ready)
        state.models.write().await.insert(
            "server1".to_string(),
            make_starting_state("model.gguf", "llama-cpp"),
        );

        let result = state.evict_lru_if_needed().await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            None,
            "Should return None when no Ready models are available"
        );

        // Verify Starting model remains in the map
        assert!(
            state.models.read().await.contains_key("server1"),
            "Starting model should remain in the map"
        );
    }

    /// Test that evict_lru_if_needed skips Failed models.
    #[tokio::test]
    async fn test_evict_lru_if_needed_skips_failed_models() {
        let mut config = Config::default();
        config.proxy.max_loaded_models = 1;
        let state = ProxyState::new(config, None);

        // Add a Failed model
        state
            .models
            .write()
            .await
            .insert("server1".to_string(), make_failed_state());

        let result = state.evict_lru_if_needed().await;
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            None,
            "Should return None when no Ready models are available"
        );
    }

    /// Test that concurrent evict calls don't double-evict the same model.
    /// With max_loaded_models=1 and 3 models (2 Ready + 1 Starting), each call
    /// finds a different Ready model since the Starting model is skipped.
    #[tokio::test]
    async fn test_evict_lru_if_needed_concurrent_no_double_eviction() {
        let mut config = Config::default();
        config.proxy.max_loaded_models = 1;
        let state = ProxyState::new(config, None);

        // Add 2 Ready models with different last_accessed times (LRU + newer)
        let mut ready1 = make_ready_state("model1.gguf", "llama-cpp");
        if let ModelState::Ready { last_accessed, .. } = &mut ready1 {
            *last_accessed = Instant::now() - Duration::from_secs(600); // older
        }
        state
            .models
            .write()
            .await
            .insert("server1".to_string(), ready1);

        let mut ready2 = make_ready_state("model2.gguf", "llama-cpp");
        if let ModelState::Ready { last_accessed, .. } = &mut ready2 {
            *last_accessed = Instant::now() - Duration::from_secs(100); // newer
        }
        state
            .models
            .write()
            .await
            .insert("server2".to_string(), ready2);

        // Add 1 Starting model — it should be skipped by eviction, ensuring
        // both concurrent calls have a Ready model to evict.
        state.models.write().await.insert(
            "server3".to_string(),
            make_starting_state("model3.gguf", "llama-cpp"),
        );

        // Run two evict calls concurrently
        let state_a = state.clone();
        let state_b = state.clone();
        let handle_a = tokio::spawn(async move { state_a.evict_lru_if_needed().await });
        let handle_b = tokio::spawn(async move { state_b.evict_lru_if_needed().await });

        let result_a = handle_a.await.unwrap();
        let result_b = handle_b.await.unwrap();

        // Both calls should succeed (each evicts a different Ready model)
        assert!(result_a.is_ok());
        assert!(result_b.is_ok());

        // Each call returns a different server name — no double-eviction
        let name_a = result_a.unwrap().unwrap();
        let name_b = result_b.unwrap().unwrap();
        assert_ne!(
            name_a, name_b,
            "Concurrent calls must evict different models (no double-eviction)"
        );

        // Both evicted models should be removed from the map
        assert!(
            !state.models.read().await.contains_key(&name_a),
            "Evicted model '{}' should be removed",
            name_a
        );
        assert!(
            !state.models.read().await.contains_key(&name_b),
            "Evicted model '{}' should be removed",
            name_b
        );
    }

    /// Test that TTS backends are excluded from LRU eviction count.
    #[tokio::test]
    async fn test_evict_lru_excludes_tts_backends() {
        use crate::config::ModelConfig;

        let mut config = Config::default();
        config.proxy.max_loaded_models = 1;
        let state = ProxyState::new(config, None);

        // Register the TTS server in model_configs with a tts_ backend
        // so it's excluded from the LLM count.
        state.model_configs.write().await.insert(
            "tts-server".to_string(),
            ModelConfig {
                backend: "tts_kokoro".to_string(),
                ..Default::default()
            },
        );

        // Add a TTS backend (tts_kokoro) — should NOT count toward limit
        let tts_state = make_ready_state("model.gguf", "tts_kokoro");
        state
            .models
            .write()
            .await
            .insert("tts-server".to_string(), tts_state);

        // Verify no eviction happens (TTS doesn't count)
        let result = state.evict_lru_if_needed().await.unwrap();
        assert_eq!(result, None, "TTS backends should not trigger eviction");

        // Verify the TTS model is still in the map
        assert!(
            state.models.read().await.contains_key("tts-server"),
            "TTS backend should remain loaded"
        );
    }
}
