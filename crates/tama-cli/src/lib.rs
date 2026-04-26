//! Tama CLI library
//!
//! This library provides the core functionality for the tama CLI application.
//!
//! ## Model Card-Based Configuration
//!
//! Tama uses model cards to store quantization info, context settings, and sampling presets
//! for each model. Model cards are stored in `~/.config/tama/configs/<company>--<model>.toml`
//! and are automatically discovered when models are installed.

pub mod cli;
pub mod commands;
pub mod flags;
pub mod handlers;
pub mod service;

// Re-exports for integration tests
pub use flags::extract_tama_flags;
pub use handlers::server::{cmd_server_add, cmd_server_edit};

use crate::commands::tts::TtsArgs;

use anyhow::Result;
use clap::Parser;
use cli::{Args, Commands};
use handlers::{config, profile, run, serve, server, service_cmd, status};
#[cfg(target_os = "windows")]
use service::service_dispatch;
use tama_core::config::Config;

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

    let args = Args::parse();
    let mut config = Config::load()?;

    // For serve/service-run --proxy, use file logging. Otherwise use stdout.
    let use_file_logging = matches!(
        args.command,
        Commands::Serve { .. } | Commands::ServiceRun { proxy: true, .. }
    );

    if use_file_logging {
        if let Ok(logs_dir) = config.logs_dir() {
            if let Err(e) = tama_core::logging::init_with_file(&logs_dir) {
                eprintln!("Failed to initialize file logging: {}", e);
                tama_core::logging::init(); // Fallback to stdout
            }
        } else {
            tama_core::logging::init();
        }
    } else {
        tama_core::logging::init();
    }

    match args.command {
        Commands::Run { name, ctx } => run::cmd_run(&config, &name, ctx).await,
        Commands::Service { command } => service_cmd::cmd_service(&config, command),
        Commands::ServiceRun { server, ctx, proxy } => {
            if proxy {
                let host = config.proxy.host.clone();
                let port = config.proxy.port;
                let auto_unload = config.proxy.auto_unload;
                let idle_timeout = config.proxy.idle_timeout_secs;
                serve::cmd_serve(&config, host, port, auto_unload, idle_timeout).await
            } else {
                let server = server.ok_or_else(|| {
                    anyhow::anyhow!("Either --server or --proxy must be provided for service-run")
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
            commands::backend::run(&config, crate::commands::backend::BackendArgs { command }).await
        }
        Commands::Tts { command } => commands::tts::run(TtsArgs { command }).await,
        Commands::Bench {
            name,
            all,
            pp,
            tg,
            runs,
            warmup,
            ctx,
        } => handlers::bench::cmd_bench(&config, name, all, pp, tg, runs, warmup, ctx).await,
        Commands::SelfUpdate { check, force } => {
            handlers::self_update::cmd_self_update(check, force).await
        }
        Commands::Serve {
            host,
            port,
            auto_unload,
            idle_timeout,
        } => {
            serve::cmd_serve(
                &config,
                host,
                port,
                auto_unload.unwrap_or(config.proxy.auto_unload),
                idle_timeout,
            )
            .await
        }
        Commands::Logs {
            name,
            follow,
            lines,
        } => {
            let name = name.unwrap_or_else(|| "tama".to_string());
            crate::handlers::logs::cmd_logs(&config, &name, follow, lines).await
        }
        Commands::Backup { output, dry_run } => {
            commands::backup::cmd_backup(&config, output, dry_run)
        }
        Commands::Restore {
            archive,
            select,
            dry_run,
            skip_backends,
            skip_models,
        } => {
            use crate::commands::backup::RestoreArgs;
            let args = RestoreArgs {
                archive,
                select,
                dry_run,
                skip_backends,
                skip_models,
            };
            commands::backup::cmd_restore(&mut config, args).await
        }
        #[cfg(feature = "web-ui")]
        Commands::Web {
            port,
            proxy_url,
            logs_dir,
            config_path,
        } => handlers::web::cmd_web(port, proxy_url, logs_dir, config_path).await,
    }
}
