/// Start the Tama web control plane UI server.
#[cfg(feature = "web-ui")]
pub async fn cmd_web(
    port: u16,
    _proxy_url: String,
    _logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;

    // Build config from config_path or default
    let config = if let Some(ref cp) = config_path {
        let config_dir = cp.parent().map(|p| p.to_path_buf());
        if let Some(cd) = config_dir {
            tama_core::config::Config::load_from(&cd).unwrap_or_default()
        } else {
            tama_core::config::Config::default()
        }
    } else {
        tama_core::config::Config::default()
    };

    // Derive db_dir from config_path
    let db_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf());

    let state = Arc::new(tama_core::proxy::ProxyState::new(config, db_dir));

    // Set web-specific fields
    let mut state_inner = (*state).clone();
    state_inner.web_jobs = Some(Arc::new(tama_core::web_types::JobManager::new()));
    state_inner.web_capabilities = Some(Arc::new(tama_core::web_types::CapabilitiesCache::new()));
    state_inner.web_binary_version = env!("CARGO_PKG_VERSION").to_string();
    let state = Arc::new(state_inner);

    let jobs_for_shutdown = state.web_jobs.clone();
    let app = tama_web::router::build_web_routes().with_state(state);
    tracing::info!("Tama web UI listening on http://{}", addr);

    // Use axum-server for timeout-based graceful shutdown.
    let handle = axum_server::Handle::new();
    let shutdown_handle = handle.clone();

    tokio::spawn(async move {
        // Wait for Ctrl+C in standalone mode
        tokio::signal::ctrl_c().await.ok();
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

    // axum_server::from_tcp takes a std::net::TcpListener
    let std_listener = std::net::TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;

    let result = axum_server::from_tcp(std_listener)
        .handle(handle)
        .serve(app.into_make_service())
        .await;

    result?;
    Ok(())
}
