mod parallel;
mod single;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::header::HeaderMap;
use reqwest::Client;

const MIN_CHUNK_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB
const MAX_RETRIES: u32 = 3;

/// Callback type for reporting download progress.
/// Called with (bytes_downloaded, total_bytes).
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// Parse the Content-Length header from raw headers, bypassing reqwest's
/// Response::content_length() which returns Some(0) for HEAD requests (known bug).
pub fn parse_content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// Download a file using parallel HTTP Range requests.
/// Falls back to single-stream if Range is not supported.
/// Skips download if the destination already exists with matching size.
pub async fn download_chunked(
    client: &Client,
    url: &str,
    dest: &Path,
    connections: usize,
) -> Result<u64> {
    download_chunked_with_progress(client, url, dest, connections, None).await
}

/// Download a file using parallel HTTP Range requests with progress callback.
/// Falls back to single-stream if Range is not supported.
/// Skips download if the destination already exists with matching size.
///
/// The progress callback is called periodically with (bytes_downloaded, total_bytes).
/// This is useful for reporting progress to external consumers (e.g., SSE streams).
pub async fn download_chunked_with_progress(
    client: &Client,
    url: &str,
    dest: &Path,
    connections: usize,
    progress_callback: Option<ProgressCallback>,
) -> Result<u64> {
    // HEAD request to get Content-Length and check Range support
    let head = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD request failed for {}", url))?;

    if !head.status().is_success() {
        anyhow::bail!("HEAD request returned HTTP {}: {}", head.status(), url);
    }

    let total_size = parse_content_length(head.headers())
        .context("Server did not return a valid Content-Length")?;

    if total_size == 0 {
        anyhow::bail!("Server reported Content-Length of 0 for {}", url);
    }

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

    // Wrap the callback to also update the progress bar
    let callback_for_bar = if let Some(cb) = progress_callback.clone() {
        let pb_clone = pb.clone();
        Some(Arc::new(move |downloaded: u64, total: u64| {
            pb_clone.set_position(downloaded);
            cb(downloaded, total);
        }) as ProgressCallback)
    } else {
        None
    };

    let result = if num_connections == 1 {
        single::download_single(client, url, dest, total_size, &pb, callback_for_bar.as_ref()).await
    } else {
        parallel::download_parallel(
            client,
            url,
            dest,
            total_size,
            num_connections,
            &pb,
            callback_for_bar.as_ref(),
        )
        .await
    };

    pb.finish_and_clear();
    result?;
    Ok(total_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_length_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "12345".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(12345));
    }

    #[test]
    fn test_parse_content_length_missing() {
        let headers = HeaderMap::new();
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_non_numeric() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "abc".parse().unwrap());
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_zero() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "0".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(0));
    }

    #[tokio::test]
    #[ignore] // Requires network access
    async fn test_download_single_small_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("test.txt");

        // Use a small known file from HuggingFace (a config.json)
        let url = "https://huggingface.co/gpt2/resolve/main/config.json";
        let client = Client::new();
        let size = download_chunked(&client, url, &dest, 1).await.unwrap();

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
        let client = Client::new();
        let size = download_chunked(&client, url, &dest, 4).await.unwrap();

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
        let client = Client::new();

        // Download once
        let size1 = download_chunked(&client, url, &dest, 1).await.unwrap();
        // Download again — should skip
        let size2 = download_chunked(&client, url, &dest, 1).await.unwrap();

        assert_eq!(size1, size2);
    }
}
