/// Start the Tama web control plane UI server.
#[cfg(feature = "web-ui")]
pub async fn cmd_web(
    port: u16,
    proxy_url: String,
    logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use std::sync::Arc;
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;
    let jobs = Arc::new(tama_core::web_types::JobManager::new());
    let capabilities = Arc::new(tama_core::web_types::CapabilitiesCache::new());
    tama_web::server::run_with_opts(
        addr,
        proxy_url,
        logs_dir,
        config_path,
        None,
        Some(jobs),
        Some(capabilities),
        env!("CARGO_PKG_VERSION").to_string(),
        None,
        None, // standalone — web UI registers its own signal handlers
    )
    .await
}
