/// Start the Kronk web control plane UI server.
#[cfg(feature = "web-ui")]
pub async fn cmd_web(
    port: u16,
    proxy_url: String,
    logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;
    koji_web::server::run_with_opts(addr, proxy_url, logs_dir, config_path, None).await
}
