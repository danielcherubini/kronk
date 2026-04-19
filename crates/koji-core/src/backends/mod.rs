pub mod installer;
pub mod registry;
pub mod updater;

pub use installer::{install_backend, install_backend_with_progress, InstallOptions};
pub use registry::{BackendInfo, BackendRegistry, BackendSource, BackendType};
pub use updater::{
    check_latest_version, check_updates, update_backend, update_backend_with_progress, UpdateCheck,
};

use crate::config::Config;
use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

/// Trait for logging progress during backend installation.
pub trait ProgressSink: Send + Sync {
    fn log(&self, line: &str);

    /// Called with benchmark results as JSON when a benchmark completes.
    fn result(&self, json: &str);
}

/// A no-op implementation of ProgressSink for use when no progress tracking is needed.
pub struct NullSink;

impl ProgressSink for NullSink {
    fn log(&self, _line: &str) {}
    fn result(&self, _json: &str) {}
}

/// Returns the backends directory path: `<config_dir>/backends`.
/// Creates the directory if it doesn't exist.
pub fn backends_dir() -> Result<PathBuf> {
    let base_dir = Config::base_dir()?;
    let backends_dir = base_dir.join("backends");
    std::fs::create_dir_all(&backends_dir).with_context(|| {
        format!(
            "Failed to create backends directory: {}",
            backends_dir.display()
        )
    })?;
    Ok(backends_dir)
}

/// Safely removes a backend installation by validating the path is within the managed backends directory.
///
/// This function canonicalizes both the target path and the backends directory, then verifies
/// the target is within the managed directory before deletion. This prevents directory traversal
/// attacks and accidental deletion of files outside the backends directory.
///
/// On Windows, if removal fails with PermissionDenied, it retries once after a short delay.
pub fn safe_remove_installation(info: &BackendInfo) -> Result<()> {
    let parent = info
        .path
        .parent()
        .ok_or_else(|| anyhow!("Failed to get parent directory of backend path"))?;

    let canonical_parent = std::fs::canonicalize(parent).with_context(|| {
        format!(
            "Failed to canonicalize backend parent path: {}",
            parent.display()
        )
    })?;

    let managed = backends_dir().with_context(|| "Failed to get backends directory")?;
    let canonical_managed = std::fs::canonicalize(&managed).with_context(|| {
        format!(
            "Failed to canonicalize backends directory: {}",
            managed.display()
        )
    })?;

    if !canonical_parent.starts_with(&canonical_managed) {
        return Err(anyhow!("path is outside the managed backends directory"));
    }

    // On Windows, remove_dir_all fails if a process is using the directory
    #[cfg(windows)]
    {
        use std::io::ErrorKind;
        match std::fs::remove_dir_all(parent) {
            Ok(_) => {
                tracing::info!("Files removed.");
            }
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                tracing::warn!("Skipping file removal: backend may still be running. Retrying...");
                std::thread::sleep(std::time::Duration::from_millis(500));
                match std::fs::remove_dir_all(parent) {
                    Ok(_) => {
                        tracing::info!("Files removed.");
                    }
                    Err(e) => {
                        tracing::warn!("Skipping file removal: {}", e);
                        return Err(anyhow!("Failed to remove backend directory: {}", e));
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Skipping file removal: {}", e);
                return Err(anyhow!("Failed to remove backend directory: {}", e));
            }
        }
    }

    // On Unix, remove_dir_all will fail if directory is in use
    #[cfg(not(windows))]
    {
        match std::fs::remove_dir_all(parent) {
            Ok(_) => {
                tracing::info!("Files removed.");
            }
            Err(e) => {
                tracing::warn!("Skipping file removal: {}", e);
                return Err(anyhow!("Failed to remove backend directory: {}", e));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backends_dir_returns_config_subdir() {
        let path = backends_dir().expect("backends_dir() should succeed");
        assert!(
            path.ends_with("backends"),
            "backends_dir() should return a path ending in 'backends', got: {:?}",
            path
        );
        assert!(
            path.exists(),
            "backends_dir() should create the directory if missing"
        );
    }

    #[test]
    fn test_safe_remove_installation_rejects_outside_path() {
        // Use a real temp directory path that exists on all platforms.
        // Keep the TempDir alive so the path remains valid during the test.
        let _outside_dir = tempfile::tempdir().expect("tempdir");
        let outside_path = _outside_dir.path().join("llama-server");
        let outside_info = BackendInfo {
            name: "test".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "test".to_string(),
            path: outside_path,
            installed_at: 0,
            gpu_type: None,
            source: None,
        };

        let result = safe_remove_installation(&outside_info);
        assert!(
            result.is_err(),
            "safe_remove_installation should reject paths outside backends_dir()"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("outside the managed backends directory"),
            "Error message should mention 'outside the managed backends directory', got: {}",
            err_msg
        );
    }

    #[test]
    fn test_progress_sink_trait() {
        // NullSink should implement ProgressSink
        let sink: NullSink = NullSink;
        sink.log("test line"); // Should not panic
    }
}
