use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use hf_hub::api::tokio::{Api, ApiBuilder};
use tokio::sync::OnceCell;

use crate::models::card::ModelCard;

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

/// List GGUF files available in a HuggingFace model repository.
/// Returns a `RepoGgufListing` with the resolved repo_id, commit SHA, and file list.
///
/// Auto-resolves repos: if `repo_id` doesn't end with `-GGUF` and the initial
/// fetch finds no GGUF files (or the repo doesn't exist), retries with `-GGUF` appended.
pub async fn list_gguf_files(repo_id: &str) -> Result<RepoGgufListing> {
    let api = hf_api().await?;

    // Try the repo_id as given first
    let candidates = if repo_id.to_uppercase().ends_with("-GGUF") {
        vec![repo_id.to_string()]
    } else {
        vec![repo_id.to_string(), format!("{}-GGUF", repo_id)]
    };

    let mut last_error: Option<String> = None;

    for candidate in &candidates {
        let repo = api.model(candidate.clone());
        match repo.info().await {
            Ok(info) => {
                let commit_sha = info.sha.clone();
                let ggufs: Vec<RemoteGguf> = info
                    .siblings
                    .into_iter()
                    .filter(|s| s.rfilename.ends_with(".gguf"))
                    .map(|s| {
                        let quant = infer_quant_from_filename(&s.rfilename);
                        RemoteGguf {
                            filename: s.rfilename,
                            quant,
                        }
                    })
                    .collect();

                if !ggufs.is_empty() {
                    return Ok(RepoGgufListing {
                        repo_id: candidate.clone(),
                        commit_sha,
                        files: ggufs,
                    });
                }
                // Repo exists but no GGUFs — try next candidate
                last_error = Some(format!(
                    "'{}' exists but contains no .gguf files",
                    candidate
                ));
            }
            Err(e) => {
                last_error = Some(format!("'{}': {}", candidate, e));
                continue;
            }
        }
    }

    let detail = last_error.unwrap_or_else(|| "unknown error".to_string());
    anyhow::bail!(
        "No GGUF files found. Tried: {}\nLast error: {}",
        candidates.join(", "),
        detail
    )
}

/// Fetch per-file blob metadata from HuggingFace using the blobs API.
///
/// Uses `hf_hub`'s authenticated client to call the HF API with `?blobs=true`,
/// which returns `blobId`, `size`, and `lfs.sha256` per sibling.
/// Returns a map of filename → BlobInfo for GGUF files only.
pub async fn fetch_blob_metadata(repo_id: &str) -> Result<HashMap<String, BlobInfo>> {
    let api = hf_api().await?;
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let url = format!("{}/api/models/{}?blobs=true", endpoint, repo_id);

    let response = api
        .client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch blob metadata for '{}'", repo_id))?
        .error_for_status()
        .with_context(|| {
            format!(
                "HuggingFace returned an error for blob metadata request for '{}'",
                repo_id
            )
        })?
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("Failed to parse blob metadata response for '{}'", repo_id))?;

    Ok(parse_blob_siblings(&response))
}

/// Fetch the pipeline_tag from HuggingFace model metadata API.
///
/// Returns the `pipeline_tag` field from the model metadata, which indicates
/// the model's task type (e.g., "text-generation", "image-text-to-text").
pub async fn fetch_model_pipeline_tag(repo_id: &str) -> Result<Option<String>> {
    let api = hf_api().await?;
    let endpoint =
        std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string());
    let url = format!("{}/api/models/{}", endpoint, repo_id);

    let response = api
        .client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch model metadata for '{}'", repo_id))?
        .error_for_status()
        .with_context(|| {
            format!(
                "HuggingFace returned an error for model metadata request for '{}'",
                repo_id
            )
        })?
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("Failed to parse model metadata response for '{}'", repo_id))?;

    Ok(response
        .get("pipeline_tag")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string()))
}

/// Try to infer modalities from a HuggingFace pipeline tag.
///
/// Order matters: more specific checks (e.g., "text-to-speech") must come
/// before broader ones (e.g., "speech") to avoid misclassification.
pub fn infer_modalities_from_pipeline(
    pipeline_tag: Option<&str>,
) -> Option<crate::config::ModelModalities> {
    let tag = pipeline_tag?.to_lowercase();

    if tag.contains("vision") || tag.contains("image-text") {
        Some(crate::config::ModelModalities {
            input: vec!["text".to_string(), "image".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("text-generation")
        || tag.contains("conversational")
        || tag.contains("chat")
    {
        Some(crate::config::ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("image-classification") || tag.contains("object-detection") {
        Some(crate::config::ModelModalities {
            input: vec!["image".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("text-to-speech") || tag.contains("tts") {
        // Must check TTS before generic "speech"/"audio" to avoid misclassification.
        Some(crate::config::ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["audio".to_string()],
        })
    } else if tag.contains("speech") || tag.contains("audio") {
        Some(crate::config::ModelModalities {
            input: vec!["audio".to_string()],
            output: vec!["text".to_string()],
        })
    } else if tag.contains("embedding") || tag.contains("feature-extraction") {
        Some(crate::config::ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["embedding".to_string()],
        })
    } else if tag.contains("image-generation") || tag.contains("text-to-image") {
        Some(crate::config::ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["image".to_string()],
        })
    } else {
        None
    }
}

/// Parse the `siblings` array from a HuggingFace blobs API response.
///
/// This is a pure function for testability — extract from `fetch_blob_metadata`
/// so it can be unit-tested with fixture data.
pub fn parse_blob_siblings(value: &serde_json::Value) -> HashMap<String, BlobInfo> {
    let mut result = HashMap::new();

    let siblings = match value.get("siblings").and_then(|s| s.as_array()) {
        Some(s) => s,
        None => return result,
    };

    for sibling in siblings {
        let rfilename = match sibling.get("rfilename").and_then(|f| f.as_str()) {
            Some(f) => f,
            None => continue,
        };

        if !rfilename.ends_with(".gguf") {
            continue;
        }

        let blob_id = sibling
            .get("blobId")
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());

        let size = sibling.get("size").and_then(|s| s.as_i64());

        let lfs_sha256 = sibling
            .get("lfs")
            .and_then(|lfs| lfs.get("sha256"))
            .and_then(|sha| sha.as_str())
            .map(|s| s.to_string());

        result.insert(
            rfilename.to_string(),
            BlobInfo {
                filename: rfilename.to_string(),
                blob_id,
                size,
                lfs_sha256,
            },
        );
    }

    result
}

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
    downloaded: std::sync::Arc<std::sync::atomic::AtomicU64>,
    callback: Option<crate::models::download::ProgressCallback>,
}

impl ProgressAdapter {
    pub fn new(callback: Option<crate::models::download::ProgressCallback>) -> Self {
        Self {
            total_size: 0,
            downloaded: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            callback,
        }
    }
}

impl hf_hub::api::tokio::Progress for ProgressAdapter {
    async fn init(&mut self, size: usize, _filename: &str) {
        self.total_size = size as u64;
        self.downloaded
            .store(0, std::sync::atomic::Ordering::Relaxed);
        if let Some(cb) = &self.callback {
            cb(0, self.total_size);
        }
    }

    async fn update(&mut self, size: usize) {
        // size is the chunk just downloaded, accumulate it
        let new_total = self
            .downloaded
            .fetch_add(size as u64, std::sync::atomic::Ordering::Relaxed)
            + size as u64;
        if let Some(cb) = &self.callback {
            cb(new_total, self.total_size);
        }
    }

    async fn finish(&mut self) {
        self.downloaded
            .store(self.total_size, std::sync::atomic::Ordering::Relaxed);
        if let Some(cb) = &self.callback {
            cb(self.total_size, self.total_size);
        }
    }
}

/// Clean up the HF cache file after a successful download and verification.
///
/// This function removes the source file from the HF cache directory only after
/// verifying that:
/// 1. The destination file exists
/// 2. The destination file size matches the source file size
///
/// This is a safety measure to prevent accidental data loss. If the destination
/// is missing or has a different size, the source file is preserved.
///
/// # Arguments
///
/// * `source_path` - Path to the file in the HF cache directory
/// * `dest_path` - Path to the final destination in the Koji models directory
///
/// # Returns
///
/// * `Ok(())` if cleanup was successful or not needed (source already gone)
/// * `Err(anyhow::Error)` if safety checks fail or deletion fails
pub async fn cleanup_hf_cache(
    source_path: &std::path::Path,
    dest_path: &std::path::Path,
) -> Result<()> {
    // Safety check 1: Verify destination exists
    if !dest_path.exists() {
        anyhow::bail!(
            "Destination file does not exist at '{}', skipping cache cleanup",
            dest_path.display()
        );
    }

    // If source doesn't exist, there's nothing to clean up (already deleted)
    if !source_path.exists() {
        tracing::debug!(
            "Source cache file does not exist at '{}', nothing to clean up",
            source_path.display()
        );
        return Ok(());
    }

    // Safety check 2: Verify destination size matches source
    let source_meta = std::fs::metadata(source_path).with_context(|| {
        format!(
            "Failed to get metadata for source path: {}",
            source_path.display()
        )
    })?;

    let dest_meta = std::fs::metadata(dest_path).with_context(|| {
        format!(
            "Failed to get metadata for dest path: {}",
            dest_path.display()
        )
    })?;

    if source_meta.len() != dest_meta.len() {
        anyhow::bail!(
            "Size mismatch: source={}, dest={}, skipping cache cleanup",
            source_meta.len(),
            dest_meta.len()
        );
    }

    // Safe to delete - remove the source file from HF cache
    std::fs::remove_file(source_path)
        .with_context(|| format!("Failed to remove cache file: {}", source_path.display()))?;

    Ok(())
}

/// Download a specific GGUF file using hf-hub's downloader with progress reporting.
/// Uses hf-hub's built-in parallel chunked downloads and caching.
pub async fn download_gguf_with_progress(
    repo_id: &str,
    filename: &str,
    dest_dir: &std::path::Path,
    progress_callback: Option<crate::models::download::ProgressCallback>,
) -> Result<DownloadResult> {
    let api = hf_api().await?;
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

    // Copy/symlink from cache to destination if different
    if cached_path != dest_path {
        // Remove existing file if present
        if dest_path.exists() {
            tokio::fs::remove_file(&dest_path).await.ok();
        }
        // Create symlink from cache to destination
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&cached_path, &dest_path)
                .context("Failed to create symlink")?;
        }
        #[cfg(windows)]
        {
            std::fs::hard_link(&cached_path, &dest_path).context("Failed to create hard link")?;
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
    dest_dir: &std::path::Path,
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

const MODELCARDS_BASE_URL: &str =
    "https://raw.githubusercontent.com/danielcherubini/koji/main/modelcards";

/// Try to fetch a community model card from the koji repository.
///
/// Attempts several name variants derived from the repo_id:
/// 1. Exact: `{company}/{model}.toml` (e.g. `Tesslate/OmniCoder-9B-GGUF.toml`)
/// 2. Strip `-GGUF` suffix: `Tesslate/OmniCoder-9B.toml`
/// 3. Strip `-gguf` suffix (lowercase)
///
/// Returns `None` silently on network errors or 404s.
pub async fn fetch_community_card(repo_id: &str) -> Option<ModelCard> {
    let parts: Vec<&str> = repo_id.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let (company, model) = (parts[0], parts[1]);

    // Build candidate names: exact, then stripped variants
    let mut candidates = vec![model.to_string()];
    for suffix in ["-GGUF", "-gguf", "-Gguf"] {
        if let Some(stripped) = model.strip_suffix(suffix) {
            candidates.push(stripped.to_string());
        }
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    for name in &candidates {
        let url = format!("{}/{}/{}.toml", MODELCARDS_BASE_URL, company, name);
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    if let Ok(card) = toml::from_str::<ModelCard>(&body) {
                        return Some(card);
                    }
                }
            }
        }
    }

    None
}

/// Try to infer the quantisation type from a GGUF filename.
/// Common patterns: "Model-Q4_K_M.gguf", "model.Q8_0.gguf", "model-q4_k_m.gguf"
pub fn infer_quant_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".gguf")?;

    // Ordered longest-first so "Q4_K_M" matches before "Q4_K"
    // Includes UD (Unsloth Dynamic) and APEX variants
    let quant_patterns = [
        // APEX semantic quants (must come before APEX standard patterns)
        "APEX-I-BALANCED",
        "APEX-I-QUALITY",
        "APEX-I-COMPACT",
        "APEX-I-MINI",
        // APEX IQ quants
        "APEX-IQ2_XXS",
        "APEX-IQ3_XXS",
        "APEX-IQ1_S",
        "APEX-IQ1_M",
        "APEX-IQ2_XS",
        "APEX-IQ2_S",
        "APEX-IQ2_M",
        "APEX-IQ3_XS",
        "APEX-IQ3_S",
        "APEX-IQ3_M",
        "APEX-IQ4_XS",
        "APEX-IQ4_NL",
        // APEX standard quants
        "APEX-Q2_K_S",
        "APEX-Q3_K_S",
        "APEX-Q3_K_M",
        "APEX-Q3_K_L",
        "APEX-Q4_K_S",
        "APEX-Q4_K_M",
        "APEX-Q4_K_L",
        "APEX-Q5_K_S",
        "APEX-Q5_K_M",
        "APEX-Q5_K_L",
        "APEX-Q6_K",
        "APEX-Q8_0",
        // UD semantic quants (must come before UD standard patterns)
        "UD-I-BALANCED",
        "UD-I-QUALITY",
        "UD-I-COMPACT",
        "UD-I-MINI",
        // Unsloth Dynamic (UD) IQ quants
        "UD-IQ2_XXS",
        "UD-IQ3_XXS",
        "UD-IQ1_S",
        "UD-IQ1_M",
        "UD-IQ2_XS",
        "UD-IQ2_S",
        "UD-IQ2_M",
        "UD-IQ3_XS",
        "UD-IQ3_S",
        "UD-IQ3_M",
        "UD-IQ4_XS",
        "UD-IQ4_NL",
        // Unsloth Dynamic (UD) standard quants
        "UD-Q2_K_S",
        "UD-Q3_K_S",
        "UD-Q3_K_M",
        "UD-Q3_K_L",
        "UD-Q4_K_S",
        "UD-Q4_K_M",
        "UD-Q4_K_L",
        "UD-Q5_K_S",
        "UD-Q5_K_M",
        "UD-Q5_K_L",
        "UD-Q2_K_XL",
        "UD-Q3_K_XL",
        "UD-Q4_K_XL",
        "UD-Q5_K_XL",
        "UD-Q6_K_XL",
        "UD-Q8_K_XL",
        "UD-Q2_K",
        "UD-Q3_K",
        "UD-Q4_K",
        "UD-Q5_K",
        "UD-Q6_K",
        "UD-Q4_0",
        "UD-Q4_1",
        "UD-Q5_0",
        "UD-Q5_1",
        "UD-Q6_0",
        "UD-Q8_0",
        "UD-Q8_1",
        // Standard quants
        "IQ2_XXS",
        "IQ3_XXS",
        "IQ1_S",
        "IQ1_M",
        "IQ2_XS",
        "IQ2_S",
        "IQ2_M",
        "IQ3_XS",
        "IQ3_S",
        "IQ3_M",
        "IQ4_XS",
        "IQ4_NL",
        "Q2_K_S",
        "Q3_K_S",
        "Q3_K_M",
        "Q3_K_L",
        "Q4_K_S",
        "Q4_K_M",
        "Q4_K_L",
        "Q5_K_S",
        "Q5_K_M",
        "Q5_K_L",
        "Q2_K_XL",
        "Q3_K_XL",
        "Q4_K_XL",
        "Q5_K_XL",
        "Q6_K_XL",
        "Q8_K_XL",
        "Q2_K",
        "Q3_K",
        "Q4_K",
        "Q5_K",
        "Q6_K",
        "Q4_0",
        "Q4_1",
        "Q5_0",
        "Q5_1",
        "Q6_0",
        "Q8_0",
        "Q8_1",
        "F16",
        "F32",
        "BF16",
    ];

    let stem_upper = stem.to_uppercase();
    for pattern in &quant_patterns {
        if stem_upper.ends_with(pattern)
            || stem_upper.contains(&format!("-{}", pattern))
            || stem_upper.contains(&format!(".{}", pattern))
            || stem_upper.contains(&format!("_{}", pattern))
        {
            return Some(pattern.to_string());
        }
    }

    // No standard quant pattern found. Fall back to the last component
    // after splitting by `-` or `_`. For "Qwen3.5-35B-A3B-APEX-I-Balanced",
    // this returns "I-Balanced" instead of the full stem.
    stem.split(|c| ['-', '_'].contains(&c))
        .next_back()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Verifies that GGUF siblings are parsed with blobId, size, and LFS SHA256,
    /// and that non-GGUF files (e.g. README.md) are excluded from the result.
    #[test]
    fn test_parse_blob_siblings_basic() {
        let json = serde_json::json!({
            "siblings": [
                {
                    "rfilename": "README.md",
                    "blobId": "blob1",
                    "size": 1000
                },
                {
                    "rfilename": "Model-Q4_K_M.gguf",
                    "blobId": "blob2",
                    "size": 4200000000_i64,
                    "lfs": {
                        "sha256": "abcdef1234567890"
                    }
                },
                {
                    "rfilename": "Model-Q8_0.gguf",
                    "blobId": "blob3",
                    "size": 8400000000_i64,
                    "lfs": {
                        "sha256": "fedcba0987654321"
                    }
                }
            ]
        });

        let result = parse_blob_siblings(&json);

        // README should be excluded
        assert!(!result.contains_key("README.md"));
        assert_eq!(result.len(), 2);

        let q4 = result.get("Model-Q4_K_M.gguf").unwrap();
        assert_eq!(q4.blob_id.as_deref(), Some("blob2"));
        assert_eq!(q4.size, Some(4200000000_i64));
        assert_eq!(q4.lfs_sha256.as_deref(), Some("abcdef1234567890"));

        let q8 = result.get("Model-Q8_0.gguf").unwrap();
        assert_eq!(q8.lfs_sha256.as_deref(), Some("fedcba0987654321"));
    }

    /// Verifies that a GGUF sibling without an `lfs` field has `lfs_sha256 = None`.
    #[test]
    fn test_parse_blob_siblings_no_lfs() {
        let json = serde_json::json!({
            "siblings": [
                {
                    "rfilename": "model.gguf",
                    "blobId": "blob1",
                    "size": 1000
                }
            ]
        });

        let result = parse_blob_siblings(&json);
        let info = result.get("model.gguf").unwrap();
        assert!(info.lfs_sha256.is_none());
        assert_eq!(info.size, Some(1000));
    }

    /// Verifies that an empty `siblings` array produces an empty map.
    #[test]
    fn test_parse_blob_siblings_empty() {
        let json = serde_json::json!({ "siblings": [] });
        let result: HashMap<_, _> = parse_blob_siblings(&json);
        assert!(result.is_empty());
    }

    /// Verifies that a response without a `siblings` key produces an empty map.
    #[test]
    fn test_parse_blob_siblings_no_siblings_key() {
        let json = serde_json::json!({});
        let result = parse_blob_siblings(&json);
        assert!(result.is_empty());
    }

    #[test]
    fn test_infer_quant_q4_k_m() {
        assert_eq!(
            infer_quant_from_filename("OmniCoder-8B-Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_q8_0() {
        assert_eq!(
            infer_quant_from_filename("model-Q8_0.gguf"),
            Some("Q8_0".to_string())
        );
    }

    #[test]
    fn test_infer_quant_non_standard_name() {
        // APEX semantic quants are now recognized
        // "Qwen3.5-35B-A3B-APEX-I-Balanced" -> "APEX-I-BALANCED"
        assert_eq!(
            infer_quant_from_filename("Qwen3.5-35B-A3B-APEX-I-Balanced.gguf"),
            Some("APEX-I-BALANCED".to_string())
        );
    }

    #[test]
    fn test_infer_quant_with_underscore() {
        assert_eq!(
            infer_quant_from_filename("model-Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
        // Returns the matched pattern, not the full suffix
        assert_eq!(
            infer_quant_from_filename("model-Q4_K_M_v2.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_lowercase() {
        assert_eq!(
            infer_quant_from_filename("model-q4_k_m.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_f16() {
        assert_eq!(
            infer_quant_from_filename("model-F16.gguf"),
            Some("F16".to_string())
        );
    }

    #[test]
    fn test_infer_quant_none() {
        // Returns last component when no pattern matches
        assert_eq!(
            infer_quant_from_filename("model.gguf"),
            Some("model".to_string())
        );
    }

    #[test]
    fn test_infer_quant_dot_separator() {
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-1B-Instruct.Q6_K.gguf"),
            Some("Q6_K".to_string())
        );
    }

    #[test]
    fn test_infer_quant_iq() {
        assert_eq!(
            infer_quant_from_filename("model-IQ4_NL.gguf"),
            Some("IQ4_NL".to_string())
        );
    }

    #[test]
    fn test_infer_quant_xl() {
        assert_eq!(
            infer_quant_from_filename("model-Q4_K_XL.gguf"),
            Some("Q4_K_XL".to_string())
        );
    }

    #[test]
    fn test_infer_quant_xl_lowercase() {
        assert_eq!(
            infer_quant_from_filename("model-q5_k_xl.gguf"),
            Some("Q5_K_XL".to_string())
        );
    }

    #[test]
    fn test_infer_quant_ud() {
        assert_eq!(
            infer_quant_from_filename("model-UD-Q4_K_XL.gguf"),
            Some("UD-Q4_K_XL".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-UD-Q4_K_M.gguf"),
            Some("UD-Q4_K_M".to_string())
        );
    }

    // ── APEX and UD semantic quant tests ──────────────────────────────────────

    #[test]
    fn test_infer_quant_apex_patterns() {
        // APEX IQ quants
        assert_eq!(
            infer_quant_from_filename("model-APEX-IQ2_XXS.gguf"),
            Some("APEX-IQ2_XXS".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-APEX-IQ3_XXS.gguf"),
            Some("APEX-IQ3_XXS".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-APEX-IQ4_NL.gguf"),
            Some("APEX-IQ4_NL".to_string())
        );
        // APEX standard quants
        assert_eq!(
            infer_quant_from_filename("model-APEX-Q4_K_M.gguf"),
            Some("APEX-Q4_K_M".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-APEX-Q8_0.gguf"),
            Some("APEX-Q8_0".to_string())
        );
    }

    #[test]
    fn test_infer_quant_apex_semantic() {
        // APEX semantic quants (I-Balanced, I-Quality, etc.)
        // Note: function returns uppercase patterns
        assert_eq!(
            infer_quant_from_filename("gemma-4-26B-A4B-APEX-I-Balanced.gguf"),
            Some("APEX-I-BALANCED".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Qwen3.5-35B-A3B-APEX-I-Quality.gguf"),
            Some("APEX-I-QUALITY".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-APEX-I-Compact.gguf"),
            Some("APEX-I-COMPACT".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-APEX-I-Mini.gguf"),
            Some("APEX-I-MINI".to_string())
        );
    }

    #[test]
    fn test_infer_quant_ud_semantic() {
        // UD semantic quants
        // Note: function returns uppercase patterns
        assert_eq!(
            infer_quant_from_filename("gemma-4-26B-A4B-UD-I-Balanced.gguf"),
            Some("UD-I-BALANCED".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Qwen3.5-35B-A3B-UD-I-Quality.gguf"),
            Some("UD-I-QUALITY".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-UD-I-Compact.gguf"),
            Some("UD-I-COMPACT".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-UD-I-Mini.gguf"),
            Some("UD-I-MINI".to_string())
        );
    }

    #[test]
    fn test_infer_quant_semantic_without_prefix() {
        // Semantic quants without APEX/UD prefix should fall back gracefully
        // Returns last component from original stem (preserves case)
        assert_eq!(
            infer_quant_from_filename("model-I-Balanced.gguf"),
            Some("Balanced".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-Quality.gguf"),
            Some("Quality".to_string())
        );
    }

    // ── HF Cache Cleanup tests ───────────────────────────────────────────────

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
        let result = super::cleanup_hf_cache(&source_path, &dest_path).await;

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
        let result = super::cleanup_hf_cache(&source_path, &dest_path).await;

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
        let result = super::cleanup_hf_cache(&source_path, &dest_path).await;

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
        let result = super::cleanup_hf_cache(&source_path, &dest_path).await;

        // Cleanup should succeed (nothing to clean up)
        assert!(
            result.is_ok(),
            "Cleanup should succeed when source is already gone"
        );
    }
}
