use anyhow::{Context, Result};
use std::ffi::OsString;
use std::time::{Duration, Instant};
use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

/// Install kronk as a native Windows Service for the given server.
/// The service will run `kronk.exe service-run --server <name> --config-dir <path>` when started.
/// The config-dir is captured at install time from the installing user's environment,
/// so the service (running as SYSTEM) can find the correct config and models.
pub fn install_service(
    service_name: &str,
    display_name: &str,
    server_name: &str,
    config_dir: &std::path::Path,
    port: u16,
) -> Result<()> {
    let exe_path = std::env::current_exe().context("Failed to get current exe path")?;

    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .context("Failed to open Service Control Manager — run as Administrator")?;

    // Remove existing service if present
    if let Ok(existing) = manager.open_service(service_name, ServiceAccess::ALL_ACCESS) {
        let status = existing.query_status()?;
        if status.current_state != ServiceState::Stopped {
            existing.stop()?;
            super::service::wait_for_state(
                &existing,
                ServiceState::Stopped,
                Duration::from_secs(30),
            )
            .with_context(|| format!("Service '{}' did not stop in time", service_name))?;
        }
        existing.delete()?;
        // Drop the handle so SCM can finalize deletion
        drop(existing);

        // Wait for SCM to fully process the deletion by retrying open
        let delete_start = Instant::now();
        let delete_timeout = Duration::from_secs(10);
        loop {
            match manager.open_service(service_name, ServiceAccess::QUERY_STATUS) {
                Ok(_) => {
                    // Service still exists — SCM hasn't finalized yet
                    if delete_start.elapsed() > delete_timeout {
                        anyhow::bail!(
                            "Timed out waiting for SCM to delete service '{}'",
                            service_name
                        );
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => {
                    // Check if this is the "service does not exist" error (code 1060)
                    if let windows_service::Error::Winapi(ref io_err) = e {
                        if io_err.raw_os_error() == Some(1060) {
                            break; // Service gone — proceed
                        }
                    }
                    // For other errors, log and retry to distinguish transient from real failures
                    tracing::warn!("Error checking service deletion status: {}", e);
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    let service_info = ServiceInfo {
        name: OsString::from(service_name),
        display_name: OsString::from(display_name),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe_path,
        launch_arguments: vec![
            OsString::from("service-run"),
            OsString::from("--server"),
            OsString::from(server_name),
            OsString::from("--config-dir"),
            OsString::from(config_dir),
        ],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    manager
        .create_service(
            &service_info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START,
        )
        .context("Failed to create service — run as Administrator")?;

    // Add firewall rule for the profile's port
    super::firewall::add_firewall_rule(service_name, port).ok();

    // Grant Interactive Users permission to start/stop the service
    // This allows the user to control the service without elevation
    super::permissions::grant_user_control(service_name)
        .with_context(|| format!("Failed to set service permissions for '{}'", service_name))?;

    Ok(())
}

/// Install kronk proxy as a native Windows Service.
/// The service will run `kronk.exe service-run --proxy --config-dir <path>` when started.
pub fn install_proxy_service(config_dir: &std::path::Path, port: u16) -> Result<()> {
    let exe_path = std::env::current_exe().context("Failed to get current exe path")?;
    let service_name = "kronk";
    let display_name = "Kronk";

    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .context("Failed to open Service Control Manager — run as Administrator")?;

    // Remove existing service if present
    if let Ok(existing) = manager.open_service(service_name, ServiceAccess::ALL_ACCESS) {
        let status = existing.query_status()?;
        if status.current_state != ServiceState::Stopped {
            existing.stop()?;
            super::service::wait_for_state(
                &existing,
                ServiceState::Stopped,
                Duration::from_secs(30),
            )
            .with_context(|| format!("Service '{}' did not stop in time", service_name))?;
        }
        existing.delete()?;
        drop(existing);

        let delete_start = Instant::now();
        let delete_timeout = Duration::from_secs(10);
        loop {
            match manager.open_service(service_name, ServiceAccess::QUERY_STATUS) {
                Ok(_) => {
                    if delete_start.elapsed() > delete_timeout {
                        anyhow::bail!(
                            "Timed out waiting for SCM to delete service '{}'",
                            service_name
                        );
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => {
                    if let windows_service::Error::Winapi(ref io_err) = e {
                        if io_err.raw_os_error() == Some(1060) {
                            break;
                        }
                    }
                    tracing::warn!("Error checking service deletion status: {}", e);
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    let service_info = ServiceInfo {
        name: OsString::from(service_name),
        display_name: OsString::from(display_name),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe_path,
        launch_arguments: vec![
            OsString::from("service-run"),
            OsString::from("--proxy"),
            OsString::from("--config-dir"),
            OsString::from(config_dir),
        ],
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };

    manager
        .create_service(
            &service_info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START,
        )
        .context("Failed to create service — run as Administrator")?;

    super::firewall::add_firewall_rule(service_name, port).ok();
    super::permissions::grant_user_control(service_name)
        .with_context(|| format!("Failed to set service permissions for '{}'", service_name))?;

    Ok(())
}
