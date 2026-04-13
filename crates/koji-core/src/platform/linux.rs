use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Manage systemd services on Linux.
pub fn install_service(
    service_name: &str,
    exe_path: &str,
    args: &[String],
    _port: u16,
    system: bool,
) -> Result<()> {
    let config_dir = service_dir(system)?;
    fs::create_dir_all(&config_dir).context("Failed to create systemd dir")?;

    let args_str = args.join(" ");
    let wanted_by = if system {
        "multi-user.target"
    } else {
        "default.target"
    };
    let unit = format!(
        "[Unit]\n\
         Description=Koji: {service_name}\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe_path} {args_str}\n\
         Restart=always\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy={wanted_by}\n"
    );

    let unit_path = config_dir.join(format!("{}.service", service_name));
    fs::write(&unit_path, unit).context("Failed to write unit file")?;

    systemctl(system, &["daemon-reload"]).context("Failed to reload systemd")?;

    systemctl(system, &["enable", service_name]).context("Failed to enable service")?;

    Ok(())
}

/// Install the koji proxy as a systemd service.
pub fn install_proxy_service(system: bool) -> Result<()> {
    let config_dir = service_dir(system)?;
    fs::create_dir_all(&config_dir).context("Failed to create systemd dir")?;

    let exe_path = std::env::current_exe()
        .context("Failed to get current exe path")?
        .display()
        .to_string();

    let wanted_by = if system {
        "multi-user.target"
    } else {
        "default.target"
    };
    let unit = format!(
        "[Unit]\n\
         Description=Koji\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe_path} service-run --proxy\n\
         Restart=always\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy={wanted_by}\n"
    );

    let unit_path = config_dir.join("koji.service");
    fs::write(&unit_path, unit).context("Failed to write unit file")?;

    systemctl(system, &["daemon-reload"]).context("Failed to reload systemd")?;

    systemctl(system, &["enable", "koji"]).context("Failed to enable service")?;

    Ok(())
}

pub fn start_service(service_name: &str, system: bool) -> Result<()> {
    let status = systemctl(system, &["start", service_name]).context("Failed to start service")?;

    if !status.success() {
        anyhow::bail!("Failed to start service '{}'", service_name);
    }
    Ok(())
}

pub fn stop_service(service_name: &str, system: bool) -> Result<()> {
    stop_service_force(service_name, system)
}

pub fn stop_service_force(service_name: &str, system: bool) -> Result<()> {
    let status = systemctl(system, &["stop", service_name]).context("Failed to stop service")?;

    if !status.success() {
        anyhow::bail!("Failed to stop service '{}'", service_name);
    }

    // Wait for systemd to fully stop the service
    std::thread::sleep(std::time::Duration::from_secs(2));

    Ok(())
}

pub fn restart_service(service_name: &str, system: bool) -> Result<()> {
    stop_service_force(service_name, system)?;

    // Wait for processes to fully terminate
    std::thread::sleep(std::time::Duration::from_millis(500));

    start_service(service_name, system)
}

pub fn remove_service(service_name: &str, system: bool) -> Result<()> {
    let _ = systemctl(system, &["stop", service_name]);
    let _ = systemctl(system, &["disable", service_name]);

    let config_dir = service_dir(system)?;
    let unit_path = config_dir.join(format!("{}.service", service_name));
    if unit_path.exists() {
        fs::remove_file(&unit_path).context("Failed to remove unit file")?;
    }

    systemctl(system, &["daemon-reload"]).context("Failed to reload systemd")?;

    Ok(())
}

pub fn query_service(service_name: &str, system: bool) -> Result<String> {
    let output = if system {
        Command::new("systemctl")
            .args(["is-active", service_name])
            .output()
    } else {
        Command::new("systemctl")
            .args(["--user", "is-active", service_name])
            .output()
    }
    .context("Failed to query service")?;

    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match state.as_str() {
        "active" => Ok("RUNNING".to_string()),
        "inactive" => Ok("STOPPED".to_string()),
        "failed" => Ok("FAILED".to_string()),
        _ => Ok("NOT_INSTALLED".to_string()),
    }
}

/// Query a service, checking user mode first then system mode.
/// Use this when the caller doesn't know which mode the service was installed in.
pub fn auto_query_service(service_name: &str) -> Result<String> {
    let user_state = query_service(service_name, false)?;
    if user_state != "NOT_INSTALLED" {
        return Ok(user_state);
    }
    query_service(service_name, true)
}

/// Restart a service, trying user mode first then system mode.
/// Use this when the caller doesn't know which mode the service was installed in.
pub fn auto_restart_service(service_name: &str) -> Result<()> {
    let user_state = query_service(service_name, false)?;
    if user_state != "NOT_INSTALLED" {
        return restart_service(service_name, false);
    }
    restart_service(service_name, true)
}

/// Detect whether a service is installed as system or user.
///
/// Returns `true` for system, `false` for user. Checks whether the unit
/// file exists on disk rather than relying on `is-active`, which reports
/// "inactive" for unknown units.
pub fn detect_service_mode(service_name: &str) -> Result<bool> {
    let unit_file = format!("{}.service", service_name);

    // Check user service dir
    if let Ok(user_dir) = service_dir(false) {
        if user_dir.join(&unit_file).exists() {
            return Ok(false); // user service
        }
    }

    // Check system service dir
    let system_dir = service_dir(true)?;
    if system_dir.join(&unit_file).exists() {
        return Ok(true); // system service
    }

    anyhow::bail!(
        "Service '{}' is not installed as either a user or system service",
        service_name
    );
}

/// Run a systemctl command, choosing system or user mode.
fn systemctl(system: bool, args: &[&str]) -> Result<std::process::ExitStatus> {
    if system {
        Command::new("systemctl").args(args).status()
    } else {
        // Prepend --user before the rest of the args
        let mut full_args = vec!["--user"];
        full_args.extend_from_slice(args);
        Command::new("systemctl").args(&full_args).status()
    }
    .context("Failed to run systemctl")
}

fn service_dir(system: bool) -> Result<PathBuf> {
    if system {
        Ok(PathBuf::from("/etc/systemd/system"))
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home).join(".config/systemd/user"))
    }
}
