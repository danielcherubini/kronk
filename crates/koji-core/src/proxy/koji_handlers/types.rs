use serde::{Deserialize, Serialize};

/// Maximum number of quants that can be downloaded in a single pull request.
///
/// Configurable via `KOJI_MAX_CONCURRENT_PULLS` environment variable.
/// Default is 8 (increased from original 4 for better parallelism).
/// For network I/O bound downloads, higher values improve throughput
/// without significant CPU/memory overhead.
pub fn max_concurrent_pulls() -> usize {
    std::env::var("KOJI_MAX_CONCURRENT_PULLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
}

/// A single quantisation variant available for a HuggingFace GGUF repo.
#[derive(Debug, Serialize)]
pub struct QuantEntry {
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<i64>,
    /// What kind of file this is (model quant vs vision projector). Used by
    /// the frontend wizard to group files into the correct step.
    pub kind: crate::config::QuantKind,
}

/// A single quantisation variant to download (used in multi-quant wizard format).
#[derive(Debug, Deserialize, Clone)]
pub struct QuantDownloadSpec {
    pub filename: String,
    pub quant: Option<String>,
    pub context_length: Option<u32>,
}

/// Request body for pull job.
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo_id: String,
    /// Quant to download, e.g. "Q4_K_M". Required — omitting returns a 422 with available quants.
    /// Legacy single-quant support (kept for backward compat).
    #[serde(default)]
    pub quant: Option<String>,
    /// New multi-quant wizard format: list of quants to download.
    #[serde(default)]
    pub quants: Vec<QuantDownloadSpec>,
    #[serde(default)]
    pub context_length: Option<u32>,
}

/// Response for a pull job.
#[derive(Debug, Serialize)]
pub struct PullResponse {
    pub job_id: String,
    pub status: String,
    pub repo_id: String,
    pub filename: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
}

/// Response for model load/unload.
#[derive(Debug, Serialize)]
pub struct ModelResponse {
    pub id: String,
    pub loaded: bool,
}

/// Response for system restart.
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct RestartResponse {
    pub message: String,
}

/// Returns `false` if the path component contains traversal sequences or invalid characters.
pub(super) fn is_safe_path_component(s: &str) -> bool {
    !s.is_empty() && !s.contains("..") && !s.contains('/') && !s.contains('\\') && !s.contains('\0')
}

#[cfg(test)]
mod tests {
    use super::*;
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_max_concurrent_pulls_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let original = std::env::var("KOJI_MAX_CONCURRENT_PULLS").ok();

        std::env::remove_var("KOJI_MAX_CONCURRENT_PULLS");
        assert_eq!(max_concurrent_pulls(), 8);

        if let Some(val) = original {
            std::env::set_var("KOJI_MAX_CONCURRENT_PULLS", val);
        }
    }

    #[test]
    fn test_max_concurrent_pulls_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("KOJI_MAX_CONCURRENT_PULLS", "16");
        }
        assert_eq!(max_concurrent_pulls(), 16);
        unsafe {
            std::env::remove_var("KOJI_MAX_CONCURRENT_PULLS");
        }
    }

    #[test]
    fn test_max_concurrent_pulls_invalid_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("KOJI_MAX_CONCURRENT_PULLS", "not_a_number");
        }
        // Should fall back to default
        assert_eq!(max_concurrent_pulls(), 8);
        unsafe {
            std::env::remove_var("KOJI_MAX_CONCURRENT_PULLS");
        }
    }

    #[test]
    fn test_is_safe_path_component_valid() {
        assert!(is_safe_path_component("model.gguf"));
        assert!(is_safe_path_component("Q4_K_M"));
        assert!(is_safe_path_component("unsloth"));
    }

    #[test]
    fn test_is_safe_path_component_invalid() {
        assert!(!is_safe_path_component(""));
        assert!(!is_safe_path_component(".."));
        assert!(!is_safe_path_component("../etc"));
        assert!(!is_safe_path_component("path/to/file"));
        assert!(!is_safe_path_component("path\\to\\file"));
        assert!(!is_safe_path_component("path\0null"));
    }
}
