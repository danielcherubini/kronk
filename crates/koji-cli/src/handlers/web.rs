/// Start the Koji web control plane UI server.
#[cfg(feature = "web-ui")]
pub async fn cmd_web(
    port: u16,
    proxy_url: String,
    logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use std::sync::Arc;
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;
    let jobs = Arc::new(koji_web::jobs::JobManager::new());
    let capabilities = Arc::new(koji_web::api::backends::CapabilitiesCache::new());
    koji_web::server::run_with_opts(
        addr,
        proxy_url,
        logs_dir,
        config_path,
        None,
        Some(jobs),
        Some(capabilities),
        env!("CARGO_PKG_VERSION").to_string(),
    )
    .await
}
