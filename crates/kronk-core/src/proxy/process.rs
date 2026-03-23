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
/// Uses `kill(pid, 0)` on Unix (POSIX-portable across Linux/macOS/BSD)
/// and `tasklist` with exact PID column matching on Windows.
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // POSIX-portable: kill(pid, 0) checks process existence without
        // sending a signal. Returns 0 if alive, -1 with ESRCH if not.
        // EPERM means the process exists but we lack permission to signal it.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        // Check errno: ESRCH = no such process, EPERM = exists but no permission
        let err = std::io::Error::last_os_error();
        err.raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        // On Windows, use tasklist to check if PID is running.
        // Parse line-by-line and match the PID column exactly to avoid
        // substring false positives (e.g. PID 12 matching PID 123).
        let pid_str = pid.to_string();
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH", "/FO", "CSV"])
            .output()
            .map(|o| {
                let output = String::from_utf8_lossy(&o.stdout);
                output.lines().any(|line| {
                    // CSV format: "name","pid","session","session#","mem"
                    line.split(',')
                        .nth(1)
                        .map(|col| col.trim_matches('"').trim() == pid_str)
                        .unwrap_or(false)
                })
            })
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
