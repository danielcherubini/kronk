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

/// Configure a child process to be spawned in its own process group.
/// On Unix, uses process_group(0) to create a new session.
/// On Windows, uses CREATE_NEW_PROCESS_GROUP flag.
/// Call this before spawning any backend process.
pub fn configure_process_group(cmd: &mut tokio::process::Command) {
    #[cfg(unix)]
    {
        #[allow(unused_imports)]
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        #[allow(unused_imports)]
        use std::os::windows::process::CommandExt;
        // CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x00000200);
    }
}

/// Send SIGTERM to an entire process group (Unix) or kill the process tree (Windows).
/// On Unix, negative PID in kill() targets the process group.
/// On Windows, delegates to kill_process() which uses taskkill /T (tree kill).
pub async fn kill_process_group(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill with a negative PID targets the entire process group.
        // The PID was obtained from a successfully spawned child process and is guaranteed > 0.
        // SIGTERM is a standard POSIX signal. The call cannot access invalid memory.
        let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGTERM) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            // ESRCH = no such process group, which is fine (already dead)
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(anyhow::anyhow!(
                    "Failed to send SIGTERM to process group {}: {}",
                    pid,
                    err
                ));
            }
        }
    }
    #[cfg(windows)]
    {
        kill_process(pid).await?;
    }
    Ok(())
}

/// Send SIGKILL to an entire process group (Unix) or force-kill the process tree (Windows).
/// On Windows, delegates to force_kill_process() which uses taskkill /T /F (forceful tree kill).
pub async fn force_kill_process_group(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill with a negative PID targets the entire process group.
        // The PID was obtained from a successfully spawned child process and is guaranteed > 0.
        // SIGKILL is a standard POSIX signal. The call cannot access invalid memory.
        let ret = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
        if ret != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(anyhow::anyhow!(
                    "Failed to send SIGKILL to process group {}: {}",
                    pid,
                    err
                ));
            }
        }
    }
    #[cfg(windows)]
    {
        force_kill_process(pid).await?;
    }
    Ok(())
}

/// Check if a process group leader (by PID) is still alive.
/// If the leader is dead, the group is effectively dead.
pub fn is_process_group_alive(pid: u32) -> bool {
    is_process_alive(pid)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_kill_process_group_nonexistent_pid_returns_ok() {
        // Use a PID that definitely doesn't exist.
        // ESRCH should be handled gracefully.
        let result = kill_process_group(99999999).await;
        assert!(
            result.is_ok(),
            "ESRCH should be treated as OK: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_force_kill_process_group_nonexistent_pid_returns_ok() {
        // Same for SIGKILL variant.
        let result = force_kill_process_group(99999999).await;
        assert!(
            result.is_ok(),
            "ESRCH should be treated as OK: {:?}",
            result
        );
    }

    #[cfg(unix)]
    #[allow(unused_imports)]
    #[tokio::test]
    async fn test_process_group_kills_children() {
        // Spawn a shell that forks a child (sleep 100) then exits immediately.
        // The shell runs in its own process group (we set it up).
        use std::os::unix::process::CommandExt;
        let mut child = TokioCommand::new("/bin/sh");
        child.process_group(0);
        child.arg("-c").arg("sleep 100 & exit 0");
        let mut child = child.spawn().unwrap();
        let pid = child.id().unwrap();

        // Give the child time to fork
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Kill the process group
        kill_process_group(pid)
            .await
            .expect("kill_process_group should succeed");

        // Wait briefly for signals to propagate
        tokio::time::sleep(Duration::from_millis(500)).await;

        // The parent shell exited on its own, but the child (sleep 100) should be killed.
        let _ = child.wait().await;

        // Verify: check that no "sleep 100" process is still running.
        // We use pgrep to find any sleep processes started recently.
        // If the process group kill worked, there should be no orphan.
        let pgrep = std::process::Command::new("pgrep")
            .args(["-f", "sleep 100"])
            .output()
            .ok();
        let orphans = pgrep
            .as_ref()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().len())
            .unwrap_or(0);
        assert!(
            orphans == 0,
            "Expected no orphan 'sleep 100' processes, found {}",
            orphans
        );
    }
}
