use anyhow::{Context, Result};
use clap::Parser;
use kronk_core::config::Config;
use kronk_core::logging;
use kronk_core::process::{ProcessEvent, ProcessSupervisor};
use kronk_core::use_cases::SamplingParams;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

mod args;
mod commands;

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
    /// Pull the lever! Run a profile in the foreground
    Run {
        #[arg(short, long, default_value = "default")]
        profile: String,
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
        profile: String,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
    },
    /// Add a new profile from a raw command line
    #[command(hide = true)]
    Add {
        /// Profile name
        name: String,
        /// The full command: binary path followed by all arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Update an existing profile with a new command line
    #[command(hide = true)]
    Update {
        /// Profile name
        name: String,
        /// The full command: binary path followed by all arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Manage profiles — list, add, edit, remove
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },
    /// Show status of all profiles
    Status,
    /// Manage use-case presets for sampling parameters
    UseCase {
        #[command(subcommand)]
        command: UseCaseCommands,
    },
    /// View or edit configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Manage models — pull, list, create profiles
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
    /// View backend logs for a profile
    Logs {
        /// Profile name (default: "default")
        #[arg(short, long, default_value = "default")]
        profile: String,
        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
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
    /// Create a profile from an installed model
    Create {
        /// Profile name to create
        name: String,
        /// Model ID in "company/modelname" format
        #[arg(long)]
        model: String,
        /// Quant to use (e.g. "Q4_K_M"). Interactive picker if omitted.
        #[arg(long)]
        quant: Option<String>,
        /// Use case preset: coding, chat, analysis, creative
        #[arg(long)]
        use_case: Option<String>,
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
pub enum ProfileCommands {
    /// List all profiles with status
    Ls,
    /// Add a new profile from a raw command line
    Add {
        /// Profile name
        name: String,
        /// Backend command and arguments (e.g. llama-server -m model.gguf)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Edit an existing profile's command line
    Edit {
        /// Profile name
        name: String,
        /// New backend command and arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Remove a profile
    Rm {
        /// Profile name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

#[derive(Parser, Debug)]
enum UseCaseCommands {
    /// List all available use cases and their sampling params
    List,
    /// Set a profile's use case
    Set {
        /// Profile name
        profile: String,
        /// Use case name: coding, chat, analysis, creative, or a custom name
        use_case: String,
    },
    /// Clear a profile's use case (remove sampling preset)
    Clear {
        /// Profile name
        profile: String,
    },
    /// Create a custom use case with specific sampling params
    Add {
        /// Custom use case name
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
    /// Remove a custom use case
    Remove {
        /// Custom use case name
        name: String,
    },
}

#[derive(Parser, Debug)]
enum ServiceCommands {
    /// Oh right, the service. Install a profile as a Windows service
    Install {
        #[arg(short, long, default_value = "default")]
        profile: String,
    },
    /// Start an installed service
    Start {
        #[arg(short, long, default_value = "default")]
        profile: String,
    },
    /// Stop a running service
    Stop {
        #[arg(short, long, default_value = "default")]
        profile: String,
    },
    /// No touchy! Remove an installed service
    Remove {
        #[arg(short, long, default_value = "default")]
        profile: String,
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

    // Normal CLI mode
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let args = Args::parse();
    let config = Config::load()?;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        match args.command {
            Commands::Run { profile, ctx } => cmd_run(&config, &profile, ctx).await,
            Commands::Service { command } => cmd_service(&config, command),
            Commands::ServiceRun { profile, ctx } => cmd_run(&config, &profile, ctx).await,
            Commands::Add { name, command } => {
                cmd_profile_add(&config, &name, command, false).await
            }
            Commands::Update { name, command } => {
                cmd_profile_edit(&mut config.clone(), &name, command).await
            }
            Commands::Profile { command } => cmd_profile(&config, command).await,
            Commands::Status => cmd_status(&config).await,
            Commands::UseCase { command } => cmd_use_case(&config, command),
            Commands::Config { command } => cmd_config(&config, command),
            Commands::Model { command } => commands::model::run(&config, command).await,
            Commands::Logs {
                profile,
                follow,
                lines,
            } => cmd_logs(&config, &profile, follow, lines).await,
        }
    })
}

async fn cmd_logs(config: &Config, profile: &str, follow: bool, lines: usize) -> Result<()> {
    let logs_dir = config.logs_dir()?;
    let log_path = logging::log_path(&logs_dir, profile);

    if !log_path.exists() {
        println!("No logs found for profile '{}'.", profile);
        println!();
        println!("Logs are created when running as a service.");
        println!("For foreground: kronk run --profile {}", profile);
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
    // Extract profile name and config-dir from args before SCM takes over
    let raw_args: Vec<String> = std::env::args().collect();
    let profile = raw_args
        .iter()
        .position(|a| a == "--profile")
        .and_then(|i| raw_args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "default".to_string());

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

    let service_name = Config::service_name(&profile);

    // Store in globals so service_main can access them
    SERVICE_PROFILE
        .set(profile)
        .map_err(|_| anyhow::anyhow!("Failed to set service profile"))?;
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
static SERVICE_PROFILE: OnceLock<String> = OnceLock::new();
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

    let profile = SERVICE_PROFILE.get().cloned().unwrap_or_default();
    let service_name = SERVICE_NAME.get().cloned().unwrap_or_default();
    let config_dir = SERVICE_CONFIG_DIR.get().and_then(|o| o.clone());
    let ctx = SERVICE_CTX.get().cloned();

    // Set up logging to file — use config_dir if available, otherwise fall back
    let log_dir = config_dir.clone().unwrap_or_else(|| {
        directories::ProjectDirs::from("", "", "kronk")
            .map(|p: directories::ProjectDirs| p.data_dir().to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    });
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file = std::fs::File::create(log_dir.join(format!("{}.log", service_name)))
        .unwrap_or_else(|_| std::fs::File::create("kronk-service.log").unwrap());

    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(log_file))
        .with_env_filter("info")
        .init();

    tracing::info!("Service starting for profile: {}", profile);
    if let Some(ref dir) = config_dir {
        tracing::info!("Config dir: {}", dir.display());
    }

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
            tracing::error!("Failed to load config: {}", e);
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
        let (prof, backend) = match config.resolve_profile(&profile) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("Failed to resolve profile '{}': {}", profile, e);
                return;
            }
        };

        let args = build_full_args(&config, prof, backend, ctx).unwrap_or_else(|e| {
            tracing::warn!("Failed to build model args: {}", e);
            let mut args = backend.default_args.clone();
            args.extend(prof.args.clone());
            args
        });
        let log_dir = config
            .logs_dir()
            .ok()
            .expect("Failed to get logs directory");
        let health_check = config.resolve_health_check(&prof);
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

/// Build the full argument list for a profile, resolving model card args at runtime.
/// Merges: backend.default_args + profile.args + model card (-m, -c, -ngl) + sampling
fn build_full_args(
    config: &Config,
    profile: &kronk_core::config::ProfileConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> Result<Vec<String>> {
    let mut args = backend.default_args.clone();
    args.extend(profile.args.clone());

    // Inject model card args: -m, -c, -ngl
    if let (Some(ref model_id), Some(ref quant_name)) = (&profile.model, &profile.quant) {
        let models_dir = config.models_dir()?;
        let registry = kronk_core::models::ModelRegistry::new(models_dir);
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
                args::inject_context_size(&mut args, ctx);
            }
            if let Some(ngl) = installed.card.model.default_gpu_layers {
                if !args.iter().any(|a| a == "-ngl" || a == "--n-gpu-layers") {
                    args.push("-ngl".to_string());
                    args.push(ngl.to_string());
                }
            }

            // 3-layer sampling merge
            if let Some(sampling) =
                config.effective_sampling_with_card(profile, Some(&installed.card))
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

    // No model card — just use profile sampling
    if let Some(sampling) = config.effective_sampling_with_card(profile, None) {
        args.extend(sampling.to_args());
    }

    Ok(args)
}

async fn cmd_run(config: &Config, profile_name: &str, ctx_override: Option<u32>) -> Result<()> {
    let (profile, backend) = config.resolve_profile(profile_name)?;

    let args = build_full_args(config, profile, backend, ctx_override)?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Profile:  {}", profile_name);
    println!("  Backend:  {}", backend.path);
    if let Some(ctx) = ctx_override {
        println!("  Context:  {}", ctx);
    }
    let health_check = config.resolve_health_check(profile);
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

fn cmd_service(config: &Config, command: ServiceCommands) -> Result<()> {
    match command {
        ServiceCommands::Install { profile } => {
            let (prof, backend) = config.resolve_profile(&profile)?;
            let service_name = Config::service_name(&profile);

            #[cfg(target_os = "windows")]
            {
                let display_name = format!("Kronk: {}", profile);
                let config_dir = Config::base_dir()?;
                let port = prof.port.unwrap_or(8080);
                kronk_core::platform::windows::install_service(
                    &service_name,
                    &display_name,
                    &profile,
                    &config_dir,
                    port,
                )?;
            }

            #[cfg(target_os = "linux")]
            {
                let args = build_full_args(config, prof, backend, None)?;
                let port = prof.port.unwrap_or(8080);
                kronk_core::platform::linux::install_service(
                    &service_name,
                    &backend.path,
                    &args,
                    port,
                )?;
            }

            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = (prof, backend);
                anyhow::bail!("Service management not supported on this platform");
            }

            println!("Oh right. The service. The service for {}.", profile);
            println!("  Installed. Auto-starts on boot.");
            println!("  Run `kronk service start` to start it now.");
        }
        ServiceCommands::Start { profile } => {
            let service_name = Config::service_name(&profile);
            service_start_inner(&service_name)?;
            println!("Pull the lever! '{}' started.", service_name);
        }
        ServiceCommands::Stop { profile } => {
            let service_name = Config::service_name(&profile);
            service_stop_inner(&service_name)?;
            println!("Wrong lever! '{}' stopped.", service_name);
        }
        ServiceCommands::Remove { profile } => {
            let service_name = Config::service_name(&profile);

            #[cfg(target_os = "windows")]
            kronk_core::platform::windows::remove_service(&service_name)?;

            #[cfg(target_os = "linux")]
            kronk_core::platform::linux::remove_service(&service_name)?;

            println!("No touchy! '{}' removed.", service_name);
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

    for (name, profile) in &config.profiles {
        let _backend = config.backends.get(&profile.backend);
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

        // Check health endpoint using profile's resolved health check config
        let health_check = config.resolve_health_check(profile);
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
        println!("  Profile:  {}", name);
        println!("  Backend:  {} ({})", profile.backend, backend_path);
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

async fn cmd_profile(config: &Config, command: ProfileCommands) -> Result<()> {
    match command {
        ProfileCommands::Ls => cmd_profile_ls(config).await,
        ProfileCommands::Add { name, command } => {
            cmd_profile_add(config, &name, command, false).await
        }
        ProfileCommands::Edit { name, command } => {
            if !config.profiles.contains_key(&name) {
                anyhow::bail!(
                    "Profile '{}' not found. Use `kronk profile add` to create it.",
                    name
                );
            }
            cmd_profile_edit(&mut config.clone(), &name, command).await
        }
        ProfileCommands::Rm { name, force } => cmd_profile_rm(config, &name, force),
    }
}

async fn cmd_profile_ls(config: &Config) -> Result<()> {
    if config.profiles.is_empty() {
        println!("No profiles configured.");
        println!();
        println!("Add one:  kronk profile add <name> <command...>");
        println!("Or pull:  kronk model pull <repo>");
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    println!("Profiles:");
    println!("{}", "-".repeat(60));

    for (name, profile) in &config.profiles {
        let _backend = config.backends.get(&profile.backend);
        let use_case = profile
            .use_case
            .as_ref()
            .map(|uc| uc.to_string())
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

        // Use profile's resolved health check config
        let health_check = config.resolve_health_check(profile);
        let health = if let Some(url) = health_check.url {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY",
                _ => "DOWN",
            }
        } else {
            "N/A"
        };

        println!();
        println!("  {}  (backend: {})", name, profile.backend);
        println!(
            "    use-case: {}  service: {}  health: {}",
            use_case, service_status, health
        );

        if let Some(ref model) = profile.model {
            let quant = profile.quant.as_deref().unwrap_or("?");
            println!("    model: {} / {}", model, quant);
        }

        if !profile.args.is_empty() {
            let args_str = profile.args.join(" ");
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

fn cmd_profile_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    if !config.profiles.contains_key(name) {
        anyhow::bail!("Profile '{}' not found.", name);
    }

    // Check if a service is installed for this profile
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
            "Profile '{}' has an installed service '{}'. Remove it first with: kronk service remove --profile {}",
            name, service_name, name
        );
    }

    if !force {
        let confirm = inquire::Confirm::new(&format!("Remove profile '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    let mut config = config.clone();
    config.profiles.remove(name);
    config.save()?;

    println!("Profile '{}' removed.", name);
    Ok(())
}

async fn cmd_profile_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
) -> Result<()> {
    use kronk_core::config::{BackendConfig, ProfileConfig};

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Resolve the exe to an absolute path
    let exe_abs = std::path::Path::new(exe_path);
    let exe_resolved = if exe_abs.is_absolute() {
        exe_abs.to_path_buf()
    } else {
        std::env::current_dir()?.join(exe_abs)
    };
    let exe_str = exe_resolved.to_string_lossy().to_string();

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
            // Derive a backend name from the exe filename
            let stem = exe_resolved
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "backend".to_string())
                .replace('-', "_");

            config.backends.insert(
                stem.clone(),
                BackendConfig {
                    path: exe_str.clone(),
                    default_args: vec![],
                    health_check_url: Some("http://localhost:8080/health".to_string()),
                },
            );
            stem
        }
    };

    // Check for duplicate profile
    if config.profiles.contains_key(name) && !overwrite {
        anyhow::bail!(
            "Profile '{}' already exists. Use `kronk update` to modify it.",
            name
        );
    }

    config.profiles.insert(
        name.to_string(),
        ProfileConfig {
            backend: backend_key.clone(),
            args,
            use_case: None,
            sampling: None,
            model: None,
            quant: None,
            port: None,
            health_check: None,
        },
    );

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Profile:  {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);
    println!();
    println!("Run it:     kronk run --profile {}", name);
    println!("Install it: kronk service install --profile {}", name);

    Ok(())
}

async fn cmd_profile_edit(config: &mut Config, name: &str, command: Vec<String>) -> Result<()> {
    use kronk_core::config::BackendConfig;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Resolve the exe to an absolute path
    let exe_abs = std::path::Path::new(exe_path);
    let exe_resolved = if exe_abs.is_absolute() {
        exe_abs.to_path_buf()
    } else {
        std::env::current_dir()?.join(exe_abs)
    };
    let exe_str = exe_resolved.to_string_lossy().to_string();

    // Check if this backend path exists
    let backend_name = config
        .backends
        .iter()
        .find(|(_, b)| b.path == exe_str)
        .map(|(k, _)| k.clone());

    let backend_key = match backend_name {
        Some(k) => k,
        None => {
            // Derive a backend name from the exe filename
            let stem = exe_resolved
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "backend".to_string())
                .replace('-', "_");

            config.backends.insert(
                stem.clone(),
                BackendConfig {
                    path: exe_str.clone(),
                    default_args: vec![],
                    health_check_url: Some("http://localhost:8080/health".to_string()),
                },
            );
            stem
        }
    };

    // Load config, update only the command string for the existing profile
    let mut config = config.clone();
    let profile = config
        .profiles
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("Profile '{}' not found", name))?;

    profile.backend = backend_key.clone();
    profile.args = args;

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Profile:  {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);
    println!();
    println!("Run it:     kronk run --profile {}", name);
    println!("Install it: kronk service install --profile {}", name);

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

fn cmd_use_case(config: &Config, command: UseCaseCommands) -> Result<()> {
    use kronk_core::use_cases::UseCase;

    match command {
        UseCaseCommands::List => {
            println!("Available use cases:");
            println!();
            for (name, desc, uc) in UseCase::all() {
                let params = uc.params();
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
                println!();
            }

            // Show custom use cases from config
            if let Some(custom) = &config.custom_use_cases {
                if !custom.is_empty() {
                    println!("Custom use cases:");
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

            // Show which profiles use which use case
            println!("Profile assignments:");
            for (name, profile) in &config.profiles {
                let uc_str = profile
                    .use_case
                    .as_ref()
                    .map(|uc| uc.to_string())
                    .unwrap_or_else(|| "none".to_string());
                println!("  {} -> {}", name, uc_str);
            }

            Ok(())
        }
        UseCaseCommands::Set { profile, use_case } => {
            let mut config = config.clone();
            let prof = config
                .profiles
                .get_mut(&profile)
                .with_context(|| format!("Profile '{}' not found", profile))?;

            // Try built-in first
            let uc = match use_case.as_str() {
                "coding" => UseCase::Coding,
                "chat" => UseCase::Chat,
                "analysis" => UseCase::Analysis,
                "creative" => UseCase::Creative,
                name => {
                    // Check if it's a known custom use case
                    let is_custom = config
                        .custom_use_cases
                        .as_ref()
                        .map(|m| m.contains_key(name))
                        .unwrap_or(false);
                    if !is_custom {
                        anyhow::bail!(
                            "Unknown use case '{}'. Use `kronk use-case list` to see available options, \
                             or `kronk use-case add {}` to create a custom one.",
                            name, name
                        );
                    }
                    UseCase::Custom {
                        name: name.to_string(),
                    }
                }
            };

            prof.use_case = Some(uc);
            config.save()?;

            println!("Oh yeah, it's all coming together.");
            println!("  Profile '{}' now uses '{}' preset.", profile, use_case);

            Ok(())
        }
        UseCaseCommands::Clear { profile } => {
            let mut config = config.clone();
            let prof = config
                .profiles
                .get_mut(&profile)
                .with_context(|| format!("Profile '{}' not found", profile))?;

            prof.use_case = None;
            config.save()?;

            println!("Use case cleared for profile '{}'.", profile);
            Ok(())
        }
        UseCaseCommands::Add {
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
                anyhow::bail!("At least one sampling parameter is required. Example:\n  kronk use-case add my-preset --temp 0.4 --top-k 30");
            }

            let custom = config.custom_use_cases.get_or_insert_with(HashMap::new);
            custom.insert(name.clone(), params);
            config.save()?;

            println!("Custom use case '{}' created.", name);
            println!("Assign it: kronk use-case set <profile> {}", name);
            Ok(())
        }
        UseCaseCommands::Remove { name } => {
            let mut config = config.clone();
            let removed = config
                .custom_use_cases
                .as_mut()
                .and_then(|m| m.remove(&name))
                .is_some();

            if !removed {
                anyhow::bail!("Custom use case '{}' not found", name);
            }

            config.save()?;
            println!("Custom use case '{}' removed.", name);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
