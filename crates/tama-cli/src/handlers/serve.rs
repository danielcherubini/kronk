//! Serve command handler
//!
//! Handles `tama serve` for starting the proxy server.

use anyhow::Result;
use tama_core::config::Config;

/// Start the tama server (proxy) with the given host, port, auto_unload setting, and idle timeout.
pub async fn cmd_serve(
    config: &Config,
    host: String,
    port: u16,
    auto_unload: bool,
    idle_timeout: u64,
) -> Result<()> {
    start_proxy_server(config, host, port, auto_unload, idle_timeout).await
}

/// Set up HF_TOKEN environment variable from config if present.
/// This must be called before any hf_hub API usage.
fn setup_hf_token(config: &Config) {
    if let Some(token) = &config.general.hf_token {
        if !token.is_empty() {
            std::env::set_var("HF_TOKEN", token);
            tracing::info!("HF_TOKEN configured from config file");
        }
    }
}

/// Start the tama server (proxy) with the given host, port, auto_unload setting, and idle timeout.
async fn start_proxy_server(
    config: &Config,
    host: String,
    port: u16,
    auto_unload: bool,
    idle_timeout: u64,
) -> Result<()> {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tama_core::proxy::ProxyServer;
    use tama_core::proxy::ProxyState;

    // Apply CLI overrides to config
    let mut updated_config = config.clone();
    updated_config.proxy.host = host.clone();
    updated_config.proxy.port = port;
    updated_config.proxy.auto_unload = auto_unload;
    updated_config.proxy.idle_timeout_secs = idle_timeout;

    // Set up HF_TOKEN from config before any hf_hub usage
    setup_hf_token(&updated_config);

    // Parse host and port
    let (host_addr, warning) = match host.parse::<std::net::IpAddr>() {
        Ok(addr) => (addr, false),
        Err(_) => (
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            true,
        ),
    };
    let addr = SocketAddr::new(host_addr, port);

    if warning {
        tracing::warn!("Invalid host '{}' - using 127.0.0.1", host);
    }

    tracing::info!("Starting tama on {}", addr);
    tracing::info!(
        "Auto-unload: {} (idle timeout: {}s)",
        auto_unload,
        idle_timeout
    );

    let db_dir = tama_core::config::Config::config_dir().ok();
    // Trigger backfill if DB is fresh (best-effort: log failures but don't abort)
    if let Some(ref dir) = db_dir {
        match tama_core::db::open(dir) {
            Ok(db_result) => {
                if db_result.needs_backfill {
                    tracing::info!("Running initial backfill...");
                    if let Err(e) = tama_core::db::backfill::run_initial_backfill(
                        &db_result.conn,
                        &updated_config,
                    )
                    .await
                    {
                        tracing::error!("Initial backfill failed: {}", e);
                    }
                }

                // Always run the backend registry TOML migration (runs once, then renames the file)
                if let Err(e) =
                    tama_core::db::backfill::migrate_backend_registry_toml(&db_result.conn, dir)
                {
                    tracing::error!("Backend registry TOML migration failed: {}", e);
                }

                // Always run the backend config TOML migration (runs once, then clears [backends])
                if let Err(e) =
                    tama_core::db::backfill::migrate_backend_config_from_toml(&db_result.conn, dir)
                {
                    tracing::error!("Backend config TOML migration failed: {}", e);
                }
            }
            Err(e) => tracing::error!("Failed to open DB for backfill check: {}", e),
        }
    }
    let state = Arc::new(ProxyState::new(updated_config.clone(), db_dir));

    // Spawn the web control plane alongside the proxy (when built with the web-ui feature).
    // The web server runs on port 11435 and terminates when this process exits.
    #[cfg(feature = "web-ui")]
    {
        let logs_dir = updated_config.logs_dir().ok();
        // Ensure logs directory exists (creates if missing)
        if let Some(ref dir) = logs_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        let web_addr: SocketAddr = "0.0.0.0:11435".parse().unwrap();
        tracing::info!("Starting tama web UI on http://{}", web_addr);
        let jobs = std::sync::Arc::new(tama_core::web_types::JobManager::new());
        let capabilities = std::sync::Arc::new(tama_core::web_types::CapabilitiesCache::new());
        let download_queue = state.download_queue.clone();

        // Shared shutdown channel: proxy server signals, web UI listens.
        // This avoids competing SIGINT/SIGTERM handlers — only the proxy
        // registers OS signals and broadcasts to the web UI.
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(());

        let state_for_web = state.clone();
        let web_handle = tokio::spawn(async move {
            // Set web-specific fields on the shared state
            let mut state_inner = (*state_for_web).clone();
            state_inner.web_jobs = Some(jobs);
            state_inner.web_capabilities = Some(capabilities);
            state_inner.web_binary_version = env!("CARGO_PKG_VERSION").to_string();
            if let Some(ref dq) = download_queue {
                state_inner.download_queue = Some(dq.clone());
            }
            let web_state = std::sync::Arc::new(state_inner);

            let jobs_for_shutdown = web_state.web_jobs.clone();
            let app = tama_web::router::build_web_routes().with_state(web_state);
            tracing::info!("Tama web UI listening on http://{}", web_addr);

            // Use axum-server for timeout-based graceful shutdown.
            let handle = axum_server::Handle::new();
            let shutdown_handle = handle.clone();

            // Listen for shutdown signal from proxy server
            tokio::spawn(async move {
                if let Err(e) = shutdown_rx.changed().await {
                    tracing::debug!("Shutdown channel closed: {}", e);
                }
                tracing::info!("Web UI initiating graceful shutdown (timeout: 5s)...");
                shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));

                // Cleanup: kill all child processes for active jobs
                if let Some(jobs) = jobs_for_shutdown {
                    if let Some(active_job) = jobs.active().await {
                        tracing::info!("Killing children of active job {}...", active_job.id);
                        jobs.kill_children(&active_job).await;
                    }
                }
            });

            let std_listener = match std::net::TcpListener::bind(web_addr) {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to bind web UI: {}", e);
                    return;
                }
            };
            std_listener.set_nonblocking(true).ok();

            if let Err(e) = axum_server::from_tcp(std_listener)
                .handle(handle)
                .serve(app.into_make_service())
                .await
            {
                tracing::error!("Web UI server error: {}", e);
            }
        });

        // Pass the shutdown sender to the proxy server
        let shutdown_tx_for_proxy = Some(shutdown_tx);

        // Create and run proxy server
        let server = ProxyServer::new(state.clone()).await;
        server.run(addr, shutdown_tx_for_proxy).await?;

        // Proxy has shut down (signal received, TTS backends unloaded, etc.).
        // Now wait for the web UI to finish its own graceful shutdown.
        // The web UI listens on the shared shutdown channel — it will exit
        // promptly since the proxy already broadcast the signal.
        // Use a timeout so systemd stop never hangs (systemd default TimeoutStopSec=90s).
        match tokio::time::timeout(std::time::Duration::from_secs(10), web_handle).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!("Web UI task joined with error: {}", e);
            }
            Err(_) => {
                tracing::warn!("Web UI did not shut down within 10s — aborting");
            }
        }

        Ok(())
    }

    // Non web-ui path
    #[cfg(not(feature = "web-ui"))]
    {
        // Create and run proxy server
        let server = ProxyServer::new(state.clone()).await;
        server.run(addr, None).await?;

        Ok(())
    }
}
