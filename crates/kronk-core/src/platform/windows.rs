use anyhow::{Context, Result};
use std::ffi::OsString;
use std::time::{Duration, Instant};
use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

/// Poll a service until it reaches the desired state, or timeout.
/// Uses exponential backoff starting at 100ms, capped at 2s per poll.
fn wait_for_state(
    service: &windows_service::service::Service,
    desired: ServiceState,
    timeout: Duration,
) -> Result<()> {
    let start = Instant::now();
    let mut interval = Duration::from_millis(100);
    let max_interval = Duration::from_secs(2);

    loop {
        let status = service
            .query_status()
            .context("Failed to query service status while waiting")?;
        if status.current_state == desired {
            return Ok(());
        }
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out waiting for service to reach {:?} (current: {:?})",
                desired,
                status.current_state,
            );
        }
        std::thread::sleep(interval);
        interval = (interval * 2).min(max_interval);
    }
}

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
            wait_for_state(&existing, ServiceState::Stopped, Duration::from_secs(30))
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
                Err(_) => break, // Service gone — proceed
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
    add_firewall_rule(service_name, port).ok();

    // Grant Interactive Users permission to start/stop the service
    // This allows the user to control the service without elevation
    grant_user_control(service_name)
        .with_context(|| format!("Failed to set service permissions for '{}'", service_name))?;

    Ok(())
}

/// Resolve the SID of the current user via `whoami /user`.
/// Returns the SID string, e.g. "S-1-5-21-1234567890-1234567890-1234567890-1001".
fn get_current_user_sid() -> Result<String> {
    let output = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .context("Failed to run 'whoami /user' — is this a Windows system?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("whoami failed (exit {}): {}", output.status, stderr.trim());
    }

    // Output format: "DOMAIN\User","S-1-5-21-..."
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();

    // Parse CSV: split on "," and take the second field, strip quotes
    let sid = line
        .split(',')
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| s.starts_with("S-1-"))
        .with_context(|| format!("Failed to parse SID from whoami output: {}", line))?;

    Ok(sid)
}

/// Grant the installing user permission to start, stop, and query the service.
/// Resolves the current user's SID and applies it via `sc sdset`, so only the
/// installer (plus SYSTEM and Administrators) can control the service.
fn grant_user_control(service_name: &str) -> Result<()> {
    let user_sid = match get_current_user_sid() {
        Ok(sid) => {
            tracing::info!("Granting service control to user SID: {}", sid);
            sid
        }
        Err(e) => {
            tracing::warn!(
                "Could not resolve current user SID ({}), falling back to Interactive Users (IU)",
                e
            );
            "IU".to_string()
        }
    };

    // SDDL breakdown:
    //   SY  = Local System: full control
    //   BA  = Builtin Administrators: full control
    //   <SID> = Installing user: start (RP), stop (WP), query status (LC), query config (LO), read (CR)
    let sddl = format!(
        "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;RPWPLCLOCR;;;{})",
        user_sid
    );

    tracing::debug!("Setting service SDDL for '{}': {}", service_name, sddl);

    let output = std::process::Command::new("sc")
        .args(["sdset", service_name, &sddl])
        .output()
        .context("Failed to run sc sdset")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "sc sdset {} failed (exit {}): {}",
            service_name,
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}

/// Add a Windows Firewall rule to allow inbound TCP on the given port.
pub fn add_firewall_rule(name: &str, port: u16) -> Result<()> {
    let rule_name = format!("Kronk: {}", name);

    // Remove existing rule if present
    std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={}", rule_name),
        ])
        .output()
        .ok();

    let status = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={}", rule_name),
            "dir=in",
            "action=allow",
            "protocol=TCP",
            &format!("localport={}", port),
        ])
        .output()
        .context("Failed to run netsh")?;

    if !status.status.success() {
        anyhow::bail!("Failed to add firewall rule");
    }

    Ok(())
}

/// Start an installed service.
pub fn start_service(service_name: &str) -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open Service Control Manager — run as Administrator")?;

    let service = manager
        .open_service(
            service_name,
            ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        )
        .with_context(|| format!("Service '{}' not found", service_name))?;

    let status = service.query_status()?;
    if status.current_state == ServiceState::Running {
        return Ok(());
    }

    service
        .start::<String>(&[])
        .context("Failed to start service")?;

    Ok(())
}

/// Stop a running service.
pub fn stop_service(service_name: &str) -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open Service Control Manager — run as Administrator")?;

    let service = manager
        .open_service(
            service_name,
            ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
        )
        .with_context(|| format!("Service '{}' not found", service_name))?;

    let status = service.query_status()?;
    if status.current_state == ServiceState::Stopped {
        return Ok(());
    }

    service.stop().context("Failed to stop service")?;

    Ok(())
}

/// Remove an installed service.
pub fn remove_service(service_name: &str) -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open Service Control Manager — run as Administrator")?;

    let service = manager
        .open_service(
            service_name,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )
        .with_context(|| format!("Service '{}' not found", service_name))?;

    // Stop if running, then wait for it to actually stop
    let status = service.query_status()?;
    if status.current_state != ServiceState::Stopped {
        let _ = service.stop();
        wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(30))
            .with_context(|| format!("Service '{}' did not stop in time", service_name))?;
    }

    service.delete().context("Failed to delete service")?;

    // Remove firewall rule
    remove_firewall_rule(service_name).ok();

    Ok(())
}

/// Remove a firewall rule by service name.
pub fn remove_firewall_rule(name: &str) -> Result<()> {
    let rule_name = format!("Kronk: {}", name);
    std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={}", rule_name),
        ])
        .output()
        .context("Failed to run netsh")?;
    Ok(())
}

/// Query the status of a service.
pub fn query_service(service_name: &str) -> Result<String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open Service Control Manager")?;

    match manager.open_service(service_name, ServiceAccess::QUERY_STATUS) {
        Ok(service) => {
            let status = service.query_status()?;
            let state = match status.current_state {
                ServiceState::Stopped => "STOPPED",
                ServiceState::StartPending => "STARTING",
                ServiceState::StopPending => "STOPPING",
                ServiceState::Running => "RUNNING",
                ServiceState::ContinuePending => "RESUMING",
                ServiceState::PausePending => "PAUSING",
                ServiceState::Paused => "PAUSED",
            };
            Ok(state.to_string())
        }
        Err(_) => Ok("NOT_INSTALLED".to_string()),
    }
}
