//! Serve command handler
//!
//! Handles `kronk serve` for starting the proxy server.

use anyhow::Result;
use kronk_core::config::Config;

/// Start the kronk server (proxy) with the given host, port, and idle timeout.
pub async fn cmd_serve(config: &Config, host: String, port: u16, idle_timeout: u64) -> Result<()> {
    start_proxy_server(config, host, port, idle_timeout).await
}

/// Start the OpenAI-compliant proxy server (deprecated: use `kronk serve`).
pub async fn cmd_proxy(config: &Config, command: crate::cli::ProxyCommands) -> Result<()> {
    eprintln!("Warning: `kronk proxy start` is deprecated. Use `kronk serve` instead.");

    match command {
        crate::cli::ProxyCommands::Start {
            host,
            port,
            idle_timeout,
        } => start_proxy_server(config, host, port, idle_timeout).await,
    }
}

/// Start the kronk server (proxy) with the given host, port, and idle timeout.
async fn start_proxy_server(
    config: &Config,
    host: String,
    port: u16,
    idle_timeout: u64,
) -> Result<()> {
    use kronk_core::proxy::ProxyServer;
    use kronk_core::proxy::ProxyState;
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

    tracing::info!("Starting Kronk on {}", addr);
    tracing::info!("Idle timeout: {}s", idle_timeout);

    let state = Arc::new(ProxyState::new(updated_config));

    // Create and run proxy server
    let server = ProxyServer::new(state.clone());
    server.run(addr).await?;

    Ok(())
}
