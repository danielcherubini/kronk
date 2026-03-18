use anyhow::{Context, Result};
use futures_util::TryStreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const MIN_CHUNK_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB
const MAX_RETRIES: u32 = 3;

/// Build an HTTP client with sensible timeouts for large file downloads.
fn build_client() -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")
}

/// Download a file using parallel HTTP Range requests.
/// Falls back to single-stream if Range is not supported.
/// Skips download if the destination already exists with matching size.
pub async fn download_chunked(url: &str, dest: &Path, connections: usize) -> Result<u64> {
    let client = build_client()?;

    // HEAD request to get Content-Length and check Range support
    let head = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD request failed for {}", url))?;

    let total_size = head
        .content_length()
        .with_context(|| "Server did not return Content-Length")?;

    // Skip download if file already exists with matching size
    if dest.exists() {
        if let Ok(meta) = tokio::fs::metadata(dest).await {
            if meta.len() == total_size {
                return Ok(total_size);
            }
        }
    }

    let accept_ranges = head
        .headers()
        .get("accept-ranges")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none");

    let use_chunked = accept_ranges != "none" && total_size > MIN_CHUNK_SIZE;
    let num_connections = if use_chunked {
        connections
            .min((total_size / MIN_CHUNK_SIZE) as usize)
            .max(1)
    } else {
        1
    };

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
            .unwrap()
            .progress_chars("=>-"),
    );

    let result = if num_connections == 1 {
        download_single(&client, url, dest, &pb).await
    } else {
        download_parallel(&client, url, dest, total_size, num_connections, &pb).await
    };

    match result {
        Ok(()) => {
            pb.finish_with_message("done");
            Ok(total_size)
        }
        Err(e) => {
            pb.finish_and_clear();
            Err(e)
        }
    }
}

async fn download_single(client: &Client, url: &str, dest: &Path, pb: &ProgressBar) -> Result<()> {
    let mut attempt = 0u32;
    let mut downloaded: u64 = 0;

    loop {
        attempt += 1;

        let mut request = client.get(url);
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
            anyhow::bail!(
                "Expected 206 Partial Content for resumed download, got {}",
                status
            );
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
        let stream_failed = false;

        while let Ok(Some(chunk)) = stream.try_next().await {
            file.write_all(&chunk)
                .await
                .with_context(|| format!("Failed to write to {}", dest.display()))?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
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

async fn download_parallel(
    client: &Client,
    url: &str,
    dest: &Path,
    total_size: u64,
    num_connections: usize,
    pb: &ProgressBar,
) -> Result<()> {
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

        let handle = tokio::spawn(async move {
            download_chunk_with_retry(&client, &url, &tmp_path, start, end, i, &pb).await?;
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
async fn download_chunk_with_retry(
    client: &Client,
    url: &str,
    tmp_path: &Path,
    start: u64,
    end: u64,
    chunk_index: usize,
    pb: &ProgressBar,
) -> Result<()> {
    let expected_size = end - start + 1;
    let mut attempt = 0u32;

    loop {
        attempt += 1;

        let range = format!("bytes={}-{}", start, end);
        let resp = match client.get(url).header("Range", &range).send().await {
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
        let stream_failed = false;

        while let Ok(Some(chunk)) = stream.try_next().await {
            file.write_all(&chunk).await?;
            chunk_downloaded += chunk.len() as u64;
            pb.inc(chunk.len() as u64);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_single_small_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("test.txt");

        // Use a small known file from HuggingFace (a config.json)
        let url = "https://huggingface.co/gpt2/resolve/main/config.json";
        let size = download_chunked(url, &dest, 1).await.unwrap();

        assert!(dest.exists());
        assert!(size > 0);
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), size);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_parallel_chunks() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("test.bin");

        // Use a larger file to test parallel downloads
        let url = "https://huggingface.co/gpt2/resolve/main/merges.txt";
        let size = download_chunked(url, &dest, 4).await.unwrap();

        assert!(dest.exists());
        assert!(size > 0);
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), size);
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_skip_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("test.txt");

        let url = "https://huggingface.co/gpt2/resolve/main/config.json";

        // Download once
        let size1 = download_chunked(url, &dest, 1).await.unwrap();
        // Download again — should skip
        let size2 = download_chunked(url, &dest, 1).await.unwrap();

        assert_eq!(size1, size2);
    }
}
