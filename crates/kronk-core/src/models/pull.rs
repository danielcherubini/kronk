use anyhow::{Context, Result};
use hf_hub::api::tokio::Api;
use std::path::PathBuf;

/// Information about a GGUF file in a HuggingFace repo.
#[derive(Debug, Clone)]
pub struct RemoteGguf {
    /// Filename, e.g. "OmniCoder-8B-Q4_K_M.gguf"
    pub filename: String,
    /// Inferred quant type from filename, e.g. "Q4_K_M"
    pub quant: Option<String>,
}

/// List GGUF files available in a HuggingFace model repository.
pub async fn list_gguf_files(repo_id: &str) -> Result<Vec<RemoteGguf>> {
    let api = Api::new().context("Failed to initialise HuggingFace API client")?;
    let repo = api.model(repo_id.to_string());
    let info = repo
        .info()
        .await
        .with_context(|| format!("Failed to fetch repo info for '{}'", repo_id))?;

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

    Ok(ggufs)
}

/// Download a specific GGUF file from a HuggingFace repo to the given model directory.
/// Returns the local path to the downloaded file.
/// Uses hf-hub's built-in caching + progress bar (indicatif).
pub async fn download_gguf(
    repo_id: &str,
    filename: &str,
    dest_dir: &std::path::Path,
) -> Result<PathBuf> {
    let api = Api::new().context("Failed to initialise HuggingFace API client")?;
    let repo = api.model(repo_id.to_string());

    // hf-hub downloads to its own cache with built-in progress
    let cached_path = repo
        .download(filename)
        .await
        .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("Failed to create model directory: {}", dest_dir.display()))?;

    let dest_path = dest_dir.join(filename);

    // Try hard link first (same filesystem = instant, no extra space)
    // Fall back to copy if hard link fails (cross-filesystem)
    if std::fs::hard_link(&cached_path, &dest_path).is_err() {
        std::fs::copy(&cached_path, &dest_path).with_context(|| {
            format!("Failed to copy downloaded file to {}", dest_path.display())
        })?;
    }

    Ok(dest_path)
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
