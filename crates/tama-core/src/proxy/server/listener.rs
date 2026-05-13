use axum::Router;
use std::net::SocketAddr;
use tracing::info;

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

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Create a future that completes when we receive SIGTERM or SIGINT.
    // Broadcasts the shutdown signal to any subscribed servers.
    let shutdown_signal = async move {
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
    };

    // Run the server with graceful shutdown and optional cleanup
    let app = if let Some(cleanup) = on_shutdown {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown_signal.await;
                cleanup.await;
            })
            .await
    } else {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal)
            .await
    };

    app?;
    info!("Server shutdown complete");
    Ok(())
}
