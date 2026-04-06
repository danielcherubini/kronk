//! Run command handler
//!
//! Handles `koji run <server>` for debugging/foreground running.

use anyhow::Result;
use koji_core::config::Config;
use koji_core::process::{ProcessEvent, ProcessSupervisor};

/// Run a single server in the foreground (for debugging)
pub async fn cmd_run(config: &Config, server_name: &str, ctx_override: Option<u32>) -> Result<()> {
    let (server, backend) = config.resolve_server(server_name)?;

    let args = config.build_full_args(server, backend, ctx_override)?;

    // Resolve backend binary path from DB (priority) or config.path (fallback)
    let backend_path = {
        let conn = Config::open_db();
        config.resolve_backend_path(&server.backend, &conn)?
    };
    let backend_path_str = backend_path.to_string_lossy().to_string();

    println!("Starting server...");
    println!();
    println!("  Model:    {}", server_name);
    println!("  Backend:  {}", backend_path.display());
    if let Some(ctx) = ctx_override {
        println!("  Context:  {}", ctx);
    }
    let health_check = config.resolve_health_check(server);
    if let Some(ref url) = health_check.url {
        println!("  Health:   {}", url);
    }
    println!();

    let supervisor = ProcessSupervisor::new(
        backend_path_str,
        args,
        health_check,
        config.supervisor.max_restarts,
        config.supervisor.restart_delay_ms,
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProcessEvent>();

    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                ProcessEvent::Started => println!("[koji] Started."),
                ProcessEvent::Ready => println!("[koji] Ready."),
                ProcessEvent::Output(line) => println!("[backend] {}", line),
                ProcessEvent::Crashed(msg) => eprintln!("[koji] Crashed: {}", msg),
                ProcessEvent::Restarting { attempt, max } => {
                    println!("[koji] Restarting ({}/{})", attempt, max)
                }
                ProcessEvent::Stopped => {
                    println!("[koji] Stopped.")
                }
                ProcessEvent::HealthCheck {
                    alive,
                    healthy,
                    uptime_secs,
                    restarts,
                } => {
                    tracing::debug!(alive, healthy, uptime_secs, restarts, "health check");
                }
            }
        }
    });

    let result = supervisor.run(tx, None).await;
    printer.abort();
    result
}
