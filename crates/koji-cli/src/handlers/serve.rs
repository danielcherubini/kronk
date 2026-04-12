//! Serve command handler
//!
//! Handles `koji serve` for starting the proxy server.

use anyhow::Result;
use koji_core::config::Config;

/// Start the koji server (proxy) with the given host, port, and idle timeout.
pub async fn cmd_serve(config: &Config, host: String, port: u16, idle_timeout: u64) -> Result<()> {
    start_proxy_server(config, host, port, idle_timeout).await
}

/// Start the koji server (proxy) with the given host, port, and idle timeout.
async fn start_proxy_server(
    config: &Config,
    host: String,
    port: u16,
    idle_timeout: u64,
) -> Result<()> {
    use koji_core::proxy::ProxyServer;
    use koji_core::proxy::ProxyState;
    use std::net::SocketAddr;
    use std::sync::Arc;

    // Apply CLI overrides to config
    let mut updated_config = config.clone();
    updated_config.proxy.host = host.clone();
    updated_config.proxy.port = port;
    updated_config.proxy.idle_timeout_secs = idle_timeout;

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

    tracing::info!("Starting koji on {}", addr);
    tracing::info!("Idle timeout: {}s", idle_timeout);

    let db_dir = koji_core::config::Config::config_dir().ok();
    // Trigger backfill if DB is fresh (best-effort: log failures but don't abort)
    if let Some(ref dir) = db_dir {
        match koji_core::db::open(dir) {
            Ok(db_result) => {
                if db_result.needs_backfill {
                    tracing::info!("Running initial backfill...");
                    if let Err(e) = koji_core::db::backfill::run_initial_backfill(
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
                    koji_core::db::backfill::migrate_backend_registry_toml(&db_result.conn, dir)
                {
                    tracing::error!("Backend registry TOML migration failed: {}", e);
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
        let proxy_base_url = format!("http://127.0.0.1:{}", port);
        let logs_dir = updated_config.logs_dir().ok();
        // Ensure logs directory exists (creates if missing)
        if let Some(ref dir) = logs_dir {
            let _ = std::fs::create_dir_all(dir);
        }
        let config_path = koji_core::config::Config::config_path().ok();
        let web_addr: SocketAddr = "0.0.0.0:11435".parse().unwrap();
        tracing::info!("Starting koji web UI on http://{}", web_addr);
        let proxy_config = Some(Arc::clone(&state.config));
        let jobs = std::sync::Arc::new(koji_web::jobs::JobManager::new());
        let capabilities = std::sync::Arc::new(koji_web::api::backends::CapabilitiesCache::new());
        tokio::spawn(async move {
            if let Err(e) = koji_web::server::run_with_opts(
                web_addr,
                proxy_base_url,
                logs_dir,
                config_path,
                proxy_config,
                Some(jobs),
                Some(capabilities),
                env!("CARGO_PKG_VERSION").to_string(),
            )
            .await
            {
                tracing::error!("Web UI server error: {}", e);
            }
        });
    }

    // Create and run proxy server
    let server = ProxyServer::new(state.clone());
    server.run(addr).await?;

    Ok(())
}
