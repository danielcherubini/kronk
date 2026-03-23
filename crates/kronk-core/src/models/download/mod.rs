mod parallel;
mod single;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use std::time::Duration;

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
pub async fn download_chunked(
    url: &str,
    dest: &Path,
    connections: usize,
    auth_header: Option<&str>,
) -> Result<u64> {
    let client = build_client()?;

    // Apply auth header if provided, otherwise use hf-hub's default token
    let client = if let Some(header) = auth_header {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(header)
                .context("Invalid authorization header value")?,
        );
        Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(30))
            .default_headers(headers)
            .build()
            .context("Failed to create HTTP client with auth header")?
    } else {
        client
    };

    // HEAD request to get Content-Length and check Range support
    let head = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD request failed for {}", url))?;

    if !head.status().is_success() {
        anyhow::bail!("HEAD request returned HTTP {}: {}", head.status(), url);
    }

    let total_size = head
        .content_length()
        .context("Server did not return Content-Length")?;

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
        .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
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
    let template = "{spinner:.green} [{elapsed_precise}] \
                    [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})";
    pb.set_style(
        ProgressStyle::default_bar()
            .template(template)
            .context("Invalid progress bar template")?
            .progress_chars("=>-"),
    );

    // Note: auth_header is already set as a default header on the client,
    // so we pass None to avoid duplicate Authorization headers.
    let result = if num_connections == 1 {
        single::download_single(&client, url, dest, None, &pb).await
    } else {
        parallel::download_parallel(&client, url, dest, total_size, num_connections, None, &pb)
            .await
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
        let size = download_chunked(url, &dest, 1, None).await.unwrap();

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
        let size = download_chunked(url, &dest, 4, None).await.unwrap();

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
        let size1 = download_chunked(url, &dest, 1, None).await.unwrap();
        // Download again — should skip
        let size2 = download_chunked(url, &dest, 1, None).await.unwrap();

        assert_eq!(size1, size2);
    }
}
