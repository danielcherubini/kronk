use anyhow::{Context, Result};
use serde::Deserialize;

const HF_API_BASE: &str = "https://huggingface.co/api/models";

/// A model search result from HuggingFace.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    /// Repo ID, e.g. "bartowski/Llama-3.2-3B-Instruct-GGUF"
    #[serde(rename = "modelId")]
    pub model_id: String,
    /// Total downloads
    #[serde(default)]
    pub downloads: u64,
    /// Total likes
    #[serde(default)]
    pub likes: u64,
    /// Tags (e.g. ["gguf", "llama", "text-generation"])
    #[serde(default)]
    pub tags: Vec<String>,
    /// Last modified date
    #[serde(rename = "lastModified", default)]
    pub last_modified: Option<String>,
    /// Author/org name
    #[serde(default)]
    pub author: Option<String>,
}

/// Sort order for search results.
#[derive(Debug, Clone, Copy)]
pub enum SortBy {
    Downloads,
    Likes,
    Modified,
}

impl SortBy {
    fn as_str(&self) -> &str {
        match self {
            SortBy::Downloads => "downloads",
            SortBy::Likes => "likes",
            SortBy::Modified => "lastModified",
        }
    }
}

/// Search HuggingFace for GGUF models matching the query.
pub async fn search_models(
    query: &str,
    sort: SortBy,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("Failed to create HTTP client")?;

    // Always filter to GGUF library
    let url = format!(
        "{}?search={}&library=gguf&sort={}&direction=-1&limit={}",
        HF_API_BASE,
        urlencoding(query),
        sort.as_str(),
        limit,
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .with_context(|| format!("Failed to search HuggingFace for '{}'", query))?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "HuggingFace search failed with status {}",
            resp.status()
        );
    }

    let results: Vec<SearchResult> = resp
        .json()
        .await
        .context("Failed to parse search results")?;

    Ok(results)
}

/// URL encode a string for use in a query parameter.
/// Uses percent-encoding for all non-alphanumeric characters except
/// the ones that are safe in query strings: - _ . ~
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "+".to_string(),
            '-' | '_' | '.' | '~' => c.to_string(),
            c if c.is_alphanumeric() => c.to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello+world");
        assert_eq!(urlencoding("foo&bar=baz"), "foo%26bar%3Dbaz");
    }

    // Network test — run with: cargo test -p kronk-core -- search --ignored
    #[tokio::test]
    #[ignore]
    async fn test_search_gguf_models() {
        let results = search_models("llama", SortBy::Downloads, 5).await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].model_id.to_lowercase().contains("llama")
            || results[0].tags.iter().any(|t| t == "gguf"));
    }
}