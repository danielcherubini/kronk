//! Windows service dispatch and management
//!
//! This module handles Windows Service Control Manager (SCM) integration
//! and cross-platform service management.

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
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
#[cfg(target_os = "windows")]
use windows_service::service_control_handler;

#[cfg(target_os = "windows")]
use anyhow::{Context, Result};
#[cfg(target_os = "windows")]
use kronk_core::config::Config;
#[cfg(target_os = "windows")]
use kronk_core::proxy::server::ProxyServer;
#[cfg(target_os = "windows")]
use kronk_core::proxy::ProxyState;
#[cfg(target_os = "windows")]
use std::net::SocketAddr;
#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use tokio::sync::mpsc;

/// Windows Service dispatch handler
#[cfg(target_os = "windows")]
pub fn service_dispatch() -> Result<()> {
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

/// Windows service main entry point
#[cfg(target_os = "windows")]
pub fn win_service_main(_arguments: Vec<std::ffi::OsString>) {
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
        .or_else(|_| std::fs::File::create("kronk-service.log"))
        .or_else(|_| std::fs::File::create(std::env::temp_dir().join("kronk-service.log")))
        .or_else(|_| {
            // Last resort: write to /dev/null (Unix) or NUL (Windows)
            #[cfg(unix)]
            {
                std::fs::File::create("/dev/null")
            }
            #[cfg(windows)]
            {
                std::fs::File::create("NUL")
            }
        });

    let log_file = match log_file {
        Ok(f) => f,
        Err(e) => {
            eprintln!("FATAL: Cannot create any log file: {}", e);
            return;
        }
    };

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
            let host = config.proxy.host.clone();
            let port = config.proxy.port;
            let (host_addr, _) = match host.parse::<std::net::IpAddr>() {
                Ok(addr) => (addr, false),
                Err(_) => (
                    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
                    true,
                ),
            };
            let addr = SocketAddr::new(host_addr, port);

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
            let log_dir = config.logs_dir().unwrap_or_else(|e| {
                tracing::warn!("Failed to get logs directory: {}, using current dir", e);
                std::path::PathBuf::from(".")
            });
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

/// Build the full argument list for a server, resolving model card args at runtime.
/// Merges: backend.default_args + server.args + model card (-m, -c, -ngl) + sampling
#[cfg(target_os = "windows")]
fn build_full_args(
    config: &Config,
    server: &kronk_core::config::ModelConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> anyhow::Result<Vec<String>> {
    config.build_full_args(server, backend, ctx_override)
}

// On Windows, use the real ProcessSupervisor from kronk_core
#[cfg(target_os = "windows")]
use kronk_core::process::ProcessSupervisor;

#[cfg(not(target_os = "windows"))]
pub fn service_dispatch() -> anyhow::Result<()> {
    anyhow::bail!("Service dispatch is only available on Windows");
}

#[cfg(not(target_os = "windows"))]
pub fn win_service_main(_arguments: Vec<std::ffi::OsString>) {
    // No-op on non-Windows
}
