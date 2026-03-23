//! Service command handler
//!
//! Handles `kronk service install/start/stop/remove` commands.

use anyhow::Result;
use kronk_core::config::Config;

/// Manage system services (Windows and Linux)
pub fn cmd_service(config: &Config, command: crate::cli::ServiceCommands) -> Result<()> {
    match command {
        crate::cli::ServiceCommands::Install { name } => {
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
        crate::cli::ServiceCommands::Start { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "kronk".to_string());
            service_start_inner(&service_name)?;
            println!("Pull the lever! '{}' started.", service_name);
        }
        crate::cli::ServiceCommands::Stop { name } => {
            let service_name = name
                .map(|n| Config::service_name(&n))
                .unwrap_or_else(|| "kronk".to_string());
            service_stop_inner(&service_name)?;
            println!("Wrong lever! '{}' stopped.", service_name);
        }
        crate::cli::ServiceCommands::Remove { name } => {
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

/// Start a service
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

/// Stop a service
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

/// Build the full argument list for a server, resolving model card args at runtime.
fn build_full_args(
    config: &Config,
    server: &kronk_core::config::ModelConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> anyhow::Result<Vec<String>> {
    config.build_full_args(server, backend, ctx_override)
}
