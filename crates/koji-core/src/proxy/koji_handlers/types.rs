use serde::{Deserialize, Serialize};

/// Maximum number of quants that can be downloaded in a single pull request.
pub const MAX_CONCURRENT_PULLS: usize = 4;

/// Global mutex serialising post-pull config writes to prevent concurrent-completion races.
pub(super) static CONFIG_WRITE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
