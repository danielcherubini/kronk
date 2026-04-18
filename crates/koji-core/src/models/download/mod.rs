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

/// Calculate the number of connections to use for parallel download,
/// based on total file size and minimum chunk size.
pub fn calculate_connections(total_size: u64, max_connections: usize) -> usize {
    if total_size <= MIN_CHUNK_SIZE {
        return 1;
    }
    let suggested = (total_size / MIN_CHUNK_SIZE) as usize;
    suggested.min(max_connections).max(1)
}

/// Calculate chunk ranges for parallel download.
/// Returns a vector of (start, end) byte ranges for each chunk.
pub fn calculate_chunk_ranges(total_size: u64, num_chunks: usize) -> Vec<(u64, u64)> {
    if num_chunks == 0 || total_size == 0 {
        return vec![];
    }
    let chunk_size = total_size / num_chunks as u64;
    (0..num_chunks)
        .map(|i| {
            let start = i as u64 * chunk_size;
            let end = if i == num_chunks - 1 {
                total_size.saturating_sub(1)
            } else {
                (i as u64 + 1) * chunk_size - 1
            };
            (start, end)
        })
        .collect()
}

/// Calculate the expected size of a single chunk given total size and number of chunks.
pub fn chunk_size_for(total_size: u64, num_chunks: usize) -> u64 {
    if num_chunks == 0 || total_size == 0 {
        return 0;
    }
    total_size / num_chunks as u64
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
        single::download_single(
            client,
            url,
            dest,
            total_size,
            &pb,
            callback_for_bar.as_ref(),
        )
        .await
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

    // ── parse_content_length tests ────────────────────────────────────────

    #[test]
    fn test_parse_content_length_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "12345".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(12345));
    }

    #[test]
    fn test_parse_content_length_large_value() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "999999999999".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(999999999999));
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

    #[test]
    fn test_parse_content_length_negative_string() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "-1".parse().unwrap());
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_with_whitespace() {
        let mut headers = HeaderMap::new();
        headers.insert("content-length", "  512  ".parse().unwrap());
        // to_str() preserves whitespace, parse::<u64>() fails on whitespace
        assert_eq!(parse_content_length(&headers), None);
    }

    #[test]
    fn test_parse_content_length_case_insensitive_header() {
        let mut headers = HeaderMap::new();
        // HTTP headers are case-insensitive
        headers.insert("Content-Length", "4096".parse().unwrap());
        assert_eq!(parse_content_length(&headers), Some(4096));
    }

    #[test]
    fn test_parse_content_length_multiple_values() {
        let mut headers = HeaderMap::new();
        headers.append("content-length", "100".parse().unwrap());
        headers.append("content-length", "200".parse().unwrap());
        // and_then takes the first value
        assert_eq!(parse_content_length(&headers), Some(100));
    }

    // ── calculate_connections tests ───────────────────────────────────────

    #[test]
    fn test_calculate_connections_small_file() {
        // File smaller than MIN_CHUNK_SIZE (5 MiB)
        assert_eq!(calculate_connections(1024, 4), 1);
        assert_eq!(calculate_connections(MIN_CHUNK_SIZE - 1, 8), 1);
    }

    #[test]
    fn test_calculate_connections_exact_chunk_size() {
        // File exactly MIN_CHUNK_SIZE should use 1 connection
        assert_eq!(calculate_connections(MIN_CHUNK_SIZE, 4), 1);
    }

    #[test]
    fn test_calculate_connections_multiple_chunks() {
        // 10 MiB file / 5 MiB chunk = 2 connections
        assert_eq!(calculate_connections(10 * 1024 * 1024, 4), 2);
        // 20 MiB file with max 2 connections
        assert_eq!(calculate_connections(20 * 1024 * 1024, 2), 2);
    }

    #[test]
    fn test_calculate_connections_capped_by_max() {
        // Large file but max connections limits it
        assert_eq!(calculate_connections(100 * 1024 * 1024, 3), 3);
    }

    #[test]
    fn test_calculate_connections_minimum_one() {
        // Even with max=1, should return at least 1 for large files
        assert_eq!(calculate_connections(100 * 1024 * 1024, 1), 1);
    }

    #[test]
    fn test_calculate_connections_zero_max() {
        // With max_connections=0, should still return at least 1
        assert_eq!(calculate_connections(100 * 1024 * 1024, 0), 1);
    }

    #[test]
    fn test_calculate_connections_zero_size() {
        // Zero-size file should return 1 (not 0)
        assert_eq!(calculate_connections(0, 8), 1);
    }

    // ── calculate_chunk_ranges tests ──────────────────────────────────────

    #[test]
    fn test_calculate_chunk_ranges_single_chunk() {
        let ranges = calculate_chunk_ranges(1000, 1);
        assert_eq!(ranges, vec![(0, 999)]);
    }

    #[test]
    fn test_calculate_chunk_ranges_even_split() {
        // 100 bytes split into 4 chunks = 25 bytes each
        let ranges = calculate_chunk_ranges(100, 4);
        assert_eq!(ranges, vec![(0, 24), (25, 49), (50, 74), (75, 99)]);
    }

    #[test]
    fn test_calculate_chunk_ranges_uneven_split() {
        // 100 bytes split into 3 chunks = 33 bytes each, last chunk gets remainder
        let ranges = calculate_chunk_ranges(100, 3);
        assert_eq!(ranges[0], (0, 32));
        assert_eq!(ranges[1], (33, 65));
        // Last chunk covers remaining bytes
        assert_eq!(ranges[2].0, 66);
        assert_eq!(ranges[2].1, 99);
    }

    #[test]
    fn test_calculate_chunk_ranges_zero_size() {
        let ranges = calculate_chunk_ranges(0, 4);
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_calculate_chunk_ranges_zero_chunks() {
        let ranges = calculate_chunk_ranges(1000, 0);
        assert!(ranges.is_empty());
    }

    #[test]
    fn test_calculate_chunk_ranges_covers_full_range() {
        // Verify that all ranges together cover [0, total_size - 1]
        let total_size = 1024;
        let num_chunks = 7;
        let ranges = calculate_chunk_ranges(total_size, num_chunks);

        assert_eq!(ranges.len(), num_chunks);
        assert_eq!(ranges[0].0, 0);
        assert_eq!(ranges[num_chunks - 1].1, total_size - 1);

        // Verify no gaps between consecutive ranges
        for i in 0..(num_chunks - 1) {
            assert_eq!(ranges[i + 1].0, ranges[i].1 + 1);
        }
    }

    #[test]
    fn test_calculate_chunk_ranges_two_chunks() {
        let ranges = calculate_chunk_ranges(10, 2);
        assert_eq!(ranges, vec![(0, 4), (5, 9)]);
    }

    // ── chunk_size_for tests ──────────────────────────────────────────────

    #[test]
    fn test_chunk_size_for_even_split() {
        assert_eq!(chunk_size_for(1000, 4), 250);
        assert_eq!(chunk_size_for(100, 10), 10);
    }

    #[test]
    fn test_chunk_size_for_uneven_split() {
        // 100 / 3 = 33 (integer division)
        assert_eq!(chunk_size_for(100, 3), 33);
    }

    #[test]
    fn test_chunk_size_for_single_chunk() {
        assert_eq!(chunk_size_for(1000, 1), 1000);
    }

    #[test]
    fn test_chunk_size_for_zero_values() {
        assert_eq!(chunk_size_for(0, 5), 0);
        assert_eq!(chunk_size_for(100, 0), 0);
    }
}
