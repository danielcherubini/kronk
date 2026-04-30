/// Docker container log streaming.
///
/// Streams container logs by running `docker logs -f <container_name>` and
/// returns a (Receiver, JoinHandle) tuple for SSE consumers.
///
/// Note: Docker log streaming does NOT use the existing `BackendLogManager`
/// broadcast system. It uses a separate SSE endpoint that directly streams
/// `docker logs -f` output.
use anyhow::{anyhow, Context, Result};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;

/// Stream container logs by running `docker logs -f <container_name>`.
///
/// Returns a `(Receiver, JoinHandle)` tuple:
/// - Receiver: for the SSE handler to read log lines from
/// - JoinHandle: the spawned task running `docker logs -f`
///
/// The caller is responsible for dropping the receiver to close the channel,
/// which will cause the spawned task to exit.
pub async fn stream_logs(
    container_name: &str,
) -> Result<(mpsc::Receiver<String>, tokio::task::JoinHandle<Result<()>>)> {
    let (tx, rx) = mpsc::channel::<String>(1024);

    let container_name = container_name.to_string();
    let handle = tokio::spawn(async move {
        let mut child = Command::new("docker")
            .args(["logs", "-f", &container_name])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn 'docker logs -f {}'", container_name))?;

        // Stream stdout
        if let Some(stdout) = child.stdout.take() {
            let tx = tx.clone();
            tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(line).await;
                }
            });
        }

        // Stream stderr
        if let Some(stderr) = child.stderr.take() {
            let tx = tx.clone();
            tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = tx.send(line).await;
                }
            });
        }

        // Wait for the process to exit
        let status = child
            .wait()
            .await
            .with_context(|| format!("Failed to wait on 'docker logs -f {}'", container_name))?;

        if !status.success() {
            return Err(anyhow!(
                "'docker logs -f {}' exited with status: {}",
                container_name,
                status
            ));
        }

        Ok(())
    });

    Ok((rx, handle))
}
