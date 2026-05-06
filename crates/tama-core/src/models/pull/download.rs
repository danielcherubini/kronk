use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest;

use crate::models::download::ProgressCallback;

/// Result of downloading a GGUF file.
pub struct DownloadResult {
    /// Local path to the file (in the model directory)
    pub path: PathBuf,
    /// File size in bytes (from the hf-hub cache, always accurate)
    pub size_bytes: u64,
}

/// Progress adapter that bridges hf-hub's Progress trait to our callback.
#[derive(Clone)]
pub struct ProgressAdapter {
    total_size: u64,
    downloaded: Arc<AtomicU64>,
    callback: Option<ProgressCallback>,
}

impl ProgressAdapter {
    pub fn new(callback: Option<ProgressCallback>) -> Self {
        Self {
            total_size: 0,
            downloaded: Arc::new(AtomicU64::new(0)),
            callback,
        }
    }
}

impl hf_hub::api::tokio::Progress for ProgressAdapter {
    async fn init(&mut self, size: usize, _filename: &str) {
        self.total_size = size as u64;
        self.downloaded.store(0, Ordering::Relaxed);
        if let Some(cb) = &self.callback {
            cb(0, self.total_size);
        }
    }

    async fn update(&mut self, size: usize) {
        // size is the chunk just downloaded, accumulate it
        let new_total = self.downloaded.fetch_add(size as u64, Ordering::Relaxed) + size as u64;
        if let Some(cb) = &self.callback {
            cb(new_total, self.total_size);
        }
    }

    async fn finish(&mut self) {
        self.downloaded.store(self.total_size, Ordering::Relaxed);
        if let Some(cb) = &self.callback {
            cb(self.total_size, self.total_size);
        }
    }
}

/// Clean up the HF cache file after a successful download and verification.
///
/// Called after the file has been moved or copied from the HF cache to the
/// destination. On a same-filesystem rename the cache file is already gone
/// (source not found → returns Ok immediately). On a cross-filesystem copy the
/// source still exists and is removed here, only after verifying:
/// 1. The destination file exists
/// 2. The destination file size matches the source file size
///
/// # Arguments
///
/// * `source_path` - Path to the file in the HF cache directory
/// * `dest_path` - Path to the final destination in the Tama models directory
///
/// # Returns
///
/// * `Ok(())` if cleanup was successful or not needed (source already gone)
/// * `Err(anyhow::Error)` if safety checks fail or deletion fails
pub async fn cleanup_hf_cache(source_path: &Path, dest_path: &Path) -> Result<()> {
    // Safety check 1: Verify destination exists and get its metadata FIRST
    // This fails fast if dest is gone, preventing TOCTOU race condition
    let dest_meta = tokio::fs::metadata(dest_path).await.with_context(|| {
        format!(
            "Destination file does not exist at '{}', skipping cache cleanup",
            dest_path.display()
        )
    })?;

    // Get source metadata - if not found, there's nothing to clean up (already deleted)
    let source_meta = match tokio::fs::metadata(source_path).await {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                "Source cache file does not exist at '{}', nothing to clean up",
                source_path.display()
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Failed to get metadata for source path: {}",
                    source_path.display()
                )
            });
        }
    };

    // Safety check 2: Verify destination size matches source
    if source_meta.len() != dest_meta.len() {
        anyhow::bail!(
            "Size mismatch: source={}, dest={}, skipping cache cleanup",
            source_meta.len(),
            dest_meta.len()
        );
    }

    // Safe to delete - remove the source file from HF cache
    tokio::fs::remove_file(source_path)
        .await
        .with_context(|| format!("Failed to remove cache file: {}", source_path.display()))?;

    Ok(())
}

/// Download a specific GGUF file using hf-hub's downloader with progress reporting.
/// Uses hf-hub's built-in parallel chunked downloads and caching.
pub async fn download_gguf_with_progress(
    repo_id: &str,
    filename: &str,
    dest_dir: &Path,
    progress_callback: Option<ProgressCallback>,
) -> Result<DownloadResult> {
    let api = crate::models::pull::hf_api().await?;
    let repo = api.model(repo_id.to_string());

    // Check if file already exists with correct size (hf-hub handles caching)
    let dest_path = dest_dir.join(filename);
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Use hf-hub's downloader with our progress adapter
    let progress_adapter = ProgressAdapter::new(progress_callback);

    let cached_path = repo
        .download_with_progress(filename, progress_adapter)
        .await
        .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    // Get file size
    let size_bytes = tokio::fs::metadata(&cached_path)
        .await
        .context("Failed to get file size")?
        .len();

    // Move or copy from cache to destination if different.
    // Canonicalise first — hf-hub snapshot paths are symlinks to the real blob;
    // renaming the symlink entry would leave a broken link at dest_path.
    if cached_path != dest_path {
        if dest_path.exists() {
            tokio::fs::remove_file(&dest_path).await.ok();
        }
        let blob = tokio::fs::canonicalize(&cached_path)
            .await
            .unwrap_or_else(|_| cached_path.clone());
        if tokio::fs::rename(&blob, &dest_path).await.is_err() {
            // Cross-filesystem: copy then delete the blob from cache.
            tokio::fs::copy(&blob, &dest_path)
                .await
                .context("Failed to copy file from cache to destination")?;
            tokio::fs::remove_file(&blob).await.ok();
        }
        // Remove the snapshot symlink if it is distinct from the blob.
        if cached_path != blob {
            tokio::fs::remove_file(&cached_path).await.ok();
        }
    }

    Ok(DownloadResult {
        path: dest_path,
        size_bytes,
    })
}

/// Download a specific GGUF file from a HuggingFace repo to the given model directory.
/// Returns the local path and file size.
/// Downloads directly via reqwest with parallel chunked downloads (bypasses hf-hub's downloader).
#[allow(dead_code)]
pub async fn download_gguf(
    client: &reqwest::Client,
    repo_id: &str,
    filename: &str,
    dest_dir: &Path,
) -> Result<DownloadResult> {
    // Ensure the full directory path exists
    let dest_path = dest_dir.join(filename);
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        repo_id, filename
    );

    // Use chunked parallel download (includes skip-if-exists check)
    let size_bytes = crate::models::download::download_chunked(
        client, &url, &dest_path, 8, // connections
    )
    .await
    .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    Ok(DownloadResult {
        path: dest_path,
        size_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `cleanup_hf_cache` deletes the source file when:
    /// - The destination exists
    /// - The destination size matches the source size
    #[tokio::test]
    async fn test_cleanup_hf_cache_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("source.gguf");
        let dest_path = temp_dir.path().join("dest.gguf");

        // Create source file (simulating HF cache)
        std::fs::write(&source_path, b"test data").unwrap();

        // Create dest file with same size (simulating successful move)
        std::fs::write(&dest_path, b"test data").unwrap();

        // Verify source exists before cleanup
        assert!(source_path.exists());
        assert!(dest_path.exists());

        // Run cleanup
        let result = cleanup_hf_cache(&source_path, &dest_path).await;

        // Verify cleanup succeeded
        assert!(result.is_ok(), "Cleanup should succeed: {:?}", result.err());

        // Verify source was deleted but dest remains
        assert!(
            !source_path.exists(),
            "Source should be deleted after successful cleanup"
        );
        assert!(dest_path.exists(), "Dest should still exist after cleanup");
    }

    /// Verifies that `cleanup_hf_cache` does NOT delete the source file when:
    /// - The destination does not exist (safety check)
    #[tokio::test]
    async fn test_cleanup_hf_cache_dest_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("source.gguf");
        let dest_path = temp_dir.path().join("dest.gguf");

        // Create source file only
        std::fs::write(&source_path, b"test data").unwrap();

        // Verify source exists, dest does not
        assert!(source_path.exists());
        assert!(!dest_path.exists());

        // Run cleanup - should fail safety check
        let result = cleanup_hf_cache(&source_path, &dest_path).await;

        // Verify cleanup was skipped (source still exists)
        assert!(result.is_err(), "Cleanup should fail when dest is missing");
        assert!(
            source_path.exists(),
            "Source should NOT be deleted when dest is missing"
        );
    }

    /// Verifies that `cleanup_hf_cache` does NOT delete the source file when:
    /// - The destination size does not match the source size (safety check)
    #[tokio::test]
    async fn test_cleanup_hf_cache_size_mismatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("source.gguf");
        let dest_path = temp_dir.path().join("dest.gguf");

        // Create source file with specific size
        std::fs::write(&source_path, b"test data").unwrap();

        // Create dest file with different size
        std::fs::write(&dest_path, b"test data with different size").unwrap();

        // Verify both exist with different sizes
        assert!(source_path.exists());
        assert!(dest_path.exists());

        // Run cleanup - should fail size check
        let result = cleanup_hf_cache(&source_path, &dest_path).await;

        // Verify cleanup was skipped (source still exists)
        assert!(result.is_err(), "Cleanup should fail when sizes mismatch");
        assert!(
            source_path.exists(),
            "Source should NOT be deleted when sizes mismatch"
        );
    }

    /// Verifies that `cleanup_hf_cache` handles missing source gracefully
    /// (e.g., if it was already deleted by another process)
    #[tokio::test]
    async fn test_cleanup_hf_cache_source_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source_path = temp_dir.path().join("source.gguf");
        let dest_path = temp_dir.path().join("dest.gguf");

        // Create dest file only (source already gone)
        std::fs::write(&dest_path, b"test data").unwrap();

        // Verify source is missing
        assert!(!source_path.exists());
        assert!(dest_path.exists());

        // Run cleanup - should handle gracefully (not panic)
        let result = cleanup_hf_cache(&source_path, &dest_path).await;

        // Cleanup should succeed (nothing to clean up)
        assert!(
            result.is_ok(),
            "Cleanup should succeed when source is already gone"
        );
    }
}
