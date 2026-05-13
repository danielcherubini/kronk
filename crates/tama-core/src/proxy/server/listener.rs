use axum::Router;
use axum_server::Handle;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::time::Duration;
use tracing::info;

/// Timeout for forcing connections closed during graceful shutdown.
/// SSE streams (metrics, jobs, downloads) can hold connections open
/// indefinitely. This timeout ensures shutdown completes promptly.
const SHUTDOWN_TIMEOUT_SECS: u64 = 5;

/// Start the proxy server on the given address.
///
/// Binds a TCP listener and serves the provided router until shutdown.
/// Handles SIGTERM/SIGINT for graceful shutdown.
/// Optionally runs a cleanup future before exiting.
/// If `shutdown_tx` is provided, the signal is broadcast to other servers
/// (e.g. the web UI) so they shut down simultaneously.
pub async fn run(
    app: Router,
    addr: SocketAddr,
    on_shutdown: Option<impl std::future::Future<Output = ()> + Send + 'static>,
    shutdown_tx: Option<tokio::sync::watch::Sender<()>>,
) -> anyhow::Result<()> {
    info!("Starting proxy server on {}", addr);

    // axum_server::from_tcp takes a std::net::TcpListener
    let std_listener = StdTcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;

    let handle = Handle::new();

    // Spawn a task that listens for shutdown signals and triggers cleanup
    // + graceful shutdown with timeout.
    let shutdown_handle = handle.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM, shutting down...");
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT, shutting down...");
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Received Ctrl+C, shutting down...");
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
            info!("Received Ctrl+C, shutting down...");
        }

        // Broadcast shutdown to other servers (e.g. web UI)
        if let Some(tx) = shutdown_tx {
            let _ = tx.send(());
        }

        // Run optional cleanup (e.g. unload TTS backends)
        if let Some(cleanup) = on_shutdown {
            cleanup.await;
        }

        // Initiate graceful shutdown with timeout.
        // axum-server will close all connections after SHUTDOWN_TIMEOUT_SECS.
        info!(
            "Initiating graceful shutdown (timeout: {}s)...",
            SHUTDOWN_TIMEOUT_SECS
        );
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(SHUTDOWN_TIMEOUT_SECS)));
    });

    // Run the server using axum-server (supports timeout-based graceful shutdown)
    let result = axum_server::from_tcp(std_listener)
        .handle(handle)
        .serve(app.into_make_service())
        .await;

    result?;
    info!("Server shutdown complete");
    Ok(())
}
