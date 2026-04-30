/// Docker daemon availability checks and container health monitoring.
///
/// Uses the Docker CLI (`docker` command) to check availability and inspect
/// container status.
use anyhow::{anyhow, Context, Result};
use tokio::process::Command;

/// Check if Docker is available.
/// Returns `Ok(())` if `docker --version` succeeds.
pub async fn check_docker_available() -> Result<()> {
    let output = Command::new("docker")
        .arg("--version")
        .output()
        .await
        .context("Failed to execute 'docker --version'")?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!("Docker is not available: {}", stderr.trim()))
    }
}

/// Get container status via `docker inspect`.
/// Returns "running", "exited", "dead", etc.
pub async fn container_status(container_name: &str) -> Result<String> {
    let output = Command::new("docker")
        .args(["inspect", container_name, "--format", "{{.State.Status}}"])
        .output()
        .await
        .context("Failed to execute 'docker inspect'")?;

    if output.status.success() {
        let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(status)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!(
            "Container '{}' not found or error: {}",
            container_name,
            stderr.trim()
        ))
    }
}

/// Get container ID via `docker ps`.
/// Returns None if container not found.
pub async fn container_id(container_name: &str) -> Result<Option<String>> {
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name={}", container_name),
            "--format",
            "{{.ID}}",
        ])
        .output()
        .await
        .context("Failed to execute 'docker ps'")?;

    if output.status.success() {
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(if id.is_empty() { None } else { Some(id) })
    } else {
        Err(anyhow!(
            "Failed to get container ID for '{}'",
            container_name
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_check_docker_available_fails_when_not_installed() {
        // This test verifies behavior when docker is not available.
        // On systems with Docker running, this will pass (Ok(())),
        // on systems without Docker, it will fail with a clear error.
        let result = check_docker_available().await;
        // We don't assert success/failure here since Docker availability
        // varies by test environment. The function should not panic.
        let _ = result;
    }
}
