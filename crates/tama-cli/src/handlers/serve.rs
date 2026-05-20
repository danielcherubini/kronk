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

    #[cfg(feature = "web-ui")]
    {
        let logs_dir = updated_config.logs_dir().ok();
        // Ensure logs directory exists (creates if missing)
        if let Some(ref dir) = logs_dir {
            let _ = std::fs::create_dir_all(dir);
        }

        // Build the unified router: proxy routes + web UI routes on a single server.
        // The proxy handles OS signals (SIGTERM/SIGINT) and graceful shutdown.
        let web_routes = tama_web::router::build_web_routes();
        let server = ProxyServer::new(state.clone()).await;
        let app = server.into_unified_router(web_routes);

        // Clone state for shutdown cleanup (unloads TTS backends + kills job children)
        let cleanup_state = Arc::clone(&state);
        let on_shutdown = async move {
            // Kill children of any active backend job
            if let Some(jobs) = &cleanup_state.web_jobs {
                if let Some(active_job) = jobs.active().await {
                    tracing::info!("Killing children of active job {}...", active_job.id);
                    jobs.kill_children(&active_job).await;
                }
            }
            // Unload TTS backends
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

        // Use the listener module which handles OS signals + graceful shutdown
        tama_core::proxy::server::listener::run(app, addr, Some(on_shutdown), None).await
    }

    #[cfg(not(feature = "web-ui"))]
    {
        let server = ProxyServer::new(state.clone()).await;
        server.run(addr, None).await
    }
}
