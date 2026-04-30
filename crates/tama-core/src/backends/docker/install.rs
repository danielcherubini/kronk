/// Docker container installation and startup.
///
/// Creates the compose YAML with injected container name, writes it to disk,
/// and starts the container via `docker compose up -d`.
use anyhow::{anyhow, Context, Result};
use serde_yml;
use tokio::process::Command;

use super::DockerBackend;

/// Start a Docker container from the given backend configuration.
///
/// Steps:
/// 1. Create config_dir/docker/{name}/ directory
/// 2. Inject container_name and network_mode into compose YAML
/// 3. Write compose YAML to disk
/// 4. Write Dockerfile if provided
/// 5. Run `docker compose -f <path> up -d`
/// 6. Extract and return container_id
pub async fn start_container(backend: &DockerBackend) -> Result<String> {
    // Step 1: Create the directory
    let dir = backend.config_dir.join("docker").join(&backend.name);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))?;

    // Step 2: Inject container_name and network_mode into compose YAML
    let injected_yaml = inject_container_name(&backend.compose_yaml, &backend.container_name())
        .with_context(|| "Failed to inject container name into compose YAML")?;

    // Step 3: Write compose YAML to disk
    let compose_path = dir.join("docker-compose.yaml");
    std::fs::write(&compose_path, &injected_yaml)
        .with_context(|| format!("Failed to write compose YAML to {}", compose_path.display()))?;

    // Step 4: Write Dockerfile if provided
    if let Some(ref dockerfile) = backend.dockerfile {
        let dockerfile_path = dir.join("Dockerfile");
        std::fs::write(&dockerfile_path, dockerfile).with_context(|| {
            format!(
                "Failed to write Dockerfile to {}",
                dockerfile_path.display()
            )
        })?;
    }

    // Step 5: Start the container
    let output = Command::new("docker")
        .args([
            "compose",
            "-f",
            compose_path.to_string_lossy().as_ref(),
            "up",
            "-d",
        ])
        .output()
        .await
        .context("Failed to execute 'docker compose up -d'")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "Failed to start Docker container: {}",
            stderr.trim()
        ));
    }

    // Step 6: Get container ID
    let container_name = backend.container_name();
    let container_id = get_container_id(&container_name).await?;

    Ok(container_id)
}

/// Inject container_name and network_mode into the compose YAML.
///
/// For each service in the compose file, sets:
/// - `container_name: tama_{name}`
/// - `network_mode: host`
///
/// If the user already set these values, they are overwritten.
fn inject_container_name(yaml: &str, container_name: &str) -> Result<String> {
    let mut parsed: serde_yml::Value =
        serde_yml::from_str(yaml).map_err(|e| anyhow!("Failed to parse compose YAML: {}", e))?;

    // Navigate to the services map
    if let Some(services) = parsed.get_mut("services").and_then(|v| v.as_mapping_mut()) {
        for (_, service) in services.iter_mut() {
            if let Some(service_map) = service.as_mapping_mut() {
                // Set container_name
                service_map.insert(
                    serde_yml::Value::String("container_name".to_string()),
                    serde_yml::Value::String(container_name.to_string()),
                );
                // Set network_mode
                service_map.insert(
                    serde_yml::Value::String("network_mode".to_string()),
                    serde_yml::Value::String("host".to_string()),
                );
            }
        }
    }

    // Serialize back to string
    let result = serde_yml::to_string(&parsed)
        .map_err(|e| anyhow!("Failed to serialize compose YAML: {}", e))?;

    Ok(result)
}

/// Get the container ID from `docker ps`.
async fn get_container_id(container_name: &str) -> Result<String> {
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
        if id.is_empty() {
            Err(anyhow!(
                "Container '{}' not found after starting",
                container_name
            ))
        } else {
            Ok(id)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(anyhow!("Failed to get container ID: {}", stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_container_name() {
        let yaml = r#"
services:
  vllm:
    image: vllm/vllm-openai:latest
    ports:
      - "8000:8000"
"#;
        let result = inject_container_name(yaml, "tama_test").unwrap();
        assert!(result.contains("container_name: tama_test"));
        assert!(result.contains("network_mode: host"));
    }

    #[test]
    fn test_inject_container_name_overwrites_existing() {
        let yaml = r#"
services:
  vllm:
    image: vllm/vllm-openai:latest
    container_name: old_name
    network_mode: bridge
"#;
        let result = inject_container_name(yaml, "tama_test").unwrap();
        assert!(result.contains("container_name: tama_test"));
        assert!(result.contains("network_mode: host"));
        assert!(!result.contains("old_name"));
        assert!(!result.contains("bridge"));
    }
}
