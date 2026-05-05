//! Service command handler
//!
//! Handles `tama service install/start/stop/remove` commands.

use anyhow::Result;
use tama_core::config::Config;
use tama_core::db::OpenResult;

/// Manage system services (Windows and Linux)
pub fn cmd_service(config: &Config, command: crate::cli::ServiceCommands) -> Result<()> {
    match command {
        crate::cli::ServiceCommands::Install { name, system } => {
            if let Some(server_name) = name {
                // Legacy: install a single backend as a service
                let db_dir = tama_core::config::Config::config_dir()?;
                let OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;
                let model_configs = tama_core::db::load_model_configs(&conn)?;

                let (srv, backend) = config.resolve_server(&model_configs, &server_name)?;
                #[cfg(target_os = "windows")]
                let _ = (&backend, system);
                let service_name = Config::service_name(&server_name);

                #[cfg(target_os = "windows")]
                {
                    let display_name = format!("Tama: {}", server_name);
                    let config_dir = Config::base_dir()?;
                    let port = srv.port.unwrap_or(8080);
                    tama_core::platform::windows::install_service(
                        &service_name,
                        &display_name,
                        &server_name,
                        &config_dir,
                        port,
                    )?;
                }

                #[cfg(target_os = "linux")]
                {
                    let args = config.build_full_args(srv, backend, None)?;
                    let port = srv.port.unwrap_or(8080);
                    // Resolve backend binary path from DB (priority) or config.path (fallback)
                    let backend_path = {
                        let conn = Config::open_db();
                        config.resolve_backend_path(&srv.backend, srv.gpu_variant.as_deref(), &conn)?
                    };
                    let backend_path_str = backend_path.to_string_lossy().to_string();
                    tama_core::platform::linux::install_service(
                        &service_name,
                        &backend_path_str,
                        &args,
                        port,
                        system,
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
                    let _ = system;
                    let config_dir = Config::base_dir()?;
                    let port = config.proxy.port;
                    tama_core::platform::windows::install_proxy_service(&config_dir, port)?;
                }

                #[cfg(target_os = "linux")]
                tama_core::platform::linux::install_proxy_service(system)?;

                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                {
                    let _ = system;
                    anyhow::bail!("Service management not supported on this platform");
                }

                if system {
                    println!("Installed tama system service.");
                } else {
                    println!("Installed tama service.");
                }
                println!(
                    "Start it: tama service start{}",
                    if system { " --system" } else { "" }
                );
            }
        }
        crate::cli::ServiceCommands::Start { name, system } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "tama".to_string());
            let system = resolve_system_flag(system, &service_name);
            service_start_inner(&service_name, system)?;
            println!("Started '{}'.", service_name);
        }
        crate::cli::ServiceCommands::Stop { name, system } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "tama".to_string());
            let system = resolve_system_flag(system, &service_name);
            service_stop_inner(&service_name, system)?;
            println!("Stopped '{}'.", service_name);
        }
        crate::cli::ServiceCommands::Restart { name, system } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "tama".to_string());
            let system = resolve_system_flag(system, &service_name);
            service_restart_inner(&service_name, system)?;
            println!("Restarted '{}'.", service_name);
        }
        crate::cli::ServiceCommands::Remove { name, system } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "tama".to_string());
            let system = resolve_system_flag(system, &service_name);

            #[cfg(target_os = "windows")]
            {
                let _ = system;
                tama_core::platform::windows::remove_service(&service_name)?;
            }

            #[cfg(target_os = "linux")]
            tama_core::platform::linux::remove_service(&service_name, system)?;

            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = (service_name, system);
                anyhow::bail!("Not supported on this platform");
            }

            println!("Removed '{}'.", service_name);
        }
    }
    Ok(())
}

/// Start a service
#[allow(dead_code)]
fn service_start_inner(service_name: &str, system: bool) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = system;
        tama_core::platform::windows::start_service(service_name)?;
    }

    #[cfg(target_os = "linux")]
    {
        tama_core::platform::linux::start_service(service_name, system)?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = (service_name, system);
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

/// Stop a service
#[allow(dead_code)]
fn service_stop_inner(service_name: &str, system: bool) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = system;
        // First try normal stop, then forcefully kill processes
        tama_core::platform::windows::stop_service_force(service_name)?;
    }

    #[cfg(target_os = "linux")]
    {
        tama_core::platform::linux::stop_service(service_name, system)?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = (service_name, system);
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

/// Restart a service (stop then start)
#[allow(dead_code)]
fn service_restart_inner(service_name: &str, system: bool) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        let _ = system;
        // Stop forcefully first
        tama_core::platform::windows::restart_service(service_name)?;
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, just stop and start
        tama_core::platform::linux::restart_service(service_name, system)?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = (service_name, system);
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

/// When `--system` is not passed, auto-detect whether the service is
/// installed as a system or user service. Falls back to user (false) if
/// detection fails (e.g. the service isn't installed yet).
#[cfg(target_os = "linux")]
fn resolve_system_flag(explicit: bool, service_name: &str) -> bool {
    if explicit {
        return true;
    }
    tama_core::platform::linux::detect_service_mode(service_name).unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn resolve_system_flag(explicit: bool, _service_name: &str) -> bool {
    explicit
}
