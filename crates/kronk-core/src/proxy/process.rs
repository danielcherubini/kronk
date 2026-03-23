use anyhow::{Context, Result};
use std::time::Duration;
use tokio::process::Command as TokioCommand;

/// Override a CLI flag's value in an argument list (e.g. --host, --port).
/// If the flag exists, replaces its value. If not, appends the flag and value.
pub fn override_arg(args: &mut Vec<String>, flag: &str, value: &str) {
    if let Some(pos) = args.iter().position(|a| a == flag) {
        if pos + 1 < args.len() {
            args[pos + 1] = value.to_string();
        } else {
            args.push(value.to_string());
        }
    } else {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

/// Check if a process is still alive by PID.
/// Note: Unix implementation uses /proc which is Linux-specific.
/// TODO: Use libc::kill(pid, 0) for POSIX-portable check on macOS/BSD.
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Check /proc/<pid> existence (Linux) as a no-dependency check
        std::path::Path::new(&format!("/proc/{}", pid)).exists()
    }
    #[cfg(windows)]
    {
        // On Windows, use tasklist to check if PID is running
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// Kill a process by PID (cross-platform).
/// On Unix, sends SIGTERM for graceful shutdown.
/// On Windows, uses `taskkill /T` without `/F` for graceful termination.
pub async fn kill_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .spawn()
            .with_context(|| format!("Failed to execute kill command for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!("Failed to send SIGTERM to PID {}", pid));
        }
    }
    #[cfg(windows)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .spawn()
            .with_context(|| format!("Failed to execute taskkill command for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "Failed to terminate process with PID {}",
                pid
            ));
        }
    }
    Ok(())
}

/// Forcefully kill a process by PID (SIGKILL on Unix, taskkill /F on Windows).
pub async fn force_kill_process(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .spawn()
            .with_context(|| format!("Failed to execute kill -KILL for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!("Failed to send SIGKILL to PID {}", pid));
        }
    }
    #[cfg(windows)]
    {
        let mut child: tokio::process::Child = TokioCommand::new("taskkill")
            .arg("/PID")
            .arg(pid.to_string())
            .arg("/T")
            .arg("/F")
            .spawn()
            .with_context(|| format!("Failed to execute taskkill /F for PID {}", pid))?;
        let status: std::process::ExitStatus = child.wait().await?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "Failed to forcefully terminate process with PID {}",
                pid
            ));
        }
    }
    Ok(())
}

/// Check the health of a backend by making a request to its health endpoint.
pub async fn check_health(url: &str, timeout: Option<u64>) -> Result<reqwest::Response> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout.unwrap_or(10)))
        .build()?;
    client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to check health: {}", url))
}
