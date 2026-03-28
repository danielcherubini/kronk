use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use windows_service::service::{ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

/// Poll a service until it reaches the desired state, or timeout.
/// Uses exponential backoff starting at 100ms, capped at 2s per poll.
pub(super) fn wait_for_state(
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

    wait_for_state(&service, ServiceState::Running, Duration::from_secs(30))
        .with_context(|| format!("Service '{}' did not start in time", service_name))?;

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

    wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(30))
        .with_context(|| format!("Service '{}' did not stop in time", service_name))?;

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
        match service.stop() {
            Ok(_status) => {
                // Successfully initiated stop, now wait for it to complete
                wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(30))
                    .with_context(|| format!("Service '{}' did not stop in time", service_name))?;
            }
            Err(e) => {
                // If already in StopPending, wait for it to complete
                let stop_status = service.query_status()?;
                if stop_status.current_state == ServiceState::StopPending {
                    wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(30))
                        .with_context(|| {
                            format!("Service '{}' did not stop in time", service_name)
                        })?;
                } else {
                    // Propagate the stop error
                    return Err(e)
                        .with_context(|| format!("Failed to stop service '{}'", service_name));
                }
            }
        }
    }

    service.delete().context("Failed to delete service")?;

    // Remove firewall rule
    super::firewall::remove_firewall_rule(service_name).ok();

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
