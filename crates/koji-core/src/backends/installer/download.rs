use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::backends::ProgressSink;
use crate::models::download::parse_content_length;

/// Maximum number of download retries.
const MAX_RETRIES: u32 = 3;
/// Base backoff delay for retry attempts (1s, 2s, 4s).
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Download a file from a URL to a destination path with retry logic.
///
/// Retries up to `MAX_RETRIES` times on network errors and 5xx responses,
/// with exponential backoff (1s, 2s, 4s).
///
/// When `progress.is_some()`, skips `indicatif` and emits throttled progress lines
/// via the sink (format: "downloaded {hsz_done} / {hsz_total} ({pct}%)").
///
/// When `progress.is_none()`, preserves the existing `indicatif` TTY bar behavior.
///
/// After the stream completes, verifies downloaded bytes match Content-Length when known.
#[allow(dead_code)]
pub async fn download_file(
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    download_with_client(url, dest, progress, None).await
}

/// Download a file using a shared reqwest::Client with retry logic.
pub async fn download_with_client(
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
    client: Option<&Client>,
) -> Result<()> {
    let client = match client {
        Some(c) => c,
        None => &Client::builder()
            .user_agent("koji-backend-manager")
            .timeout(Duration::from_secs(300))
            .connect_timeout(Duration::from_secs(30))
            .build()?,
    };

    let mut last_error = None;
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let multiplier: u32 = 1 << (attempt - 1); // 1, 2, 4
            let backoff = BASE_BACKOFF * multiplier;
            tracing::info!(
                attempt,
                max_retries = MAX_RETRIES,
                url,
                "Retrying download after {:?} backoff",
                backoff
            );
            tokio::time::sleep(backoff).await;
        }

        match perform_download(client, url, dest, progress).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Check if this is a retryable error (network error or 5xx)
                let is_retryable = is_retryable_error(&e);
                if !is_retryable || attempt == MAX_RETRIES {
                    tracing::warn!(attempt, url, error = %e, "Download failed after {} attempts", attempt + 1);
                    return Err(e);
                }
                last_error = Some(e);
            }
        }
    }

    // Should not reach here, but just in case
    Err(last_error.unwrap_or_else(|| anyhow!("Download failed after {} retries", MAX_RETRIES)))
}

/// Check if an error is retryable (network errors and 5xx responses).
fn is_retryable_error(e: &anyhow::Error) -> bool {
    let msg = e.to_string().to_lowercase();
    // Network-level errors
    if msg.contains("connection")
        || msg.contains("timeout")
        || msg.contains("dns")
        || msg.contains("refused")
        || msg.contains("reset")
        || msg.contains("tls")
        || msg.contains("certificate")
        || msg.contains("closed")
    {
        return true;
    }
    // HTTP 5xx errors
    if msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
        || msg.contains("501")
        || msg.contains("status: 5")
    {
        return true;
    }
    false
}

/// Perform a single download attempt.
pub async fn perform_download(
    client: &Client,
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download from {}", url))?;

    let status = response.status();
    if !status.is_success() {
        // For 5xx responses, return a retryable error
        if status.as_u16() >= 500 {
            return Err(anyhow!(
                "Download failed with server error {}: {}",
                status,
                status.canonical_reason().unwrap_or("unknown")
            ));
        }
        return Err(anyhow!("Download failed with status: {}", status));
    }

    let total_size = parse_content_length(response.headers()).unwrap_or(0);

    if let Some(sink) = progress {
        // Web path: no indicatif, emit throttled progress lines
        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();
        let mut last_emit = tokio::time::Instant::now();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // Throttle emissions to ~1 per 250ms
            if downloaded % (1024 * 1024) < chunk.len() as u64
                || last_emit.elapsed() >= Duration::from_millis(250)
            {
                let pct = if total_size > 0 {
                    (downloaded as f64 / total_size as f64) * 100.0
                } else {
                    0.0
                };
                let done_mb = downloaded as f64 / 1_048_576.0;
                let total_mb = total_size as f64 / 1_048_576.0;
                let msg = format!(
                    "downloaded {:.1} MiB / {:.1} MiB ({:.0}%)",
                    done_mb, total_mb, pct
                );
                sink.log(&msg);
                last_emit = tokio::time::Instant::now();
            }
        }

        // Verify downloaded bytes match Content-Length when known
        if total_size > 0 && downloaded != total_size {
            return Err(anyhow!(
                "Download incomplete: expected {} bytes but got {}",
                total_size,
                downloaded
            ));
        }

        // Flush to ensure all data is written to disk before returning
        file.flush().await?;
        Ok(())
    } else {
        // CLI path: preserve indicatif TTY bar
        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        // Verify downloaded bytes match Content-Length when known
        if total_size > 0 && downloaded != total_size {
            return Err(anyhow!(
                "Download incomplete: expected {} bytes but got {}",
                total_size,
                downloaded
            ));
        }

        // Flush to ensure all data is written to disk before returning
        file.flush().await?;

        pb.finish_and_clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    /// Simple HTTP server that counts requests and returns a specific status code.
    struct CountingServer {
        addr: std::net::SocketAddr,
        _request_count: Arc<AtomicU32>,
        _success_after: u32,
    }

    impl CountingServer {
        async fn new(success_after: u32) -> Result<Self> {
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let addr = listener.local_addr()?;
            let request_count = Arc::new(AtomicU32::new(0));
            let rc_clone = Arc::clone(&request_count);

            tokio::spawn(async move {
                loop {
                    let (mut stream, _) = match listener.accept().await {
                        Ok(s) => s,
                        Err(_) => break,
                    };

                    // Read the HTTP request (headers only)
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let _ = n;

                    let count = rc_clone.fetch_add(1, Ordering::SeqCst) + 1;

                    // Build response
                    let (status_code, status_text) = if count < success_after {
                        (503, "Service Unavailable")
                    } else {
                        (200, "OK")
                    };

                    let body = b"test content";
                    let response = format!(
                        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        status_code,
                        status_text,
                        body.len()
                    );

                    let _ = stream.write_all(response.as_bytes()).await;
                    if status_code == 200 {
                        let _ = stream.write_all(body).await;
                    }
                }
            });

            Ok(Self {
                addr,
                _request_count: request_count,
                _success_after: success_after,
            })
        }

        fn url(&self) -> String {
            format!("http://{}/test", self.addr)
        }
    }

    #[tokio::test]
    async fn test_download_retries_on_503_then_succeeds() {
        let server = CountingServer::new(2).await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("test.bin");

        // Should retry on 503 and succeed on the 2nd attempt
        download_file(&server.url(), &dest, None).await.unwrap();

        assert!(dest.exists());
        let content = tokio::fs::read(&dest).await.unwrap();
        assert_eq!(content, b"test content");
    }

    #[tokio::test]
    async fn test_download_fails_after_max_retries() {
        // Always return 503 — should fail after MAX_RETRIES + 1 attempts
        let server = CountingServer::new(u32::MAX).await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("fail.bin");

        let result = download_file(&server.url(), &dest, None).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("503") || err_msg.contains("retries"),
            "Expected retry error, got: {}",
            err_msg
        );
    }

    #[tokio::test]
    async fn test_download_succeeds_on_first_attempt() {
        // Return 200 immediately
        let server = CountingServer::new(1).await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("fast.bin");

        download_file(&server.url(), &dest, None).await.unwrap();

        assert!(dest.exists());
    }

    #[test]
    fn test_is_retryable_error_network() {
        let err = anyhow::anyhow!("connection refused");
        assert!(is_retryable_error(&err));

        let err = anyhow::anyhow!("timeout exceeded");
        assert!(is_retryable_error(&err));

        let err = anyhow::anyhow!("dns resolution failed");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_5xx() {
        let err = anyhow::anyhow!("Download failed with status: 500 Internal Server Error");
        assert!(is_retryable_error(&err));

        let err = anyhow::anyhow!("Download failed with server error 503: Service Unavailable");
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_not_retryable() {
        let err = anyhow::anyhow!("Download failed with status: 404 Not Found");
        assert!(!is_retryable_error(&err));

        let err = anyhow::anyhow!("Download failed with status: 401 Unauthorized");
        assert!(!is_retryable_error(&err));

        let err = anyhow::anyhow!("file not found");
        assert!(!is_retryable_error(&err));
    }
}
