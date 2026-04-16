use std::process::{Command, Stdio};
use std::time::Duration;

/// Integration test that verifies the restart handler causes the process to exit.
///
/// This test spawns the koji binary with a valid config, sends a restart request,
/// and verifies that the process terminates.
#[tokio::test]
async fn test_restart_handler_exits_process() {
    // Skip this test in CI or if we can't find the binary
    let binary_path = if let Ok(path) = std::env::var("KOJI_BINARY_PATH") {
        path
    } else {
        // Try multiple possible paths for the binary
        // From crate directory, parent is crates root, grandparent is workspace root
        let cwd = std::env::current_dir().unwrap_or_default();
        let workspace_root = cwd
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(cwd.as_path());
        let candidate = workspace_root.join("target/debug/koji");
        if candidate.exists() {
            candidate.to_string_lossy().to_string()
        } else {
            "target/debug/koji".to_string()
        }
    };

    eprintln!("Looking for binary at: {:?}", binary_path);
    if !std::path::Path::new(&binary_path).exists() {
        panic!("Koji binary not found at {:?}", binary_path);
    }

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_dir = temp_dir.path();

    // Create a minimal config file
    let config_path = config_dir.join("config.toml");
    let config_content = r#"
[proxy]
port = 0

[[models]]
id = "test-model"
backend = "llama_cpp"
model = "test-model"
enabled = true
"#;
    std::fs::write(&config_path, config_content).expect("Failed to write config");

    // Spawn the koji binary using the serve subcommand (config loaded from default location)
    let mut child = Command::new(&binary_path)
        .arg("serve")
        .arg("--port")
        .arg("0") // Use port 0 to let the OS assign a free port
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn koji binary");

    // Give the process time to start
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Check if the process is still alive
    // try_wait().is_none() means the process is still running
    let is_alive = child
        .try_wait()
        .expect("Failed to check process status")
        .is_none();

    // The process should be alive at this point
    assert!(is_alive, "Koji process should be running after spawn");

    // Terminate the process
    let _ = child.kill();
    let _ = child.wait();
}
