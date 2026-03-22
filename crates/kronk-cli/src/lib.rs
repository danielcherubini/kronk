pub mod args;
pub mod commands;

use crate::commands::backend::{BackendArgs, BackendSubcommand};
use anyhow::{Context, Result};
use clap::Parser;
use kronk_core::config::{Config, ModelConfig};
use kronk_core::logging;
use kronk_core::models::ModelRegistry;
use kronk_core::process::{ProcessEvent, ProcessSupervisor};
use kronk_core::profiles::SamplingParams;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

// Re-export functions for testing

/// Flags extracted from command line arguments that are specific to kronk.
/// Remaining args are passed through to the backend unchanged.
#[derive(Debug, Clone)]
pub struct ExtractedFlags {
    /// Model identifier - extracted if it looks like a model card ref (contains `/`, no `.gguf`, not absolute path)
    pub model: Option<String>,
    /// Quantization level (e.g., "Q4_K_M")
    pub quant: Option<String>,
    /// Sampling profile name
    pub profile: Option<String>,
    /// Port to bind to
    pub port: Option<u16>,
    /// Context length override
    pub context_length: Option<u32>,
    /// Arguments not recognized as kronk flags (passed to backend)
    pub remaining_args: Vec<String>,
}

/// Extract kronk-specific flags from command line arguments.
///
/// Parses arguments looking for: `--model`, `--profile`, `--quant`, `--port`, `--ctx`
///
/// # Model detection
/// A model argument is extracted if it looks like a model card reference:
/// - Contains `/` (e.g., "unsloth/Qwen3.5-0.8B")
/// - Does NOT contain `.gguf`
/// - Is NOT an absolute filesystem path
///
/// Otherwise, it's left in `remaining_args` for the backend.
///
/// # Flags consumed
/// Each recognized flag consumes both the flag AND its value from the argument list.
///
/// # Errors
/// Returns an error if a flag is present without a following value.
///
/// # Quant without model
/// If `--quant` is provided without `--model`, it's still extracted (no error).
/// The call site handles the warning about quant without model.
pub fn extract_kronk_flags(args: Vec<String>) -> Result<ExtractedFlags> {
    let mut model: Option<String> = None;
    let mut quant: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut context_length: Option<u32> = None;
    let mut remaining_args = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        match arg.as_str() {
            "--model" | "-m" => {
                if i + 1 >= args.len() {
                    anyhow::bail!("--model/-m flag requires a value");
                }
                let model_value = args[i + 1].clone();
                // Check if it looks like a model card ref
                let is_model_ref = model_value.contains('/')
                    && !model_value.contains(".gguf")
                    && !model_value.starts_with(std::path::MAIN_SEPARATOR)
                    && !model_value.starts_with('/');
                if is_model_ref {
                    model = Some(model_value);
                } else {
                    // Not a model ref, leave in remaining_args
                    remaining_args.push(arg.clone());
                    remaining_args.push(model_value);
                }
                i += 2;
            }
            "--profile" => {
                if i + 1 >= args.len() {
                    anyhow::bail!("--profile flag requires a value");
                }
                profile = Some(args[i + 1].clone());
                i += 2;
            }
            "--quant" => {
                if i + 1 >= args.len() {
                    anyhow::bail!("--quant flag requires a value");
                }
                quant = Some(args[i + 1].clone());
                i += 2;
            }
            "--port" => {
                if i + 1 >= args.len() {
                    anyhow::bail!("--port flag requires a valid u16 value");
                }
                let port_val = args[i + 1]
                    .parse::<u16>()
                    .context("--port requires a valid u16 value")?;
                port = Some(port_val);
                i += 2;
            }
            "--ctx" => {
                if i + 1 >= args.len() {
                    anyhow::bail!("--ctx flag requires a value");
                }
                let ctx_val = args[i + 1]
                    .parse::<u32>()
                    .context("--ctx requires a valid u32 value")?;
                context_length = Some(ctx_val);
                i += 2;
            }
            _ => {
                remaining_args.push(arg.clone());
                i += 1;
            }
        }
    }

    Ok(ExtractedFlags {
        model,
        quant,
        profile,
        port,
        context_length,
        remaining_args,
    })
}

#[derive(Parser, Debug)]
#[command(name = "kronk")]
#[command(version)]
#[command(about = "Oh yeah, it's all coming together. -- Local AI Server")]
struct Args {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Parser, Debug)]
enum Commands {
    /// Run a single server in the foreground (for debugging)
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
        /// Run a single server backend (legacy mode)
        #[arg(short, long)]
        server: Option<String>,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
        /// Run the proxy server instead of a single backend
        #[arg(long)]
        proxy: bool,
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
    #[command(hide = true)]
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
    /// Start kronk server (OpenAI-compatible API on a single port)
    Serve {
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
    /// OpenAI-compliant proxy for local AI models (deprecated: use `kronk serve`)
    #[command(hide = true)]
    Proxy {
        /// Proxy settings
        #[command(subcommand)]
        command: ProxyCommands,
    },
    /// View server logs
    Logs {
        /// Server name (defaults to "kronk" proxy logs)
        name: Option<String>,
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
    /// Enable a model (will be loaded on demand by the proxy)
    Enable {
        /// Model config name
        name: String,
    },
    /// Disable a model (will not be loaded by the proxy)
    Disable {
        /// Model config name
        name: String,
    },
    /// Create a model config from an installed model
    Create {
        /// Config name to create
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
    /// Install kronk as a system service (proxy mode)
    Install {
        /// Server name (omit to install the proxy; provide a name for legacy single-backend mode)
        name: Option<String>,
    },
    /// Start the kronk service
    Start {
        /// Server name (omit to start the proxy service)
        name: Option<String>,
    },
    /// Stop the kronk service
    Stop {
        /// Server name (omit to stop the proxy service)
        name: Option<String>,
    },
    /// Remove the kronk service
    Remove {
        /// Server name (omit to remove the proxy service)
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

#[allow(dead_code)]
fn main() -> Result<()> {
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

    logging::init();

    let args = Args::parse();
    let config = Config::load()?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        match args.command {
            Commands::Run { name, ctx } => cmd_run(&config, &name, ctx).await,
            Commands::Service { command } => cmd_service(&config, command),
            Commands::ServiceRun { server, ctx, proxy } => {
                if proxy {
                    let host = config.proxy.host.clone();
                    let port = config.proxy.port;
                    let idle_timeout = config.proxy.idle_timeout_secs;
                    cmd_serve(&config, host, port, idle_timeout).await
                } else {
                    let server = server.ok_or_else(|| {
                        anyhow::anyhow!(
                            "Either --server or --proxy must be provided for service-run"
                        )
                    })?;
                    cmd_run(&config, &server, ctx).await
                }
            }
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
            Commands::Serve {
                host,
                port,
                idle_timeout,
            } => cmd_serve(&config, host, port, idle_timeout).await,
            Commands::Proxy { command } => cmd_proxy(&config, command).await,
            Commands::Logs {
                name,
                follow,
                lines,
            } => {
                let name = name.unwrap_or_else(|| "kronk".to_string());
                cmd_logs(&config, &name, follow, lines).await
            }
        }
    })
}

#[allow(dead_code)]
async fn cmd_logs(config: &Config, name: &str, follow: bool, lines: usize) -> Result<()> {
    let logs_dir = config.logs_dir()?;
    let log_path = logging::log_path(&logs_dir, name);

    if !log_path.exists() {
        println!("No logs found for '{}'.", name);
        println!();
        println!("Logs are created when running as a service.");
        println!("Install the service: kronk service install");
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
    // Extract args from the command line before SCM takes over
    let raw_args: Vec<String> = std::env::args().collect();
    let is_proxy = raw_args.iter().any(|a| a == "--proxy");

    let server = raw_args
        .iter()
        .position(|a| a == "--server")
        .and_then(|i| raw_args.get(i + 1))
        .cloned();

    if !is_proxy && server.is_none() {
        anyhow::bail!("Either --server or --proxy must be provided. This binary should be launched by the Windows Service Control Manager.");
    }

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

    let service_name = if is_proxy {
        "kronk".to_string()
    } else {
        Config::service_name(server.as_deref().unwrap())
    };

    // Store in globals so service_main can access them
    SERVICE_PROXY
        .set(is_proxy)
        .map_err(|_| anyhow::anyhow!("Failed to set service proxy flag"))?;
    SERVICE_SERVER
        .set(server.unwrap_or_default())
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
static SERVICE_PROXY: OnceLock<bool> = OnceLock::new();
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

    let is_proxy = SERVICE_PROXY.get().copied().unwrap_or(false);
    let server = SERVICE_SERVER.get().cloned().unwrap_or_default();
    let service_name = SERVICE_NAME.get().cloned().unwrap_or_default();
    let config_dir = SERVICE_CONFIG_DIR.get().and_then(|o| o.clone());
    let ctx = SERVICE_CTX.get().and_then(|o| *o);

    // Set up logging to file — use config_dir/logs if available, otherwise fall back
    let log_dir = config_dir
        .clone()
        .unwrap_or_else(|| {
            directories::ProjectDirs::from("", "", "kronk")
                .map(|p: directories::ProjectDirs| p.data_dir().to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        })
        .join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = std::fs::File::create(log_dir.join(format!("{}.log", service_name)))
        .unwrap_or_else(|_| std::fs::File::create("kronk-service.log").unwrap());

    // Set up tracing to write to the log file (services have no stderr)
    let subscriber = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(log_file))
        .with_ansi(false)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);

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
        if is_proxy {
            // Proxy mode: start the proxy server
            use kronk_core::proxy::server::ProxyServer;
            use kronk_core::proxy::ProxyState;
            use std::sync::Arc;

            let host = config.proxy.host.clone();
            let port = config.proxy.port;
            let (host_addr, _) = match host.parse::<std::net::IpAddr>() {
                Ok(addr) => (addr, false),
                Err(_) => (
                    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
                    true,
                ),
            };
            let addr = std::net::SocketAddr::new(host_addr, port);

            tracing::info!("Starting Kronk proxy service on {}", addr);

            let state = Arc::new(ProxyState::new(config));
            let server = ProxyServer::new(state);

            // Bridge SCM shutdown signal to abort the server
            let (shutdown_tx_tokio, mut shutdown_rx_tokio) = mpsc::channel::<()>(1);
            tokio::task::spawn_blocking(move || {
                let _ = shutdown_rx.recv();
                let _ = shutdown_tx_tokio.blocking_send(());
            });

            tokio::select! {
                result = server.run(addr) => {
                    if let Err(e) = result {
                        tracing::error!("Proxy server error: {}", e);
                    }
                }
                _ = shutdown_rx_tokio.recv() => {
                    tracing::info!("Received shutdown signal, stopping proxy...");
                }
            }
        } else {
            // Legacy single-backend mode
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
                        ProcessEvent::Crashed(msg) => {
                            tracing::warn!("Backend crashed: {}", msg)
                        }
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
        }
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
#[allow(dead_code)]
fn build_full_args(
    config: &Config,
    server: &kronk_core::config::ModelConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> Result<Vec<String>> {
    config.build_full_args(server, backend, ctx_override)
}

#[allow(dead_code)]
async fn cmd_run(config: &Config, server_name: &str, ctx_override: Option<u32>) -> Result<()> {
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

    let (tx, mut rx) = mpsc::unbounded_channel::<ProcessEvent>();

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

    supervisor.run(tx, None).await?;
    printer.abort();
    Ok(())
}

#[allow(dead_code)]
fn cmd_service(config: &Config, command: ServiceCommands) -> Result<()> {
    match command {
        ServiceCommands::Install { name } => {
            if let Some(server_name) = name {
                // Legacy: install a single backend as a service
                let (srv, backend) = config.resolve_server(&server_name)?;
                let service_name = Config::service_name(&server_name);

                #[cfg(target_os = "windows")]
                {
                    let display_name = format!("Kronk: {}", server_name);
                    let config_dir = Config::base_dir()?;
                    let port = srv.port.unwrap_or(8080);
                    kronk_core::platform::windows::install_service(
                        &service_name,
                        &display_name,
                        &server_name,
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

                println!("Installed service for model '{}'.", server_name);
            } else {
                // Default: install the proxy as a service
                #[cfg(target_os = "windows")]
                {
                    let config_dir = Config::base_dir()?;
                    let port = config.proxy.port;
                    kronk_core::platform::windows::install_proxy_service(&config_dir, port)?;
                }

                #[cfg(target_os = "linux")]
                kronk_core::platform::linux::install_proxy_service()?;

                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                anyhow::bail!("Service management not supported on this platform");

                println!("Installed kronk service.");
                println!("Start it: kronk service start");
            }
        }
        ServiceCommands::Start { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "kronk".to_string());
            service_start_inner(&service_name)?;
            println!("Pull the lever! '{}' started.", service_name);
        }
        ServiceCommands::Stop { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "kronk".to_string());
            service_stop_inner(&service_name)?;
            println!("Wrong lever! '{}' stopped.", service_name);
        }
        ServiceCommands::Remove { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "kronk".to_string());

            #[cfg(target_os = "windows")]
            kronk_core::platform::windows::remove_service(&service_name)?;

            #[cfg(target_os = "linux")]
            kronk_core::platform::linux::remove_service(&service_name)?;

            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = service_name;
                anyhow::bail!("Not supported on this platform");
            }

            println!("No touchy! '{}' removed.", service_name);
        }
    }
    Ok(())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
/// Format seconds as human-readable duration (e.g. "4m28s" or "32s").
fn format_duration_secs(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

#[allow(dead_code)]
async fn cmd_status(config: &Config) -> Result<()> {
    println!("KRONK Status");
    println!("{}", "-".repeat(60));

    // Query proxy /status endpoint with 500ms timeout
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .unwrap_or_default();

    let proxy_url = config.proxy_url().map(|url| format!("{}/status", url));
    let proxy_response = if let Some(url) = proxy_url {
        match http_client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => resp.json::<serde_json::Value>().await.ok(),
            _ => None,
        }
    } else {
        None
    };

    if let Some(ref proxy_json) = proxy_response {
        // VRAM from proxy response
        if let Some(vram) = proxy_json.get("vram").and_then(|v| v.as_object()) {
            let used = vram.get("used_mib").and_then(|v| v.as_u64()).unwrap_or(0);
            let total = vram.get("total_mib").and_then(|v| v.as_u64()).unwrap_or(0);
            println!("  VRAM:     {} / {} MiB", used, total);
        }

        // Models from proxy response (object keyed by model name)
        if let Some(models) = proxy_json.get("models").and_then(|m| m.as_object()) {
            for (model_name, model) in models {
                let backend = model
                    .get("backend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("???");
                let backend_path = model
                    .get("backend_path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("???");
                let source = model.get("source").and_then(|v| v.as_str()).unwrap_or("");
                let quant = model.get("quant").and_then(|v| v.as_str()).unwrap_or("");
                let profile = model.get("profile").and_then(|v| v.as_str()).unwrap_or("");
                let context_length = model.get("context_length").and_then(|v| v.as_u64());
                let loaded = model
                    .get("loaded")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let loaded_str = if loaded {
                    let last_accessed = model
                        .get("last_accessed_secs_ago")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let remaining = model
                        .get("idle_timeout_remaining_secs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    format!(
                        "true (idle: {}s ago, unloads in {})",
                        last_accessed,
                        format_duration_secs(remaining),
                    )
                } else {
                    "false".to_string()
                };

                println!();
                println!("  Model:    {}", model_name);
                println!("  Source:   {}", source);
                println!("  Quant:    {}", quant);
                println!("  Profile:  {}", profile);
                if let Some(ctx) = context_length {
                    println!("  Context:  {}", ctx);
                }
                println!("  Backend:  {} ({})", backend, backend_path);
                println!("  Loaded:   {}", loaded_str);
            }
        }
    } else {
        // Proxy not running - query VRAM locally for fallback
        if let Some(vram) = kronk_core::gpu::query_vram() {
            println!("  VRAM:     {} / {} MiB", vram.used_mib, vram.total_mib);
        }

        for (name, srv) in &config.models {
            let backend_path = config
                .backends
                .get(&srv.backend)
                .map(|b| b.path.as_str())
                .unwrap_or("???");

            println!();
            println!("  Model:    {}", name);
            println!("  Source:   {}", srv.source.as_deref().unwrap_or(""));
            println!("  Quant:    {}", srv.quant.as_deref().unwrap_or(""));
            println!(
                "  Profile:  {}",
                srv.profile
                    .as_ref()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "none".to_string())
            );
            if let Some(ctx) = srv.context_length {
                println!("  Context:  {}", ctx);
            }
            println!("  Backend:  {} ({})", srv.backend, backend_path);
            println!("  Loaded:   proxy not running");
        }
    }

    println!();
    Ok(())
}

#[allow(dead_code)]
async fn cmd_server(config: &Config, command: ServerCommands) -> Result<()> {
    match command {
        ServerCommands::Ls => cmd_server_ls(config).await,
        ServerCommands::Add { name, command } => {
            cmd_server_add(config, &name, command, false).await
        }
        ServerCommands::Edit { name, command } => {
            if !config.models.contains_key(&name) {
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

#[allow(dead_code)]
async fn cmd_server_ls(config: &Config) -> Result<()> {
    if config.models.is_empty() {
        println!("No models configured.");
        println!();
        println!("Pull one: kronk model pull <repo>");
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    println!("Models:");
    println!("{}", "-".repeat(60));

    for (name, srv) in &config.models {
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

#[allow(dead_code)]
fn cmd_server_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    if !config.models.contains_key(name) {
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
        let confirm = inquire::Confirm::new(&format!("Remove model '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut config = config.clone();
    config.models.remove(name);
    config.save()?;

    println!("Model '{}' removed.", name);
    Ok(())
}

/// Resolve a backend path to a backend key in the config.
///
/// This function handles:
/// - Path absolutization: filesystem paths (containing separators, starting with `./` or `/`)
///   are resolved to absolute paths; bare command names (e.g., "llama-server") are left as-is
///   for PATH resolution at runtime.
/// - Finding an existing backend by path, or creating a new one if not found.
///
/// # Arguments
/// * `config` - Mutable config to store new backends
/// * `exe_path` - The executable path or bare command name
///
/// # Returns
/// The backend key (name) that should be used for this backend.
fn resolve_backend(config: &mut Config, exe_path: &str) -> Result<(String, String)> {
    use kronk_core::config::BackendConfig;

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

    Ok((backend_key, exe_str))
}

/// Add a new server from a command line, extracting kronk flags.
pub async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
) -> Result<()> {
    use anyhow::Context;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    // Split exe from args
    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Resolve backend path
    let mut config = config.clone();
    let (backend_key, exe_str) = resolve_backend(&mut config, exe_path)?;

    // Extract kronk flags from args
    let extracted = extract_kronk_flags(args.clone())?;

    // Check for duplicate server
    if config.models.contains_key(name) && !overwrite {
        anyhow::bail!(
            "Server '{}' already exists. Use `kronk server edit` to modify it.",
            name
        );
    }

    // Resolve model card if model ref is provided
    let model_info = if let Some(ref model_ref) = extracted.model {
        let models_dir = config.models_dir()?;
        let configs_dir = config.configs_dir()?;
        let registry = ModelRegistry::new(models_dir, configs_dir);

        match registry.find(model_ref) {
            Ok(Some(installed)) => Some(installed),
            Ok(None) => {
                anyhow::bail!(
                    "Model '{}' not found. Use `kronk model pull <repo>` to install it first.",
                    model_ref
                );
            }
            Err(e) => {
                anyhow::bail!("Failed to look up model '{}': {}", model_ref, e);
            }
        }
    } else {
        None
    };

    // Resolve quantization with interactive selection if needed
    let quant_name = if let Some(ref quant) = extracted.quant {
        // If model card exists, validate quant against available quants
        if let Some(ref installed) = model_info {
            if !installed.card.quants.contains_key(quant) {
                let available: Vec<_> = installed.card.quants.keys().collect();
                anyhow::bail!(
                    "Quant '{}' not found in model '{}'. Available quants: {}",
                    quant,
                    installed.card.model.source,
                    available
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Some(quant.clone())
        } else {
            // No model card - quant is just stored as-is
            Some(quant.clone())
        }
    } else if let Some(ref installed) = model_info {
        // Model ref provided but no --quant - offer interactive selection
        let quant_names: Vec<String> = installed.card.quants.keys().cloned().collect();
        if quant_names.is_empty() {
            anyhow::bail!(
                "No quants available for '{}'. Run `kronk model pull {}` first.",
                installed.card.model.source,
                installed.card.model.source
            );
        }
        Some(if quant_names.len() == 1 {
            quant_names.into_iter().next().unwrap()
        } else {
            inquire::Select::new("Select a quant:", quant_names)
                .prompt()
                .context("Quant selection cancelled")?
        })
    } else {
        None
    };

    // Verify GGUF file exists if both model and quant are specified
    if let Some(ref quant_name) = quant_name {
        let gguf_path = model_info
            .as_ref()
            .unwrap()
            .card
            .quants
            .get(quant_name)
            .map(|q| model_info.as_ref().unwrap().dir.join(&q.file));

        if let Some(ref path) = gguf_path {
            if !path.exists() {
                anyhow::bail!(
                    "GGUF file '{}' not found. Make sure the model is properly installed.",
                    path.display()
                );
            }
        }
    }

    // Parse profile if provided
    let profile = extracted
        .profile
        .as_ref()
        .map(|s| s.parse::<kronk_core::profiles::Profile>())
        .transpose()
        .context("Failed to parse profile name")?;

    // Build ModelConfig
    let model_config = ModelConfig {
        backend: backend_key.clone(),
        args: extracted.remaining_args.clone(),
        profile,
        sampling: None,
        model: extracted.model.clone(),
        quant: quant_name,
        port: extracted.port,
        health_check: None,
        enabled: true,
        source: model_info.as_ref().map(|m| m.card.model.source.clone()),
        context_length: extracted.context_length,
    };

    config.models.insert(name.to_string(), model_config.clone());
    config.save()?;

    // Output
    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Model:    {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = model_config.model {
        let quant = model_config.quant.as_deref().unwrap_or("?");
        let source = model_config.source.as_deref().unwrap_or(model);
        println!("  Model:    {} ({})", source, quant);
    }

    if let Some(ref profile) = model_config.profile {
        println!("  Profile:  {}", profile);
    }

    if let Some(ref quant) = model_config.quant {
        if let Some(ref model) = model_config.model {
            let models_dir = config.models_dir()?;
            let configs_dir = config.configs_dir()?;
            let registry = ModelRegistry::new(models_dir, configs_dir);
            if let Ok(Some(installed)) = registry.find(model) {
                if let Some(q) = installed.card.quants.get(quant) {
                    println!("  GGUF:     {}", q.file);
                }
            }
        }
    }

    println!();
    println!("Enable it:  kronk model enable {}", name);
    println!("Start:      kronk serve");

    Ok(())
}

pub async fn cmd_server_edit(config: &mut Config, name: &str, command: Vec<String>) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    let (backend_key, exe_str) = resolve_backend(config, exe_path)?;

    // Extract kronk flags from args
    let extracted = extract_kronk_flags(args)?;

    let mut config = config.clone();

    // Verify server exists
    if !config.models.contains_key(name) {
        anyhow::bail!("Server '{}' not found", name);
    }

    // Mutate via get_mut in a block so the borrow is dropped before save()
    {
        let srv = config.models.get_mut(name).unwrap();

        // Selectively merge extracted flags into existing ModelConfig
        if let Some(ref model) = extracted.model {
            srv.model = Some(model.clone());
            srv.source = Some(model.clone());
        }
        if let Some(ref quant) = extracted.quant {
            srv.quant = Some(quant.clone());
        }
        if let Some(ref profile) = extracted.profile {
            let p = profile
                .parse::<kronk_core::profiles::Profile>()
                .context("Failed to parse profile name")?;
            srv.profile = Some(p);
        }
        if let Some(port) = extracted.port {
            srv.port = Some(port);
        }
        if let Some(ctx) = extracted.context_length {
            srv.context_length = Some(ctx);
        }

        srv.backend = backend_key.clone();
        srv.args = extracted.remaining_args.clone();
    }

    config.save()?;

    // Read back for output
    let srv = config.models.get(name).unwrap();

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Name:     {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = srv.model {
        let quant = srv.quant.as_deref().unwrap_or("?");
        let source = srv.source.as_deref().unwrap_or(model);
        println!("  Model:    {} ({})", source, quant);
    }
    if let Some(ref profile) = srv.profile {
        println!("  Profile:  {}", profile);
    }

    println!();

    Ok(())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

            // Show which models use which profile
            println!("Model assignments:");
            for (name, srv) in &config.models {
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
            if !config.models.contains_key(&server) {
                anyhow::bail!("Model '{}' not found", server);
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

            config.models.get_mut(&server).unwrap().profile = Some(resolved);
            config.save()?;

            println!("Oh yeah, it's all coming together.");
            println!("  Model '{}' now uses '{}' preset.", server, profile);

            Ok(())
        }
        ProfileCommands::Clear { server } => {
            let mut config = config.clone();
            let srv = config
                .models
                .get_mut(&server)
                .with_context(|| format!("Model '{}' not found", server))?;

            srv.profile = None;
            config.save()?;

            println!("Profile cleared for model '{}'.", server);
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
            println!("Assign it: kronk profile set <model> {}", name);
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
                .models
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

#[allow(dead_code)]
/// Start the kronk server (proxy) with the given host, port, and idle timeout.
async fn start_proxy_server(
    config: &Config,
    host: String,
    port: u16,
    idle_timeout: u64,
) -> Result<()> {
    use kronk_core::proxy::server::ProxyServer;
    use kronk_core::proxy::ProxyState;
    use std::net::SocketAddr;
    use std::sync::Arc;

    // Apply CLI overrides to config
    let mut updated_config = config.clone();
    updated_config.proxy.host = host.clone();
    updated_config.proxy.port = port;
    updated_config.proxy.idle_timeout_secs = idle_timeout;

    // Parse host and port
    let (host_addr, warning) = match host.parse::<std::net::IpAddr>() {
        Ok(addr) => (addr, false),
        Err(_) => (
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            true,
        ),
    };
    let addr = SocketAddr::new(host_addr, port);

    if warning {
        tracing::warn!("Invalid host '{}' - using 127.0.0.1", host);
    }

    tracing::info!("Starting Kronk on {}", addr);
    tracing::info!("Idle timeout: {}s", idle_timeout);

    let state = Arc::new(ProxyState::new(updated_config));

    // Create and run proxy server
    let server = ProxyServer::new(state.clone());
    server.run(addr).await?;

    Ok(())
}

#[allow(dead_code)]
/// Start the kronk server.
async fn cmd_serve(config: &Config, host: String, port: u16, idle_timeout: u64) -> Result<()> {
    start_proxy_server(config, host, port, idle_timeout).await
}

#[allow(dead_code)]
/// Start the OpenAI-compliant proxy server (deprecated: use `kronk serve`).
async fn cmd_proxy(config: &Config, command: ProxyCommands) -> Result<()> {
    eprintln!("Warning: `kronk proxy start` is deprecated. Use `kronk serve` instead.");

    let ProxyCommands::Start {
        host,
        port,
        idle_timeout,
    } = command;

    start_proxy_server(config, host, port, idle_timeout).await
}
