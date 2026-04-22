pub mod installer;
pub mod registry;
pub mod tts_kokoro;
pub mod updater;

pub use installer::{install_backend, install_backend_with_progress, InstallOptions};
pub use registry::{BackendInfo, BackendRegistry, BackendSource, BackendType};
pub use tts_kokoro::install_tts_kokoro;
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
    // Determine what to remove:
    // - If path is a directory (TTS backends), remove the path itself
    // - If path is a binary file (llama_cpp, ik_llama), remove its parent directory
    let target = if info.path.is_dir() {
        info.path.clone()
    } else {
        info.path
            .parent()
            .ok_or_else(|| anyhow!("Failed to get parent directory of backend path"))?
            .to_path_buf()
    };

    let canonical_target = std::fs::canonicalize(&target)
        .with_context(|| format!("Failed to canonicalize backend path: {}", target.display()))?;

    let managed = backends_dir().with_context(|| "Failed to get backends directory")?;
    let canonical_managed = std::fs::canonicalize(&managed).with_context(|| {
        format!(
            "Failed to canonicalize backends directory: {}",
            managed.display()
        )
    })?;

    if !canonical_target.starts_with(&canonical_managed) {
        return Err(anyhow!("path is outside the managed backends directory"));
    }

    // On Windows, remove_dir_all fails if a process is using the directory
    #[cfg(windows)]
    {
        use std::io::ErrorKind;
        match std::fs::remove_dir_all(&target) {
            Ok(_) => {
                tracing::info!("Files removed.");
            }
            Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                tracing::warn!("Skipping file removal: backend may still be running. Retrying...");
                std::thread::sleep(std::time::Duration::from_millis(500));
                match std::fs::remove_dir_all(&target) {
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
        match std::fs::remove_dir_all(&target) {
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

    /// Verify that `safe_remove_installation` removes the entire tts_kokoro directory
    /// when the BackendInfo path points to a directory (the base_dir).
    ///
    /// This simulates the new layout where:
    ///   backends/tts_kokoro/       <- info.path (base_dir, is_dir)
    ///     kokoro-fastapi/         <- git clone target
    ///       api/src/main.py
    ///     venv/                   <- virtualenv
    ///
    /// safe_remove_installation should detect that info.path is a directory and remove
    /// it entirely (including all nested files and subdirectories).
    #[test]
    fn test_safe_remove_tts_kokoro_directory() {
        // Create the structure inside the real backends_dir so canonicalization passes.
        let managed_backends = backends_dir().expect("backends_dir should exist");

        // Simulate: <backends>/tts_kokoro_test/kokoro-fastapi/api/src/main.py
        let test_base = managed_backends.join("tts_kokoro_test");
        std::fs::create_dir_all(test_base.join("kokoro-fastapi").join("api").join("src"))
            .expect("create kokoro-fastapi dir structure");
        std::fs::write(
            test_base
                .join("kokoro-fastapi")
                .join("api")
                .join("src")
                .join("main.py"),
            "# mock",
        )
        .expect("write main.py");

        // Simulate: <backends>/tts_kokoro_test/venv/bin/python
        std::fs::create_dir_all(test_base.join("venv").join("bin"))
            .expect("create venv dir structure");
        std::fs::write(
            test_base.join("venv").join("bin").join("python"),
            "#!/bin/sh",
        )
        .expect("write python mock");

        // Verify the structure exists before removal
        assert!(test_base.is_dir(), "tts_kokoro_test base_dir should exist");
        assert!(
            test_base
                .join("kokoro-fastapi")
                .join("api")
                .join("src")
                .join("main.py")
                .exists(),
            "main.py should exist before removal"
        );
        assert!(
            test_base.join("venv").join("bin").join("python").exists(),
            "venv python should exist before removal"
        );

        // Create BackendInfo with path pointing to the directory (base_dir)
        let info = BackendInfo {
            name: "tts_kokoro".to_string(),
            backend_type: BackendType::TtsKokoro,
            version: "v0.3.0".to_string(),
            path: test_base.clone(), // This is a directory, not a binary
            installed_at: 0,
            gpu_type: None,
            source: None,
        };

        // Call safe_remove_installation — since info.path is a directory,
        // it should remove the entire tts_kokoro_test/ directory.
        let result = safe_remove_installation(&info);
        assert!(
            result.is_ok(),
            "safe_remove_installation should succeed for TTS backend dir, got: {:?}",
            result
        );

        // Verify the entire tts_kokoro_test directory is gone
        assert!(
            !test_base.exists(),
            "tts_kokoro_test base_dir should be removed after safe_remove_installation"
        );
    }
}
