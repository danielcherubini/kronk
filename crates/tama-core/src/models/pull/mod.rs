use anyhow::{Context, Result};
use hf_hub::api::tokio::{Api, ApiBuilder};
use tokio::sync::OnceCell;

pub mod api;
pub mod download;
pub mod metadata;
pub mod quant;

static HF_API: OnceCell<Api> = OnceCell::const_new();

/// Get or create the shared HuggingFace API client.
/// Configured with max_files=8 for parallel file downloads.
///
/// **Note:** This uses `ApiBuilder::new()` which respects the `HF_HOME` environment
/// variable for cache location. No explicit cache path is set, so `hf-hub` will use
/// its default behavior:
/// - If `HF_HOME` is set: `$HF_HOME/hub`
/// - Otherwise: `~/.cache/huggingface/hub`
pub(crate) async fn hf_api() -> Result<&'static Api> {
    HF_API
        .get_or_try_init(|| async {
            ApiBuilder::new()
                .with_max_files(8) // Allow 8 concurrent file downloads
                .build()
                .context("Failed to initialise HuggingFace API client")
        })
        .await
}

/// Information about a GGUF file in a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct RemoteGguf {
    /// Filename, e.g. "OmniCoder-8B-Q4_K_M.gguf"
    pub filename: String,
    /// Inferred quant type from filename, e.g. "Q4_K_M"
    pub quant: Option<String>,
}

/// Result of listing GGUF files from a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct RepoGgufListing {
    /// Resolved repo ID (may differ from input if `-GGUF` was appended)
    pub repo_id: String,
    /// HF repo HEAD commit SHA at time of listing
    pub commit_sha: String,
    /// Available GGUF files
    pub files: Vec<RemoteGguf>,
}

/// Per-file blob metadata returned by the HuggingFace blobs API.
#[derive(Debug, Clone)]
pub struct BlobInfo {
    pub filename: String,
    pub blob_id: Option<String>,
    pub size: Option<i64>,
    pub lfs_sha256: Option<String>,
}

/// Metadata extracted from HuggingFace API and README for a model.
/// Internal data-transfer type between the fetcher and the DB update helper.
#[derive(Debug, Clone, Default)]
pub struct HfModelMetadata {
    pub hf_format: Option<String>,
    pub hf_base_model: Option<String>,
    pub hf_pipeline_tag: Option<String>,
    pub hf_total_params: Option<String>,
    pub hf_active_params: Option<String>,
    pub hf_architecture_type: Option<String>,
    pub hf_context_length: Option<u32>,
    pub hf_num_layers: Option<u32>,
    pub hf_last_modified: Option<String>,
}

// ── Re-exports from sub-modules ──────────────────────────────────────────────

pub use api::{
    fetch_blob_metadata, fetch_hf_metadata, fetch_model_pipeline_tag,
    infer_modalities_from_pipeline, list_gguf_files, parse_blob_siblings,
};
pub use download::{
    cleanup_hf_cache, download_gguf, download_gguf_with_progress, DownloadResult, ProgressAdapter,
};
pub use metadata::{fetch_community_card, parse_readme_metadata};
pub use quant::infer_quant_from_filename;
