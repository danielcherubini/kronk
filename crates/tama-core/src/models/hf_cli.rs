//! HuggingFace CLI download runner.
//!
//! Uses the `hf` command-line tool to download model files, replacing the
//! flaky `hf-hub` Rust crate with a more reliable subprocess approach.
//! Progress is reported by polling the destination directory file sizes
//! and parsing `hf download` stdout for per-file completion events.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;
use tokio::sync::watch;

/// Callback type for reporting download progress.
/// Called with (bytes_downloaded, total_bytes, current_file_description).
pub type HfProgressCallback = Arc<dyn Fn(u64, u64, &str) + Send + Sync>;

/// Result of a successful HF CLI download.
pub struct HfDownloadResult {
    /// Local directory where files were downloaded
    pub local_dir: PathBuf,
    /// Total bytes downloaded (sum of all files)
    pub total_bytes: u64,
}

/// Download specific files from a HuggingFace GGUF repository.
///
/// Uses `hf download <repo_id> <filename> --local-dir <dest>` to download
/// a single GGUF quant file. Progress is reported via the callback.
pub async fn hf_download_gguf(
    repo_id: &str,
    filename: &str,
    local_dir: &Path,
    progress_cb: Option<HfProgressCallback>,
) -> Result<HfDownloadResult> {
    let args = vec![
        "download".to_string(),
        repo_id.to_string(),
        filename.to_string(),
        "--local-dir".to_string(),
        local_dir.to_string_lossy().to_string(),
        "--format".to_string(),
        "json".to_string(),
    ];
    run_hf_download(&args, local_dir, Some(filename), progress_cb).await
}

/// Download an entire HuggingFace repository (for non-GGUF models).
///
/// Uses `hf download <repo_id> --local-dir <dest>` to download all files.
/// Progress is reported via the callback.
pub async fn hf_download_repo(
    repo_id: &str,
    local_dir: &Path,
    progress_cb: Option<HfProgressCallback>,
) -> Result<HfDownloadResult> {
    let args = vec![
        "download".to_string(),
        repo_id.to_string(),
        "--local-dir".to_string(),
        local_dir.to_string_lossy().to_string(),
        "--format".to_string(),
        "json".to_string(),
    ];
    run_hf_download(&args, local_dir, None, progress_cb).await
}

/// Core runner that executes `hf` CLI with the given arguments and monitors
/// progress by polling the destination directory.
async fn run_hf_download(
    args: &[String],
    local_dir: &Path,
    single_file: Option<&str>,
    progress_cb: Option<HfProgressCallback>,
) -> Result<HfDownloadResult> {
    // Ensure destination directory exists
    std::fs::create_dir_all(local_dir)
        .with_context(|| format!("Failed to create directory: {}", local_dir.display()))?;

    // Clean up any stale HF cache lock files in the destination
    let cache_dir = local_dir
        .join(".cache")
        .join("huggingface")
        .join("download");
    if cache_dir.exists() {
        let _ = clean_stale_locks(&cache_dir).await;
    }

    tracing::info!(args = ?args, "Starting hf download");

    let mut child = Command::new("hf")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn `hf` command. Is huggingface_hub installed?")?;

    let pid = child.id().unwrap_or(0);
    tracing::info!(pid, "hf download process started");

    // Read stdout line-by-line for completion events
    let stdout = child.stdout.take().context("Failed to capture stdout")?;
    let stderr = child.stderr.take().context("Failed to capture stderr")?;

    // Channel to signal the progress poller to stop
    let (done_tx, done_rx) = watch::channel(false);

    // Spawn progress poller
    let poll_local_dir = local_dir.to_path_buf();
    let poll_single_file = single_file.map(|s| s.to_string());
    let poll_cb = progress_cb.clone();
    let poll_handle = tokio::spawn(async move {
        poll_progress(
            &poll_local_dir,
            poll_single_file.as_deref(),
            &poll_cb,
            done_rx,
        )
        .await;
    });

    // Spawn stderr reader (just log tqdm output)
    let stderr_handle = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    // Strip ANSI escape codes and carriage returns for clean logging
                    let cleaned = strip_ansi_and_cr(&line);
                    if !cleaned.is_empty() {
                        tracing::debug!(stderr = %cleaned, "hf download progress");
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Read stdout for completion events
    let stdout_handle = tokio::spawn(async move {
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut lines = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let cleaned = line.trim().to_string();
                    if !cleaned.is_empty() {
                        tracing::info!(stdout = %cleaned, "hf download file completed");
                        lines.push(cleaned);
                    }
                }
                Err(_) => break,
            }
        }
        lines
    });

    // Wait for the process to finish
    let status = child.wait().await.context("hf download process failed")?;

    // Signal the progress poller to stop
    let _ = done_tx.send(true);
    poll_handle.await.ok();
    stderr_handle.await.ok();
    let _stdout_lines = stdout_handle.await.unwrap_or_default();

    if !status.success() {
        return Err(anyhow!("hf download exited with code {:?}", status.code()));
    }

    // Calculate total bytes downloaded
    let total_bytes = calculate_dir_size(local_dir).await;

    tracing::info!(
        dir = %local_dir.display(),
        bytes = total_bytes,
        "hf download complete"
    );

    Ok(HfDownloadResult {
        local_dir: local_dir.to_path_buf(),
        total_bytes,
    })
}

/// Poll the destination directory for progress updates.
///
/// Every 500ms, sums up file sizes in the directory and reports progress
/// via the callback. Stops when `done_rx` receives true.
async fn poll_progress(
    local_dir: &Path,
    single_file: Option<&str>,
    progress_cb: &Option<HfProgressCallback>,
    mut done_rx: watch::Receiver<bool>,
) {
    let mut last_bytes: u64 = 0;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
            _ = done_rx.changed() => {
                if *done_rx.borrow() {
                    break;
                }
            }
        }

        if *done_rx.borrow() {
            break;
        }

        let bytes = if let Some(filename) = single_file {
            tokio::fs::metadata(local_dir.join(filename))
                .await
                .map(|m| m.len())
                .unwrap_or(last_bytes)
        } else {
            calculate_dir_size(local_dir).await
        };

        if bytes != last_bytes {
            last_bytes = bytes;
            if let Some(cb) = progress_cb {
                cb(bytes, 0, single_file.unwrap_or("all files"));
            }
        }
    }
}

/// Calculate the total size of all files in a directory (non-recursive on .cache).
async fn calculate_dir_size(dir: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return 0,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        // Skip .cache directory (HF internal)
        let name = entry.file_name();
        if name == ".cache" {
            continue;
        }
        if let Ok(meta) = entry.metadata().await {
            if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// Clean up stale `.lock` files in the HF download cache directory.
///
/// These can accumulate from previous interrupted downloads and cause the
/// `hf` CLI to hang waiting for locks.
async fn clean_stale_locks(cache_dir: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(cache_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "lock") {
            tracing::debug!(path = %path.display(), "Removing stale lock file");
            tokio::fs::remove_file(&path).await.ok();
        }
    }
    Ok(())
}

/// Strip ANSI escape codes and carriage returns from a string.
fn strip_ansi_and_cr(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {}
            '\x1b' => {
                // Skip ANSI escape sequence
                if chars.peek() == Some(&'[') {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        if next.is_ascii_alphabetic() {
                            chars.next();
                            break;
                        }
                        chars.next();
                    }
                }
            }
            _ => result.push(c),
        }
    }
    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_simple() {
        assert_eq!(
            strip_ansi_and_cr("\x1b[32m✓ Downloaded\x1b[0m"),
            "✓ Downloaded"
        );
    }

    #[test]
    fn test_strip_carriage_return() {
        assert_eq!(
            strip_ansi_and_cr("Fetching 2 files:  50%\r"),
            "Fetching 2 files:  50%"
        );
    }

    #[test]
    fn test_strip_ansi_progress_bar() {
        let input = "\rFetching 10 files:  10%\x1b[32m█\x1b[0m         | 1/10";
        let result = strip_ansi_and_cr(input);
        assert!(!result.contains('\r'));
        assert!(!result.contains("\x1b"));
    }

    #[test]
    fn test_strip_ansi_empty() {
        assert_eq!(strip_ansi_and_cr(""), "");
    }

    #[test]
    fn test_strip_ansi_no_escapes() {
        assert_eq!(strip_ansi_and_cr("hello world"), "hello world");
    }

    #[tokio::test]
    async fn test_calculate_dir_size() {
        let tmp = tempfile::tempdir().unwrap();
        tokio::fs::write(tmp.path().join("a.bin"), b"12345")
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join("b.bin"), b"67890")
            .await
            .unwrap();
        // .cache should be skipped
        tokio::fs::create_dir_all(tmp.path().join(".cache"))
            .await
            .unwrap();
        tokio::fs::write(tmp.path().join(".cache").join("c.bin"), b"xxx")
            .await
            .unwrap();

        let size = calculate_dir_size(tmp.path()).await;
        assert_eq!(size, 10); // 5 + 5, not 13
    }

    #[tokio::test]
    async fn test_clean_stale_locks() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().join("download");
        tokio::fs::create_dir_all(&cache_dir).await.unwrap();
        tokio::fs::write(cache_dir.join("model.safetensors.lock"), b"")
            .await
            .unwrap();
        tokio::fs::write(cache_dir.join("model.safetensors.part"), b"data")
            .await
            .unwrap();

        clean_stale_locks(&cache_dir).await.unwrap();

        assert!(!cache_dir.join("model.safetensors.lock").exists());
        assert!(cache_dir.join("model.safetensors.part").exists());
    }
}
