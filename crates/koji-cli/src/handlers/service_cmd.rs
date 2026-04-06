//! Service command handler
//!
//! Handles `koji service install/start/stop/remove` commands.

use anyhow::Result;
use koji_core::config::Config;

/// Manage system services (Windows and Linux)
pub fn cmd_service(config: &Config, command: crate::cli::ServiceCommands) -> Result<()> {
    match command {
        crate::cli::ServiceCommands::Install { name } => {
            if let Some(server_name) = name {
                // Legacy: install a single backend as a service
                let (srv, backend) = config.resolve_server(&server_name)?;
                #[cfg(target_os = "windows")]
                let _ = &backend;
                let service_name = Config::service_name(&server_name);

                #[cfg(target_os = "windows")]
                {
                    let display_name = format!("Koji: {}", server_name);
                    let config_dir = Config::base_dir()?;
                    let port = srv.port.unwrap_or(8080);
                    koji_core::platform::windows::install_service(
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
                        config.resolve_backend_path(&srv.backend, &conn)?
                    };
                    let backend_path_str = backend_path.to_string_lossy().to_string();
                    koji_core::platform::linux::install_service(
                        &service_name,
                        &backend_path_str,
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
                    koji_core::platform::windows::install_proxy_service(&config_dir, port)?;
                }

                #[cfg(target_os = "linux")]
                koji_core::platform::linux::install_proxy_service()?;

                #[cfg(not(any(target_os = "windows", target_os = "linux")))]
                anyhow::bail!("Service management not supported on this platform");

                println!("Installed koji service.");
                println!("Start it: koji service start");
            }
        }
        crate::cli::ServiceCommands::Start { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "koji".to_string());
            service_start_inner(&service_name)?;
            println!("Started '{}'.", service_name);
        }
        crate::cli::ServiceCommands::Stop { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "koji".to_string());
            service_stop_inner(&service_name)?;
            println!("Stopped '{}'.", service_name);
        }
        crate::cli::ServiceCommands::Restart { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "koji".to_string());
            service_restart_inner(&service_name)?;
            println!("Restarted '{}'.", service_name);
        }
        crate::cli::ServiceCommands::Remove { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "koji".to_string());

            #[cfg(target_os = "windows")]
            koji_core::platform::windows::remove_service(&service_name)?;

            #[cfg(target_os = "linux")]
            koji_core::platform::linux::remove_service(&service_name)?;

            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = service_name;
                anyhow::bail!("Not supported on this platform");
            }

            println!("Removed '{}'.", service_name);
        }
    }
    Ok(())
}

/// Start a service
#[allow(dead_code)]
fn service_start_inner(service_name: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        koji_core::platform::windows::start_service(service_name)?;
    }

    #[cfg(target_os = "linux")]
    {
        koji_core::platform::linux::start_service(service_name)?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = service_name;
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

/// Stop a service
#[allow(dead_code)]
fn service_stop_inner(service_name: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        // First try normal stop, then forcefully kill processes
        koji_core::platform::windows::stop_service_force(service_name)?;
    }

    #[cfg(target_os = "linux")]
    {
        koji_core::platform::linux::stop_service(service_name)?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = service_name;
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}

/// Restart a service (stop then start)
#[allow(dead_code)]
fn service_restart_inner(service_name: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        // Stop forcefully first
        koji_core::platform::windows::restart_service(service_name)?;
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, just stop and start
        koji_core::platform::linux::restart_service(service_name)?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = service_name;
        anyhow::bail!("Not supported on this platform");
    }

    Ok(())
}
