use anyhow::Context;
use futures_util::TryStreamExt;
use indicatif::ProgressBar;
use reqwest::Client;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use super::MAX_RETRIES;

/// Download a file using a single HTTP stream with retry support.
pub async fn download_single(
    client: &Client,
    url: &str,
    dest: &Path,
    auth_header: Option<&str>,
    pb: &ProgressBar,
) -> anyhow::Result<()> {
    let mut attempt = 0u32;
    let mut downloaded: u64 = 0;

    loop {
        attempt += 1;

        let mut request = client.get(url);
        if let Some(header) = auth_header {
            request = request.header("Authorization", header);
        }
        if downloaded > 0 {
            request = request.header("Range", format!("bytes={}-", downloaded));
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) if attempt <= MAX_RETRIES => {
                pb.suspend(|| {
                    println!(
                        "  Download failed (attempt {}/{}), retrying... ({})",
                        attempt, MAX_RETRIES, e
                    );
                });
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        // Validate status code
        let status = resp.status().as_u16();
        if downloaded > 0 && status != 206 {
            // Only bail immediately for permanent mismatch (un-ranged 200)
            if status == 200 {
                anyhow::bail!(
                    "Expected 206 Partial Content for resumed download, got {}",
                    status
                );
            }
            // Retry transient errors (429/5xx)
            if attempt <= MAX_RETRIES {
                pb.suspend(|| {
                    println!(
                        "  Server returned {}, retrying ({}/{})...",
                        status, attempt, MAX_RETRIES
                    );
                });
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                continue;
            }
            anyhow::bail!("Download failed with status {}", status);
        }
        if downloaded == 0 && !resp.status().is_success() {
            if attempt <= MAX_RETRIES {
                pb.suspend(|| {
                    println!(
                        "  Server returned {}, retrying ({}/{})...",
                        status, attempt, MAX_RETRIES
                    );
                });
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                continue;
            }
            anyhow::bail!("Download failed with status {}", status);
        }

        let mut file = if downloaded > 0 {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(dest)
                .await
                .with_context(|| format!("Failed to open {} for append", dest.display()))?
        } else {
            tokio::fs::File::create(dest)
                .await
                .with_context(|| format!("Failed to create {}", dest.display()))?
        };

        let mut stream = resp.bytes_stream();
        let mut stream_failed = false;

        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk)
                        .await
                        .with_context(|| format!("Failed to write to {}", dest.display()))?;
                    downloaded += chunk.len() as u64;
                    pb.set_position(downloaded);
                }
                Ok(None) => break,
                Err(_e) => {
                    if attempt <= MAX_RETRIES {
                        pb.suspend(|| {
                            println!(
                                "  Stream interrupted at {:.1} MiB (attempt {}/{}), retrying... ({})",
                                downloaded as f64 / 1_048_576.0,
                                attempt,
                                MAX_RETRIES,
                                _e
                            );
                        });
                        // Keep progress bar at current position for retry
                        pb.set_position(downloaded);
                        stream_failed = true;
                        break;
                    }
                    return Err(_e.into());
                }
            }
        }

        file.flush().await?;

        if stream_failed {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
            continue;
        }

        // Download complete
        break;
    }

    Ok(())
}
