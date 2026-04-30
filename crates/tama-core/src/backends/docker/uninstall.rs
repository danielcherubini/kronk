/// Docker container uninstallation.
///
/// Stops the container via `docker compose down`, kills it if still running,
/// and cleans up disk files.
use anyhow::{Context, Result};
use tokio::process::Command;

use super::DockerBackend;

/// Stop a Docker container and clean up.
///
/// Steps:
/// 1. Run `docker compose -f <path> down -t 5`
/// 2. If container still running, run `docker kill <container_id>`
/// 3. Clean up disk files in config_dir/docker/{name}/
pub async fn stop_container(backend: &DockerBackend) -> Result<()> {
    let compose_path = backend.compose_path();
    let container_name = backend.container_name();

    // Step 1: Try graceful shutdown
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            compose_path.to_string_lossy().as_ref(),
            "down",
            "-t",
            "5",
        ])
        .output()
        .await
        .context("Failed to execute 'docker compose down'")?;

    // Step 2: If still running, kill it
    if output.status.success() {
        // Graceful shutdown succeeded
    } else {
        // Try to kill the container directly
        let kill_output = Command::new("docker")
            .args(["kill", &container_name])
            .output()
            .await
            .context("Failed to execute 'docker kill'")?;

        if !kill_output.status.success() {
            let stderr = String::from_utf8_lossy(&kill_output.stderr);
            // Container may already be stopped — this is OK
            let stderr_str = stderr.trim();
            if !stderr_str.contains("No such container") {
                tracing::warn!("Failed to kill container: {}", stderr_str);
            }
        }
    }

    // Step 3: Clean up disk files
    let dir = backend.config_dir.join("docker").join(&backend.name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("Failed to clean up directory: {}", dir.display()))?;
    }

    Ok(())
}
