//! Kronk CLI library
//!
//! This library provides the core functionality for the kronk CLI application.

pub mod args;
pub mod cli;
pub mod commands;
pub mod flags;
pub mod handlers;
pub mod service;

pub use handlers::server::{cmd_server_add, cmd_server_edit};

use anyhow::Result;
use clap::Parser;
use cli::{Args, Commands};
use handlers::{config, profile, run, serve, server, service_cmd, status};
use kronk_core::config::Config;
#[cfg(target_os = "windows")]
use service::{service_dispatch, win_service_main};

/// Extracted flags from command line arguments
pub use cli::ExtractedFlags;

/// Extract kronk-specific flags from command line arguments.
pub use flags::extract_kronk_flags;

/// Main entry point for the CLI
pub async fn main() -> Result<()> {
    // Check if we're being launched by the Windows Service Control Manager.
    // SCM passes "service-run" as the first real argument.
    // Skip logging::init() for service mode — the service sets up file-based logging.
    #[cfg(target_os = "windows")]
    {
        let raw_args: Vec<String> = std::env::args().collect();
        if raw_args.len() > 1 && raw_args[1] == "service-run" {
            return service_dispatch();
        }
    }

    kronk_core::logging::init();

    let args = Args::parse();
    let config = Config::load()?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        match args.command {
            Commands::Run { name, ctx } => run::cmd_run(&config, &name, ctx).await,
            Commands::Service { command } => service_cmd::cmd_service(&config, command),
            Commands::ServiceRun { server, ctx, proxy } => {
                if proxy {
                    let host = config.proxy.host.clone();
                    let port = config.proxy.port;
                    let idle_timeout = config.proxy.idle_timeout_secs;
                    serve::cmd_serve(&config, host, port, idle_timeout).await
                } else {
                    let server = server.ok_or_else(|| {
                        anyhow::anyhow!(
                            "Either --server or --proxy must be provided for service-run"
                        )
                    })?;
                    run::cmd_run(&config, &server, ctx).await
                }
            }
            Commands::Add { name, command } => {
                server::cmd_server_add(&config, &name, command, false).await
            }
            Commands::Update { name, command } => {
                server::cmd_server_edit(&mut config.clone(), &name, command).await
            }
            Commands::Server { command } => server::cmd_server(&config, command).await,
            Commands::Status => status::cmd_status(&config).await,
            Commands::Profile { command } => profile::cmd_profile(&config, command),
            Commands::Config { command } => config::cmd_config(&config, command),
            Commands::Model { command } => commands::model::run(&config, command).await,
            Commands::Backend { command } => {
                commands::backend::run(&config, crate::commands::backend::BackendArgs { command })
                    .await
            }
            Commands::Serve {
                host,
                port,
                idle_timeout,
            } => serve::cmd_serve(&config, host, port, idle_timeout).await,
            Commands::Proxy { command } => serve::cmd_proxy(&config, command).await,
            Commands::Logs {
                name,
                follow,
                lines,
            } => {
                let name = name.unwrap_or_else(|| "kronk".to_string());
                crate::handlers::logs::cmd_logs(&config, &name, follow, lines).await
            }
        }
    })
}
