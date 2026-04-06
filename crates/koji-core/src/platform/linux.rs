use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Manage systemd user services on Linux.
pub fn install_service(
    service_name: &str,
    exe_path: &str,
    args: &[String],
    _port: u16,
) -> Result<()> {
    let config_dir = systemd_user_dir()?;
    fs::create_dir_all(&config_dir).context("Failed to create systemd user dir")?;

    let args_str = args.join(" ");
    let unit = format!(
        "[Unit]\n\
         Description=Kronk: {service_name}\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe_path} {args_str}\n\
         Restart=always\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );

    let unit_path = config_dir.join(format!("{}.service", service_name));
    fs::write(&unit_path, unit).context("Failed to write unit file")?;

    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("Failed to reload systemd")?;

    Command::new("systemctl")
        .args(["--user", "enable", service_name])
        .status()
        .context("Failed to enable service")?;

    Ok(())
}

/// Install the kronk proxy as a systemd user service.
pub fn install_proxy_service() -> Result<()> {
    let config_dir = systemd_user_dir()?;
    fs::create_dir_all(&config_dir).context("Failed to create systemd user dir")?;

    let exe_path = std::env::current_exe()
        .context("Failed to get current exe path")?
        .display()
        .to_string();

    let unit = format!(
        "[Unit]\n\
         Description=Kronk\n\
         After=network.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe_path} service-run --proxy\n\
         Restart=always\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    );

    let unit_path = config_dir.join("kronk.service");
    fs::write(&unit_path, unit).context("Failed to write unit file")?;

    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("Failed to reload systemd")?;

    Command::new("systemctl")
        .args(["--user", "enable", "kronk"])
        .status()
        .context("Failed to enable service")?;

    Ok(())
}

pub fn start_service(service_name: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "start", service_name])
        .status()
        .context("Failed to start service")?;

    if !status.success() {
        anyhow::bail!("Failed to start service '{}'", service_name);
    }
    Ok(())
}

pub fn stop_service(service_name: &str) -> Result<()> {
    stop_service_force(service_name)
}

pub fn stop_service_force(service_name: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "stop", service_name])
        .status()
        .context("Failed to stop service")?;

    if !status.success() {
        anyhow::bail!("Failed to stop service '{}'", service_name);
    }

    // Wait for systemd to fully stop the service
    std::thread::sleep(std::time::Duration::from_secs(2));

    Ok(())
}

pub fn restart_service(service_name: &str) -> Result<()> {
    stop_service_force(service_name)?;

    // Wait for processes to fully terminate
    std::thread::sleep(std::time::Duration::from_millis(500));

    start_service(service_name)
}

pub fn remove_service(service_name: &str) -> Result<()> {
    let _ = Command::new("systemctl")
        .args(["--user", "stop", service_name])
        .status();

    let _ = Command::new("systemctl")
        .args(["--user", "disable", service_name])
        .status();

    let config_dir = systemd_user_dir()?;
    let unit_path = config_dir.join(format!("{}.service", service_name));
    if unit_path.exists() {
        fs::remove_file(&unit_path).context("Failed to remove unit file")?;
    }

    Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("Failed to reload systemd")?;

    Ok(())
}

pub fn query_service(service_name: &str) -> Result<String> {
    let output = Command::new("systemctl")
        .args(["--user", "is-active", service_name])
        .output()
        .context("Failed to query service")?;

    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    match state.as_str() {
        "active" => Ok("RUNNING".to_string()),
        "inactive" => Ok("STOPPED".to_string()),
        "failed" => Ok("FAILED".to_string()),
        _ => Ok("NOT_INSTALLED".to_string()),
    }
}

fn systemd_user_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}
