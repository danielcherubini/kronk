//! Run command handler
//!
//! Handles `kronk run <server>` for running a single server in foreground.

use anyhow::Result;
use kronk_core::config::Config;
use kronk_core::process::{ProcessEvent, ProcessSupervisor};

/// Run a single server in the foreground (for debugging)
pub async fn cmd_run(config: &Config, server_name: &str, ctx_override: Option<u32>) -> Result<()> {
    let (server, backend) = config.resolve_server(server_name)?;

    let args = build_full_args(config, server, backend, ctx_override)?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Model:    {}", server_name);
    println!("  Backend:  {}", backend.path);
    if let Some(ctx) = ctx_override {
        println!("  Context:  {}", ctx);
    }
    let health_check = config.resolve_health_check(server);
    if let Some(ref url) = health_check.url {
        println!("  Health:   {}", url);
    }
    println!();

    let supervisor = ProcessSupervisor::new(
        backend.path.clone(),
        args,
        health_check,
        config.supervisor.max_restarts,
        config.supervisor.restart_delay_ms,
    );

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ProcessEvent>();

    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                ProcessEvent::Started => println!("[kronk] Pull the lever!"),
                ProcessEvent::Ready => println!("[kronk] Oh yeah, it's all coming together."),
                ProcessEvent::Output(line) => println!("[backend] {}", line),
                ProcessEvent::Crashed(msg) => eprintln!("[kronk] WRONG LEVER! {}", msg),
                ProcessEvent::Restarting { attempt, max } => {
                    println!(
                        "[kronk] Why do we even have that lever? Restarting ({}/{})",
                        attempt, max
                    )
                }
                ProcessEvent::Stopped => {
                    println!("[kronk] By all accounts, it doesn't make sense.")
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

/// Build the full argument list for a server, resolving model card args at runtime.
fn build_full_args(
    config: &Config,
    server: &kronk_core::config::ModelConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> Result<Vec<String>> {
    config.build_full_args(server, backend, ctx_override)
}
