use anyhow::{Context, Result};
use clap::Parser;
use kronk_core::config::Config;
use kronk_core::logging;
use kronk_core::process::{ProcessEvent, ProcessSupervisor};
use kronk_core::profiles::SamplingParams;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

mod args;
mod commands;
use commands::backend::{BackendArgs, BackendSubcommand};

#[derive(Parser, Debug)]
#[command(name = "kronk")]
#[command(version)]
#[command(about = "Oh yeah, it's all coming together. -- Local AI Service Manager")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Pull the lever! Run a server in the foreground
    Run {
        /// Server name (required)
        name: String,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
    },
    /// Manage Windows services
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },
    /// Internal: called by Windows SCM (do not use directly)
    #[command(hide = true)]
    ServiceRun {
        #[arg(short, long)]
        server: String,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
    },
    /// Add a new server from a raw command line
    #[command(hide = true)]
    Add {
        /// Server name
        name: String,
        /// The full command: binary path followed by all arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Update an existing server with a new command line
    #[command(hide = true)]
    Update {
        /// Server name
        name: String,
        /// The full command: binary path followed by all arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Manage servers — list, add, edit, remove
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
    /// Show status of all servers
    Status,
    /// Manage sampling profiles — presets for inference params
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },
    /// View or edit configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Manage models — pull, list, create servers
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
    /// Manage backends — install, update, list, remove
    Backend {
        #[command(subcommand)]
        command: BackendSubcommand,
    },
    /// OpenAI-compliant proxy for local AI models
    Proxy {
        /// Proxy settings
        #[command(subcommand)]
        command: ProxyCommands,
    },
    /// View server logs
    Logs {
        /// Server name
        name: String,
        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
}

#[derive(Parser, Debug)]
pub enum ProxyCommands {
    /// Start the proxy server
    Start {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(long, default_value = "11434")]
        port: u16,
        /// Idle timeout in seconds (models unload after this many seconds of inactivity)
        #[arg(long, default_value = "300")]
        idle_timeout: u64,
    },
}

#[derive(Parser, Debug)]
pub enum ModelCommands {
    /// Pull a model from HuggingFace
    Pull {
        /// HuggingFace repo ID, e.g. "bartowski/OmniCoder-8B-GGUF"
        repo: String,
    },
    /// List installed models
    Ls,
    /// Show running model processes
    Ps,
    /// Create a server from an installed model
    Create {
        /// Server name to create
        name: String,
        /// Model ID in "company/modelname" format
        #[arg(long)]
        model: String,
        /// Quant to use (e.g. "Q4_K_M"). Interactive picker if omitted.
        #[arg(long)]
        quant: Option<String>,
        /// Sampling profile: coding, chat, analysis, creative
        #[arg(long)]
        profile: Option<String>,
        /// Backend to use. Interactive picker if omitted and multiple exist.
        #[arg(long)]
        backend: Option<String>,
    },
    /// Remove an installed model
    Rm {
        /// Model ID in "company/modelname" format
        model: String,
    },
    /// Scan for untracked GGUF files and update model cards
    Scan,
    /// Search HuggingFace for GGUF models
    Search {
        /// Search query (e.g. "llama", "coding", "mistral 7b")
        query: String,
        /// Sort by: downloads, likes, modified (default: downloads)
        #[arg(long, default_value = "downloads")]
        sort: String,
        /// Maximum number of results (default: 20)
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,
        /// Immediately pull a selected result
        #[arg(long)]
        pull: bool,
    },
}

#[derive(Parser, Debug)]
pub enum ServerCommands {
    /// List all servers with status
    Ls,
    /// Add a new server from a raw command line
    Add {
        /// Server name
        name: String,
        /// Backend command and arguments (e.g. llama-server -m model.gguf)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Edit an existing server's command line
    Edit {
        /// Server name
        name: String,
        /// New backend command and arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Remove a server
    Rm {
        /// Server name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

#[derive(Parser, Debug)]
enum ProfileCommands {
    /// List all available profiles and their sampling params
    List,
    /// Set a server's sampling profile
    Set {
        /// Server name
        server: String,
        /// Profile name: coding, chat, analysis, creative, or a custom name
        profile: String,
    },
    /// Clear a server's sampling profile (remove sampling preset)
    Clear {
        /// Server name
        server: String,
    },
    /// Create a custom profile with specific sampling params
    Add {
        /// Custom profile name
        name: String,
        #[arg(long)]
        temp: Option<f64>,
        #[arg(long)]
        top_k: Option<u32>,
        #[arg(long)]
        top_p: Option<f64>,
        #[arg(long)]
        min_p: Option<f64>,
        #[arg(long)]
        presence_penalty: Option<f64>,
        #[arg(long)]
        frequency_penalty: Option<f64>,
        #[arg(long)]
        repeat_penalty: Option<f64>,
    },
    /// Remove a custom profile
    Remove {
        /// Custom profile name
        name: String,
    },
}

#[derive(Parser, Debug)]
enum ServiceCommands {
    /// Install server(s) as system service(s)
    Install {
        /// Server name (omit to install all enabled servers)
        name: Option<String>,
    },
    /// Start an installed service
    Start {
        /// Server name (omit to start all enabled servers)
        name: Option<String>,
    },
    /// Stop a running service
    Stop {
        /// Server name (omit to stop all enabled servers)
        name: Option<String>,
    },
    /// Remove an installed service
    Remove {
        /// Server name (omit to remove all enabled servers)
        name: Option<String>,
    },
}

#[derive(Parser, Debug)]
enum ConfigCommands {
    /// Print the current configuration
    Show,
    /// Open config file in editor
    Edit,
    /// Show the config file path
    Path,
}

fn main() -> Result<()> {
    // Check if we're being launched by the Windows Service Control Manager.
    // SCM passes "service-run" as the first real argument.
    let raw_args: Vec<String> = std::env::args().collect();
    if raw_args.len() > 1 && raw_args[1] == "service-run" {
        #[cfg(target_os = "windows")]
        return service_dispatch();

        #[cfg(not(target_os = "windows"))]
        anyhow::bail!("Windows service mode is only available on Windows");
    }

    let args = Args::parse();
    let config = Config::load()?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        match args.command {
            Commands::Run { name, ctx } => cmd_run(&config, &name, ctx).await,
            Commands::Service { command } => cmd_service(&config, command),
            Commands::ServiceRun { server, ctx } => cmd_run(&config, &server, ctx).await,
            Commands::Add { name, command } => cmd_server_add(&config, &name, command, false).await,
            Commands::Update { name, command } => {
                cmd_server_edit(&mut config.clone(), &name, command).await
            }
            Commands::Server { command } => cmd_server(&config, command).await,
            Commands::Status => cmd_status(&config).await,
            Commands::Profile { command } => cmd_profile(&config, command),
            Commands::Config { command } => cmd_config(&config, command),
            Commands::Model { command } => commands::model::run(&config, command).await,
            Commands::Backend { command } => {
                commands::backend::run(&config, BackendArgs { command }).await
            }
            Commands::Proxy { command } => cmd_proxy(&config, command).await,
            Commands::Logs {
                name,
                follow,
                lines,
            } => cmd_logs(&config, &name, follow, lines).await,
        }
    })
}

async fn cmd_logs(config: &Config, name: &str, follow: bool, lines: usize) -> Result<()> {
    let logs_dir = config.logs_dir()?;
    let log_path = logging::log_path(&logs_dir, name);

    if !log_path.exists() {
        println!("No logs found for server '{}'.", name);
        println!();
        println!("Logs are created when running as a service.");
        println!("For foreground: kronk run {}", name);
        return Ok(());
    }

    // Print last N lines
    let tail = logging::tail_lines(&log_path, lines)?;
    for line in &tail {
        println!("{}", line);
    }

    if follow {
        // Poll for new content
        let mut file = std::fs::File::open(&log_path)?;
        file.seek(SeekFrom::End(0))?;
        let mut reader = BufReader::new(file);
        let mut tick = interval(Duration::from_millis(250));

        loop {
            tick.tick().await;
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                print!("{}", line);
                line.clear();
            }
        }
    }

    Ok(())
}

// ── Windows Service Dispatch ─────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn service_dispatch() -> Result<()> {
    // Extract server name and config-dir from args before SCM takes over
    let raw_args: Vec<String> = std::env::args().collect();
    let server = raw_args
        .iter()
        .position(|a| a == "--server")
        .and_then(|i| raw_args.get(i + 1))
        .cloned()
        .expect("Missing --server argument. This binary should be launched by the Windows Service Control Manager.");

    let config_dir = raw_args
        .iter()
        .position(|a| a == "--config-dir")
        .and_then(|i| raw_args.get(i + 1))
        .map(|s| std::path::PathBuf::from(s));

    let ctx = raw_args
        .iter()
        .position(|a| a == "--ctx")
        .and_then(|i| raw_args.get(i + 1))
        .and_then(|s| s.parse().ok());

    let service_name = Config::service_name(&server);

    // Store in globals so service_main can access them
    SERVICE_SERVER
        .set(server)
        .map_err(|_| anyhow::anyhow!("Failed to set service server"))?;
    SERVICE_NAME
        .set(service_name.clone())
        .map_err(|_| anyhow::anyhow!("Failed to set service name"))?;
    SERVICE_CONFIG_DIR
        .set(config_dir)
        .map_err(|_| anyhow::anyhow!("Failed to set service config dir"))?;
    SERVICE_CTX
        .set(ctx)
        .map_err(|_| anyhow::anyhow!("Failed to set service ctx"))?;

    windows_service::service_dispatcher::start(&service_name, ffi_service_main)
        .context("Failed to start service dispatcher — is this running as a Windows Service?")?;

    Ok(())
}

#[cfg(target_os = "windows")]
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
static SERVICE_SERVER: OnceLock<String> = OnceLock::new();
#[cfg(target_os = "windows")]
static SERVICE_NAME: OnceLock<String> = OnceLock::new();
#[cfg(target_os = "windows")]
static SERVICE_CONFIG_DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
#[cfg(target_os = "windows")]
static SERVICE_CTX: OnceLock<Option<u32>> = OnceLock::new();

#[cfg(target_os = "windows")]
windows_service::define_windows_service!(ffi_service_main, win_service_main);

#[cfg(target_os = "windows")]
fn win_service_main(_arguments: Vec<std::ffi::OsString>) {
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler;

    let server = SERVICE_SERVER.get().cloned().unwrap_or_default();
    let service_name = SERVICE_NAME.get().cloned().unwrap_or_default();
    let config_dir = SERVICE_CONFIG_DIR.get().and_then(|o| o.clone());
    let ctx = SERVICE_CTX.get().and_then(|o| *o);

    // Set up logging to file — use config_dir if available, otherwise fall back
    let log_dir = config_dir.clone().unwrap_or_else(|| {
        directories::ProjectDirs::from("", "", "kronk")
            .map(|p: directories::ProjectDirs| p.data_dir().to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    });
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = std::fs::File::create(log_dir.join(format!("{}.log", service_name)))
        .unwrap_or_else(|_| std::fs::File::create("kronk-service.log").unwrap());

    tracing::info!(
        "Service starting for server: {}, config dir: {:?}",
        server,
        config_dir.as_ref().map(|d| d.display())
    );

    // Create a shutdown channel
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();

    // Register the control handler
    let shutdown_sender = shutdown_tx.clone();
    let event_handler =
        move |control_event| -> service_control_handler::ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    tracing::info!("Received stop/shutdown signal");
                    shutdown_sender.send(()).ok();
                    service_control_handler::ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => {
                    service_control_handler::ServiceControlHandlerResult::NoError
                }
                _ => service_control_handler::ServiceControlHandlerResult::NotImplemented,
            }
        };

    let status_handle = match service_control_handler::register(&service_name, event_handler) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("Failed to register control handler: {}", e);
            return;
        }
    };

    // Report running
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
        process_id: None,
    });

    // Load config — use explicit config dir if provided (service runs as SYSTEM)
    let config = match if let Some(ref dir) = config_dir {
        Config::load_from(dir)
    } else {
        Config::load()
    } {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(
                "Failed to load config: {}, config dir: {:?}",
                e,
                config_dir.as_ref().map(|d| d.display())
            );
            let _ = status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(1),
                checkpoint: 0,
                wait_hint: std::time::Duration::default(),
                process_id: None,
            });
            return;
        }
    };

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    rt.block_on(async {
        let (srv, backend) = match config.resolve_server(&server) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to resolve server '{}': {}", server, e);
                return;
            }
        };

        let args = build_full_args(&config, srv, backend, ctx).unwrap_or_else(|e| {
            tracing::warn!("Failed to build model args: {}", e);
            let mut args = backend.default_args.clone();
            args.extend(srv.args.clone());
            args
        });
        let log_dir = config
            .logs_dir()
            .ok()
            .expect("Failed to get logs directory");
        let health_check = config.resolve_health_check(&srv);
        let supervisor = ProcessSupervisor::new(
            backend.path.clone(),
            args,
            health_check,
            config.supervisor.max_restarts,
            config.supervisor.restart_delay_ms,
        )
        .with_log_dir(log_dir);

        let (tx, mut rx) = mpsc::unbounded_channel::<ProcessEvent>();

        // Create a tokio shutdown channel bridged from the std channel
        let (shutdown_tx_tokio, shutdown_rx_tokio) = mpsc::channel::<()>(1);
        tokio::task::spawn_blocking(move || {
            let _ = shutdown_rx.recv();
            let _ = shutdown_tx_tokio.blocking_send(());
        });

        // Log events
        let logger = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match &event {
                    ProcessEvent::Started => tracing::info!("Backend process started"),
                    ProcessEvent::Ready => tracing::info!("Backend server ready"),
                    ProcessEvent::Output(line) => tracing::info!("[backend] {}", line),
                    ProcessEvent::Crashed(msg) => tracing::warn!("Backend crashed: {}", msg),
                    ProcessEvent::Restarting { attempt, max } => {
                        tracing::info!("Restarting backend ({}/{})", attempt, max)
                    }
                    ProcessEvent::Stopped => tracing::info!("Backend stopped"),
                    ProcessEvent::HealthCheck {
                        healthy,
                        uptime_secs,
                        ..
                    } => {
                        tracing::debug!("Health: healthy={}, uptime={}s", healthy, uptime_secs)
                    }
                }
            }
        });

        // Run supervisor — it will exit when shutdown signal is received
        if let Err(e) = supervisor.run(tx, Some(shutdown_rx_tokio)).await {
            tracing::error!("Supervisor error: {}", e);
        }

        tracing::info!("Shutting down...");
        logger.abort();
    });

    // Report stopped
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
        process_id: None,
    });

    tracing::info!("Service stopped");
}

// ── CLI Commands ─────────────────────────────────────────────────────────

/// Build the full argument list for a server, resolving model card args at runtime.
/// Merges: backend.default_args + server.args + model card (-m, -c, -ngl) + sampling
fn build_full_args(
    config: &Config,
    server: &kronk_core::config::ServerConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> Result<Vec<String>> {
    let mut args = backend.default_args.clone();
    args.extend(server.args.clone());

    // Inject model card args: -m, -c, -ngl
    if let (Some(ref model_id), Some(ref quant_name)) = (&server.model, &server.quant) {
        let models_dir = config.models_dir()?;
        let configs_dir = config.configs_dir()?;
        let registry = kronk_core::models::ModelRegistry::new(models_dir, configs_dir);
        if let Some(installed) = registry.find(model_id)? {
            if let Some(q) = installed.card.quants.get(quant_name) {
                if !args.iter().any(|a| a == "-m" || a == "--model") {
                    args.push("-m".to_string());
                    args.push(installed.dir.join(&q.file).to_string_lossy().to_string());
                }
            }
            // Context size: CLI override > model card
            let ctx = ctx_override.or_else(|| installed.card.context_length_for(quant_name));
            if let Some(ctx) = ctx {
                if !args.iter().any(|a| a == "-c" || a == "--ctx-size") {
                    args.push("-c".to_string());
                    args.push(ctx.to_string());
                }
            }
            if let Some(ngl) = installed.card.model.default_gpu_layers {
                if !args.iter().any(|a| a == "-ngl" || a == "--n-gpu-layers") {
                    args.push("-ngl".to_string());
                    args.push(ngl.to_string());
                }
            }

            // 3-layer sampling merge
            if let Some(sampling) =
                config.effective_sampling_with_card(server, Some(&installed.card))
            {
                args.extend(sampling.to_args());
            }

            return Ok(args);
        }
    }

    // No model card — still apply ctx override if given
    if let Some(ctx) = ctx_override {
        args::inject_context_size(&mut args, ctx);
    }

    // No model card — just use server sampling
    if let Some(sampling) = config.effective_sampling_with_card(server, None) {
        args.extend(sampling.to_args());
    }

    Ok(args)
}

async fn cmd_run(config: &Config, server_name: &str, ctx_override: Option<u32>) -> Result<()> {
    let (server, backend) = config.resolve_server(server_name)?;

    let args = build_full_args(config, server, backend, ctx_override)?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Server:   {}", server_name);
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

    let (tx, mut rx) = mpsc::unbounded_channel::<ProcessEvent>();

    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                ProcessEvent::Started => println!("[kronk] Pull the lever!"),
                ProcessEvent::Ready => println!("[kronk] Oh yeah, it's all coming together."),
                ProcessEvent::Output(line) => println!("[server] {}", line),
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

    supervisor.run(tx, None).await?;
    printer.abort();
    Ok(())
}

/// Resolve server names: if given, use that one; if None, use all enabled.
fn resolve_server_names(config: &Config, name: Option<String>) -> Result<Vec<String>> {
    match name {
        Some(n) => {
            config.resolve_server(&n)?;
            Ok(vec![n])
        }
        None => {
            let enabled: Vec<String> = config
                .servers
                .iter()
                .filter(|(_, s)| s.enabled)
                .map(|(n, _)| n.clone())
                .collect();
            if enabled.is_empty() {
                anyhow::bail!("No enabled servers. Enable one with `kronk config edit`.");
            }
            Ok(enabled)
        }
    }
}

fn cmd_service(config: &Config, command: ServiceCommands) -> Result<()> {
    match command {
        ServiceCommands::Install { name } => {
            let names = resolve_server_names(config, name)?;
            for server_name in &names {
                let (srv, backend) = config.resolve_server(server_name)?;
                let service_name = Config::service_name(server_name);

                #[cfg(target_os = "windows")]
                {
                    let display_name = format!("Kronk: {}", server_name);
                    let config_dir = Config::base_dir()?;
                    let port = srv.port.unwrap_or(8080);
                    kronk_core::platform::windows::install_service(
                        &service_name,
                        &display_name,
                        server_name,
                        &config_dir,
                        port,
                    )?;
                }

                #[cfg(target_os = "linux")]
                {
                    let args = build_full_args(config, srv, backend, None)?;
                    let port = srv.port.unwrap_or(8080);
                    kronk_core::platform::linux::install_service(
                        &service_name,
                        &backend.path,
                        &args,
                        port,
                    )?;
                }

                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    let _ = (srv, backend);
                    anyhow::bail!("Service management not supported on this platform");
                }

                println!("Installed service for server '{}'.", server_name);
            }
        }
        ServiceCommands::Start { name } => {
            let names = resolve_server_names(config, name)?;
            for server_name in &names {
                let service_name = Config::service_name(server_name);
                service_start_inner(&service_name)?;
                println!("Pull the lever! '{}' started.", service_name);
            }
        }
        ServiceCommands::Stop { name } => {
            let names = resolve_server_names(config, name)?;
            for server_name in &names {
                let service_name = Config::service_name(server_name);
                service_stop_inner(&service_name)?;
                println!("Wrong lever! '{}' stopped.", service_name);
            }
        }
        ServiceCommands::Remove { name } => {
            let names = resolve_server_names(config, name)?;
            for server_name in &names {
                let service_name = Config::service_name(server_name);

                #[cfg(target_os = "windows")]
                kronk_core::platform::windows::remove_service(&service_name)?;

                #[cfg(target_os = "linux")]
                kronk_core::platform::linux::remove_service(&service_name)?;

                println!("No touchy! '{}' removed.", service_name);
            }
        }
    }
    Ok(())
}

fn service_start_inner(service_name: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    kronk_core::platform::windows::start_service(service_name)?;

    #[cfg(target_os = "linux")]
    kronk_core::platform::linux::start_service(service_name)?;

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = service_name;
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

fn service_stop_inner(service_name: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    kronk_core::platform::windows::stop_service(service_name)?;

    #[cfg(target_os = "linux")]
    kronk_core::platform::linux::stop_service(service_name)?;

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = service_name;
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

async fn cmd_status(config: &Config) -> Result<()> {
    println!("KRONK Status");
    println!("{}", "-".repeat(60));

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    for (name, srv) in &config.servers {
        let _backend = config.backends.get(&srv.backend);
        let backend_path = _backend.map(|b| b.path.as_str()).unwrap_or("???");

        // Check service status
        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            {
                use kronk_core::platform::windows;
                windows::query_service(&service_name).unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(target_os = "linux")]
            {
                use kronk_core::platform::linux;
                linux::query_service(&service_name).unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = &service_name;
                "N/A".to_string()
            }
        };

        // Check health endpoint using server's resolved health check config
        let health_check = config.resolve_health_check(srv);
        let health = if let Some(url) = health_check.url {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY".to_string(),
                Ok(resp) => format!("HTTP {}", resp.status()),
                Err(_) => "DOWN".to_string(),
            }
        } else {
            "N/A".to_string()
        };

        println!();
        println!("  Server:   {}", name);
        println!("  Backend:  {} ({})", srv.backend, backend_path);
        println!("  Service:  {}", service_status);
        println!("  Health:   {}", health);
    }

    // GPU VRAM usage
    if let Some(vram) = kronk_core::gpu::query_vram() {
        println!();
        println!("  VRAM:     {} / {} MiB", vram.used_mib, vram.total_mib);
    }

    println!();
    Ok(())
}

async fn cmd_server(config: &Config, command: ServerCommands) -> Result<()> {
    match command {
        ServerCommands::Ls => cmd_server_ls(config).await,
        ServerCommands::Add { name, command } => {
            cmd_server_add(config, &name, command, false).await
        }
        ServerCommands::Edit { name, command } => {
            if !config.servers.contains_key(&name) {
                anyhow::bail!(
                    "Server '{}' not found. Use `kronk server add` to create it.",
                    name
                );
            }
            cmd_server_edit(&mut config.clone(), &name, command).await
        }
        ServerCommands::Rm { name, force } => cmd_server_rm(config, &name, force),
    }
}

async fn cmd_server_ls(config: &Config) -> Result<()> {
    if config.servers.is_empty() {
        println!("No servers configured.");
        println!();
        println!("Add one:  kronk server add <name> <command...>");
        println!("Or pull:  kronk model pull <repo>");
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    println!("Servers:");
    println!("{}", "-".repeat(60));

    for (name, srv) in &config.servers {
        let _backend = config.backends.get(&srv.backend);
        let profile_name = srv
            .profile
            .as_ref()
            .map(|p| p.to_string())
            .unwrap_or_else(|| "none".to_string());

        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            {
                kronk_core::platform::windows::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(target_os = "linux")]
            {
                kronk_core::platform::linux::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = &service_name;
                "N/A".to_string()
            }
        };

        // Use server's resolved health check config
        let health_check = config.resolve_health_check(srv);
        let health = if let Some(url) = health_check.url {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY",
                _ => "DOWN",
            }
        } else {
            "N/A"
        };

        println!();
        println!("  {}  (backend: {})", name, srv.backend);
        println!(
            "    profile: {}  service: {}  health: {}",
            profile_name, service_status, health
        );

        if let Some(ref model) = srv.model {
            let quant = srv.quant.as_deref().unwrap_or("?");
            println!("    model: {} / {}", model, quant);
        }

        if !srv.args.is_empty() {
            let args_str = srv.args.join(" ");
            if args_str.len() > 80 {
                let chars: Vec<char> = args_str.chars().take(77).collect();
                println!("    args: {}...", chars.iter().collect::<String>());
            } else {
                println!("    args: {}", args_str);
            }
        }
    }

    println!();
    Ok(())
}

fn cmd_server_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    if !config.servers.contains_key(name) {
        anyhow::bail!("Server '{}' not found.", name);
    }

    // Check if a service is installed for this server
    let service_name = Config::service_name(name);
    let service_installed = {
        #[cfg(target_os = "windows")]
        {
            kronk_core::platform::windows::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(target_os = "linux")]
        {
            kronk_core::platform::linux::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = &service_name;
            false
        }
    };

    if service_installed {
        anyhow::bail!(
            "Server '{}' has an installed service '{}'. Remove it first with: kronk service remove {}",
            name, service_name, name
        );
    }

    if !force {
        let confirm = inquire::Confirm::new(&format!("Remove server '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut config = config.clone();
    config.servers.remove(name);
    config.save()?;

    println!("Server '{}' removed.", name);
    Ok(())
}

async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
) -> Result<()> {
    use kronk_core::config::{BackendConfig, ServerConfig};

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Only absolutize if it looks like a filesystem path (contains separator or starts with ./..);
    // bare command names (e.g. "llama-server") are left as-is so PATH resolution works at runtime.
    let exe_abs = std::path::Path::new(exe_path);
    let is_path = exe_path.contains(std::path::MAIN_SEPARATOR)
        || exe_path.contains('/')
        || exe_path.starts_with('.')
        || exe_abs.is_absolute();
    let (exe_str, exe_stem) = if is_path {
        let resolved = if exe_abs.is_absolute() {
            exe_abs.to_path_buf()
        } else {
            std::env::current_dir()?.join(exe_abs)
        };
        let stem = resolved
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "backend".to_string());
        (resolved.to_string_lossy().to_string(), stem)
    } else {
        let stem = exe_path
            .strip_suffix(".exe")
            .unwrap_or(exe_path)
            .to_string();
        (exe_path.to_string(), stem)
    };

    // Check if this backend path already exists
    let mut config = config.clone();
    let backend_name = config
        .backends
        .iter()
        .find(|(_, b)| b.path == exe_str)
        .map(|(k, _)| k.clone());

    let backend_key = match backend_name {
        Some(k) => k,
        None => {
            // Derive a backend name from the exe filename, avoiding collisions
            let base = exe_stem.replace('-', "_");

            let mut key = base.clone();
            let mut i = 2;
            while config.backends.contains_key(&key) {
                key = format!("{}_{}", base, i);
                i += 1;
            }

            config.backends.insert(
                key.clone(),
                BackendConfig {
                    path: exe_str.clone(),
                    default_args: vec![],
                    health_check_url: Some("http://localhost:8080/health".to_string()),
                },
            );
            key
        }
    };

    // Check for duplicate server
    if config.servers.contains_key(name) && !overwrite {
        anyhow::bail!(
            "Server '{}' already exists. Use `kronk server edit` to modify it.",
            name
        );
    }

    config.servers.insert(
        name.to_string(),
        ServerConfig {
            backend: backend_key.clone(),
            args,
            profile: None,
            sampling: None,
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
        },
    );

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Server:   {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);
    println!();
    println!("Run it:     kronk run {}", name);
    println!("Install it: kronk service install {}", name);

    Ok(())
}

async fn cmd_server_edit(config: &mut Config, name: &str, command: Vec<String>) -> Result<()> {
    use kronk_core::config::BackendConfig;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Only absolutize if it looks like a filesystem path
    let exe_abs = std::path::Path::new(exe_path);
    let is_path = exe_path.contains(std::path::MAIN_SEPARATOR)
        || exe_path.contains('/')
        || exe_path.starts_with('.')
        || exe_abs.is_absolute();
    let (exe_str, exe_stem) = if is_path {
        let resolved = if exe_abs.is_absolute() {
            exe_abs.to_path_buf()
        } else {
            std::env::current_dir()?.join(exe_abs)
        };
        let stem = resolved
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "backend".to_string());
        (resolved.to_string_lossy().to_string(), stem)
    } else {
        let stem = exe_path
            .strip_suffix(".exe")
            .unwrap_or(exe_path)
            .to_string();
        (exe_path.to_string(), stem)
    };

    // Check if this backend path exists
    let backend_name = config
        .backends
        .iter()
        .find(|(_, b)| b.path == exe_str)
        .map(|(k, _)| k.clone());

    let backend_key = match backend_name {
        Some(k) => k,
        None => {
            // Derive a backend name from the exe filename, avoiding collisions
            let base = exe_stem.replace('-', "_");

            let mut key = base.clone();
            let mut i = 2;
            while config.backends.contains_key(&key) {
                key = format!("{}_{}", base, i);
                i += 1;
            }

            config.backends.insert(
                key.clone(),
                BackendConfig {
                    path: exe_str.clone(),
                    default_args: vec![],
                    health_check_url: Some("http://localhost:8080/health".to_string()),
                },
            );
            key
        }
    };

    // Load config, update only the command string for the existing server
    let mut config = config.clone();
    let srv = config
        .servers
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("Server '{}' not found", name))?;

    srv.backend = backend_key.clone();
    srv.args = args;

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Server:   {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);
    println!();
    println!("Run it:     kronk run {}", name);
    println!("Install it: kronk service install {}", name);

    Ok(())
}

fn cmd_config(config: &Config, command: ConfigCommands) -> Result<()> {
    match command {
        ConfigCommands::Show => {
            let toml_str = toml::to_string_pretty(config)?;
            println!("{}", toml_str);
        }
        ConfigCommands::Edit => {
            let path = Config::config_path()?;
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "notepad".to_string());
            std::process::Command::new(&editor)
                .arg(&path)
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to open editor '{}': {}", editor, e))?;
        }
        ConfigCommands::Path => {
            let path = Config::config_path()?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

fn cmd_profile(config: &Config, command: ProfileCommands) -> Result<()> {
    use kronk_core::profiles::Profile;

    match command {
        ProfileCommands::List => {
            // Load profiles from disk
            let profiles_dir = config.profiles_dir()?;
            let disk_profiles =
                kronk_core::profiles::load_profiles_d(&profiles_dir).unwrap_or_default();

            println!("Available profiles:");
            println!();
            for (name, desc, profile) in Profile::all() {
                let params = disk_profiles
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| profile.params());
                println!("  {}:", name);
                println!("    {}", desc);
                println!(
                    "    temp={:.1}  top-k={}  top-p={:.2}  min-p={:.2}  presence-penalty={:.1}",
                    params.temperature.unwrap_or(0.0),
                    params.top_k.unwrap_or(0),
                    params.top_p.unwrap_or(0.0),
                    params.min_p.unwrap_or(0.0),
                    params.presence_penalty.unwrap_or(0.0),
                );
                if disk_profiles.contains_key(name) {
                    println!("    (loaded from profiles.d/{}.toml)", name);
                }
                println!();
            }

            // Show additional custom profiles from disk
            for (name, params) in &disk_profiles {
                if !Profile::all().iter().any(|(n, _, _)| *n == name.as_str()) {
                    println!("  {} (custom from profiles.d/):", name);
                    let args = params.to_args().join(" ");
                    println!(
                        "    {}",
                        if args.is_empty() {
                            "(default params)".to_string()
                        } else {
                            args
                        }
                    );
                    println!();
                }
            }

            // Show custom profiles from config
            if let Some(custom) = &config.custom_profiles {
                if !custom.is_empty() {
                    println!("Custom profiles (from config):");
                    println!();
                    for (name, params) in custom {
                        println!("  {}:", name);
                        let args = params.to_args().join(" ");
                        println!(
                            "    {}",
                            if args.is_empty() {
                                "(default params)".to_string()
                            } else {
                                args
                            }
                        );
                        println!();
                    }
                }
            }

            // Show which servers use which profile
            println!("Server assignments:");
            for (name, srv) in &config.servers {
                let profile_str = srv
                    .profile
                    .as_ref()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "none".to_string());
                println!("  {} -> {}", name, profile_str);
            }

            Ok(())
        }
        ProfileCommands::Set { server, profile } => {
            let mut config = config.clone();

            // Validate server exists
            if !config.servers.contains_key(&server) {
                anyhow::bail!("Server '{}' not found", server);
            }

            // Resolve profile name before mutable borrow
            let resolved = match profile.as_str() {
                "coding" => Profile::Coding,
                "chat" => Profile::Chat,
                "analysis" => Profile::Analysis,
                "creative" => Profile::Creative,
                name => {
                    // Check config custom_profiles and profiles.d/
                    let is_custom_config = config
                        .custom_profiles
                        .as_ref()
                        .map(|m| m.contains_key(name))
                        .unwrap_or(false);
                    let is_custom_disk = config
                        .profiles_dir()
                        .ok()
                        .and_then(|dir| kronk_core::profiles::load_profiles_d(&dir).ok())
                        .map(|m| m.contains_key(name))
                        .unwrap_or(false);
                    if !is_custom_config && !is_custom_disk {
                        anyhow::bail!(
                            "Unknown profile '{}'. Use `kronk profile list` to see available options, \
                             or `kronk profile add {}` to create a custom one.",
                            name, name
                        );
                    }
                    Profile::Custom {
                        name: name.to_string(),
                    }
                }
            };

            config.servers.get_mut(&server).unwrap().profile = Some(resolved);
            config.save()?;

            println!("Oh yeah, it's all coming together.");
            println!("  Server '{}' now uses '{}' preset.", server, profile);

            Ok(())
        }
        ProfileCommands::Clear { server } => {
            let mut config = config.clone();
            let srv = config
                .servers
                .get_mut(&server)
                .with_context(|| format!("Server '{}' not found", server))?;

            srv.profile = None;
            config.save()?;

            println!("Profile cleared for server '{}'.", server);
            Ok(())
        }
        ProfileCommands::Add {
            name,
            temp,
            top_k,
            top_p,
            min_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
        } => {
            let mut config = config.clone();
            let params = SamplingParams {
                temperature: temp,
                top_k,
                top_p,
                min_p,
                presence_penalty,
                frequency_penalty,
                repeat_penalty,
            };

            if params.is_empty() {
                anyhow::bail!("At least one sampling parameter is required. Example:\n  kronk profile add my-preset --temp 0.4 --top-k 30");
            }

            // Reject names that shadow built-in profiles
            const RESERVED: &[&str] = &["coding", "chat", "analysis", "creative"];
            if RESERVED.contains(&name.as_str()) {
                anyhow::bail!(
                    "Cannot create custom profile '{}': it shadows a built-in profile. \
                     Edit profiles.d/{}.toml instead to customize it.",
                    name,
                    name
                );
            }

            let custom = config.custom_profiles.get_or_insert_with(HashMap::new);
            custom.insert(name.clone(), params);
            config.save()?;

            println!("Custom profile '{}' created.", name);
            println!("Assign it: kronk profile set <server> {}", name);
            Ok(())
        }
        ProfileCommands::Remove { name } => {
            let mut config = config.clone();

            let exists = config
                .custom_profiles
                .as_ref()
                .map(|m| m.contains_key(&name))
                .unwrap_or(false);
            if !exists {
                anyhow::bail!("Custom profile '{}' not found", name);
            }

            // Check if any servers reference this profile
            let referencing: Vec<&str> = config
                .servers
                .iter()
                .filter(
                    |(_, s)| matches!(&s.profile, Some(Profile::Custom { name: n }) if n == &name),
                )
                .map(|(k, _)| k.as_str())
                .collect();
            if !referencing.is_empty() {
                anyhow::bail!(
                    "Cannot remove profile '{}': referenced by servers: {}. \
                     Clear them first with `kronk profile clear <server>`.",
                    name,
                    referencing.join(", ")
                );
            }

            config.custom_profiles.as_mut().unwrap().remove(&name);
            config.save()?;
            println!("Custom profile '{}' removed.", name);
            Ok(())
        }
    }
}

/// Start the OpenAI-compliant proxy server
async fn cmd_proxy(config: &Config, command: ProxyCommands) -> Result<()> {
    use kronk_core::proxy::server::ProxyServer;
    use kronk_core::proxy::ProxyState;
    use std::net::SocketAddr;
    use std::sync::Arc;

    let ProxyCommands::Start {
        host,
        port,
        idle_timeout,
    } = command;

    // Apply CLI overrides to config
    let mut proxy_config = config.proxy.clone();
    proxy_config.host = host.clone();
    proxy_config.port = port;
    proxy_config.idle_timeout_secs = idle_timeout;

    // Parse host and port
    let addr = SocketAddr::new(
        std::net::IpAddr::V4(
            host.parse()
                .unwrap_or(std::net::Ipv4Addr::new(127, 0, 0, 1)),
        ),
        port,
    );

    tracing::info!("Starting Kronk Proxy on {}", addr);
    tracing::info!("Idle timeout: {}s", idle_timeout);
    tracing::info!("Use `kronk proxy start --help` for more options");

    // Create BackendRegistry for dynamic model loading
    let registry = {
        let _base_dir = std::env::current_dir().context("Failed to get current directory")?;
        let mut data = kronk_core::backends::registry::RegistryData::default();
        for (name, backend) in &config.backends {
            data.backends.insert(
                name.clone(),
                kronk_core::backends::registry::BackendInfo {
                    name: name.clone(),
                    backend_type: kronk_core::backends::registry::BackendType::Custom,
                    version: "1.0.0".to_string(),
                    path: backend.path.clone().into(),
                    installed_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64,
                    gpu_type: None,
                    source: None,
                },
            );
        }
        kronk_core::backends::registry::BackendRegistry::from_backends(data.backends)
    };

    let state = Arc::new(ProxyState::new(proxy_config, registry, config.clone()));

    // Create and run proxy server
    let server = ProxyServer::new(state.clone());
    server.run(addr).await?;

    Ok(())
}

#[cfg(test)]
mod tests;
