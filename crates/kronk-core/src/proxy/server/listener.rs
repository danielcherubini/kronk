use axum::Router;
use std::net::SocketAddr;
use tracing::info;

/// Start the proxy server on the given address.
///
/// Binds a TCP listener and serves the provided router until shutdown.
pub async fn run(app: Router, addr: SocketAddr) -> anyhow::Result<()> {
    info!("Starting proxy server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
