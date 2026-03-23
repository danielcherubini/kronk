use anyhow::Context;
use futures_util::TryStreamExt;
use indicatif::ProgressBar;
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use super::MAX_RETRIES;

/// Download a file using parallel HTTP Range requests.
pub async fn download_parallel(
    client: &Client,
    url: &str,
    dest: &Path,
    total_size: u64,
    num_connections: usize,
    auth_header: Option<&str>,
    pb: &ProgressBar,
) -> anyhow::Result<()> {
    let chunk_size = total_size / num_connections as u64;

    // Build temp file paths
    let tmp_dir = dest.parent().unwrap_or(Path::new("."));
    let dest_filename = dest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("Destination path has no file name: {:?}", dest))?
        .to_string_lossy();
    let tmp_paths: Vec<PathBuf> = (0..num_connections)
        .map(|i| tmp_dir.join(format!(".{}.part{}", dest_filename, i)))
        .collect();

    // Download each chunk to a temp file
    let mut handles = Vec::new();

    for (i, tmp_path) in tmp_paths.iter().enumerate().take(num_connections) {
        let start = i as u64 * chunk_size;
        let end = if i == num_connections - 1 {
            total_size - 1
        } else {
            (i as u64 + 1) * chunk_size - 1
        };

        let client = client.clone();
        let url = url.to_string();
        let tmp_path = tmp_path.clone();
        let pb = pb.clone();
        let auth_header = auth_header.map(|h| h.to_string());

        let handle = tokio::spawn(async move {
            download_chunk_with_retry(
                &client,
                &url,
                &tmp_path,
                start,
                end,
                i,
                &pb,
                auth_header.as_deref(),
            )
            .await?;
            Ok::<PathBuf, anyhow::Error>(tmp_path)
        });

        handles.push(handle);
    }

    // Wait for all chunks — clean up on any failure
    let mut chunk_paths = Vec::new();
    let mut first_error: Option<anyhow::Error> = None;

    for handle in handles {
        match handle.await {
            Ok(Ok(path)) => chunk_paths.push(path),
            Ok(Err(e)) => {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(e.into());
                }
            }
        }
    }

    // If any chunk failed, clean up all temp files and bail
    if let Some(err) = first_error {
        cleanup_temp_files(&tmp_paths).await;
        return Err(err);
    }

    // Reassemble chunks into final file by streaming (not loading into memory)
    let mut dest_file = tokio::fs::File::create(dest).await?;
    for chunk_path in &chunk_paths {
        let mut chunk_file = tokio::fs::File::open(chunk_path).await?;
        tokio::io::copy(&mut chunk_file, &mut dest_file).await?;
        tokio::fs::remove_file(chunk_path).await.ok();
    }
    dest_file.flush().await?;

    Ok(())
}

/// Download a single chunk with retry and exponential backoff.
#[allow(clippy::too_many_arguments)]
async fn download_chunk_with_retry(
    client: &Client,
    url: &str,
    tmp_path: &Path,
    start: u64,
    end: u64,
    chunk_index: usize,
    pb: &ProgressBar,
    auth_header: Option<&str>,
) -> anyhow::Result<()> {
    let expected_size = end - start + 1;
    let mut attempt = 0u32;

    loop {
        attempt += 1;

        let range = format!("bytes={}-{}", start, end);
        let mut request = client.get(url).header("Range", &range);
        if let Some(header) = auth_header {
            request = request.header("Authorization", header);
        }
        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) if attempt <= MAX_RETRIES => {
                pb.suspend(|| {
                    println!(
                        "  Chunk {} failed (attempt {}/{}), retrying... ({})",
                        chunk_index, attempt, MAX_RETRIES, e
                    );
                });
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                continue;
            }
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Range request failed for chunk {}", chunk_index));
            }
        };

        // Validate we got 206 Partial Content
        let status = resp.status().as_u16();
        if status != 206 {
            if attempt <= MAX_RETRIES {
                pb.suspend(|| {
                    println!(
                        "  Chunk {} got status {} (expected 206), retrying ({}/{})...",
                        chunk_index, status, attempt, MAX_RETRIES
                    );
                });
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                continue;
            }
            anyhow::bail!(
                "Chunk {} got status {} instead of 206 Partial Content",
                chunk_index,
                status
            );
        }

        let mut stream = resp.bytes_stream();
        let mut file = tokio::fs::File::create(tmp_path).await?;
        let mut chunk_downloaded: u64 = 0;
        let mut stream_failed = false;

        loop {
            match stream.try_next().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk).await?;
                    chunk_downloaded += chunk.len() as u64;
                    pb.inc(chunk.len() as u64);
                }
                Ok(None) => break,
                Err(_e) => {
                    stream_failed = true;
                    break;
                }
            }
        }

        file.flush().await?;

        if stream_failed {
            tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
            continue;
        }

        // Verify chunk size
        if chunk_downloaded != expected_size {
            if attempt <= MAX_RETRIES {
                pb.suspend(|| {
                    println!(
                        "  Chunk {} short read ({}/{} bytes), retrying ({}/{})...",
                        chunk_index, chunk_downloaded, expected_size, attempt, MAX_RETRIES
                    );
                });
                pb.dec(chunk_downloaded);
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt - 1))).await;
                continue;
            }
            anyhow::bail!(
                "Chunk {} incomplete: got {} of {} bytes",
                chunk_index,
                chunk_downloaded,
                expected_size
            );
        }

        break;
    }

    Ok(())
}

/// Best-effort cleanup of temp chunk files.
async fn cleanup_temp_files(paths: &[PathBuf]) {
    for path in paths {
        tokio::fs::remove_file(path).await.ok();
    }
}
