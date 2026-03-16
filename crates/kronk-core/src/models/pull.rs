use crate::models::card::ModelCard;
use anyhow::{Context, Result};
use hf_hub::api::tokio::Api;
use std::path::PathBuf;
use tokio::sync::OnceCell;

static HF_API: OnceCell<Api> = OnceCell::const_new();

/// Get or create the shared HuggingFace API client.
async fn hf_api() -> Result<&'static Api> {
    HF_API
        .get_or_try_init(|| async {
            Api::new().context("Failed to initialise HuggingFace API client")
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

/// List GGUF files available in a HuggingFace model repository.
/// Returns the resolved repo_id (which may differ from input if `-GGUF` was appended)
/// and the list of available GGUF files.
///
/// Auto-resolves repos: if `repo_id` doesn't end with `-GGUF` and the initial
/// fetch finds no GGUF files (or the repo doesn't exist), retries with `-GGUF` appended.
pub async fn list_gguf_files(repo_id: &str) -> Result<(String, Vec<RemoteGguf>)> {
    let api = hf_api().await?;

    // Try the repo_id as given first
    let candidates = if repo_id.to_uppercase().ends_with("-GGUF") {
        vec![repo_id.to_string()]
    } else {
        vec![repo_id.to_string(), format!("{}-GGUF", repo_id)]
    };

    for candidate in &candidates {
        let repo = api.model(candidate.clone());
        match repo.info().await {
            Ok(info) => {
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
                    return Ok((candidate.clone(), ggufs));
                }
                // Repo exists but no GGUFs — try next candidate
            }
            Err(_) => continue, // Repo not found — try next candidate
        }
    }

    anyhow::bail!("No GGUF files found. Tried: {}", candidates.join(", "))
}

/// Download a specific GGUF file from a HuggingFace repo to the given model directory.
/// Returns the local path to the downloaded file.
/// Uses hf-hub's built-in caching + progress bar (indicatif).
pub async fn download_gguf(
    repo_id: &str,
    filename: &str,
    dest_dir: &std::path::Path,
) -> Result<PathBuf> {
    let api = hf_api().await?;
    let repo = api.model(repo_id.to_string());

    // hf-hub downloads to its own cache with built-in progress
    let cached_path = repo
        .download(filename)
        .await
        .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("Failed to create model directory: {}", dest_dir.display()))?;

    let dest_path = dest_dir.join(filename);

    // Check if destination already exists with matching size (already downloaded)
    if dest_path.exists() {
        if let (Ok(cached_meta), Ok(dest_meta)) = (
            std::fs::metadata(&cached_path),
            std::fs::metadata(&dest_path),
        ) {
            if cached_meta.len() == dest_meta.len() {
                return Ok(dest_path);
            }
        }
        // Size mismatch — remove stale file before re-linking
        std::fs::remove_file(&dest_path).ok();
    }

    // Try hard link first (same filesystem = instant, no extra space)
    // Fall back to copy if hard link fails (cross-filesystem)
    if std::fs::hard_link(&cached_path, &dest_path).is_err() {
        std::fs::copy(&cached_path, &dest_path).with_context(|| {
            format!("Failed to copy downloaded file to {}", dest_path.display())
        })?;
    }

    Ok(dest_path)
}

const MODELCARDS_BASE_URL: &str =
    "https://raw.githubusercontent.com/danielcherubini/kronk/main/modelcards";

/// Try to fetch a community model card from the kronk repository.
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
    let quant_patterns = [
        "IQ2_XXS", "IQ3_XXS", "IQ1_S", "IQ1_M", "IQ2_XS", "IQ2_S", "IQ2_M", "IQ3_XS", "IQ3_S",
        "IQ3_M", "IQ4_XS", "IQ4_NL", "Q2_K_S", "Q3_K_S", "Q3_K_M", "Q3_K_L", "Q4_K_S", "Q4_K_M",
        "Q4_K_L", "Q5_K_S", "Q5_K_M", "Q5_K_L", "Q2_K", "Q3_K", "Q4_K", "Q5_K", "Q6_K", "Q4_0",
        "Q4_1", "Q5_0", "Q5_1", "Q6_0", "Q8_0", "Q8_1", "F16", "F32", "BF16",
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

    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(infer_quant_from_filename("model.gguf"), None);
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
}
