# Model Search Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `kronk model search <query>` to search HuggingFace for GGUF models, displaying results with download counts, sizes, and available quants so users can discover models without leaving the terminal.

**Architecture:** Add a `search` module to `kronk-core::models` that queries the HuggingFace REST API (`/api/models`) with `library=gguf` filter. Results are parsed into a `SearchResult` struct. The CLI command displays a formatted table and optionally lets the user pick a result to pull immediately. No new dependencies — uses existing `reqwest` and `serde_json`.

**Tech Stack:** Rust, `reqwest` (existing), `serde`/`serde_json` (existing), HuggingFace REST API

---

## File Structure

### New files to create

| File | Responsibility |
|------|---------------|
| `crates/kronk-core/src/models/search.rs` | HuggingFace model search API client |

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/models/mod.rs` | Add `pub mod search;` |
| `crates/kronk-cli/src/main.rs` | Add `Search` variant to `ModelCommands` |
| `crates/kronk-cli/src/commands/model.rs` | Add `cmd_search` handler |

---

## Chunk 1: Search API Client

### Task 1: Implement the search module

**Files:**
- Create: `crates/kronk-core/src/models/search.rs`
- Modify: `crates/kronk-core/src/models/mod.rs`

- [ ] **Step 1: Write the search types and function**

```rust
// crates/kronk-core/src/models/search.rs
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

/// Simple URL encoding for the search query.
fn urlencoding(s: &str) -> String {
    s.replace(' ', "+")
        .replace('&', "%26")
        .replace('=', "%3D")
        .replace('?', "%3F")
        .replace('#', "%23")
}
```

- [ ] **Step 2: Add `pub mod search;` to `models/mod.rs`**

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles

- [ ] **Step 4: Write a test**

```rust
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
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p kronk-core -- search`
Expected: `test_urlencoding` passes. Network test passes if run with `--ignored`.

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-core/src/models/search.rs crates/kronk-core/src/models/mod.rs
git commit -m "feat: add HuggingFace GGUF model search API client"
```

---

## Chunk 2: CLI Command

### Task 2: Add `Search` subcommand to CLI

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`
- Modify: `crates/kronk-cli/src/commands/model.rs`

- [ ] **Step 1: Add `Search` variant to `ModelCommands`**

In `main.rs`, add to the `ModelCommands` enum:

```rust
    /// Search HuggingFace for GGUF models
    Search {
        /// Search query (e.g. "llama", "coding", "mistral 7b")
        query: String,
        /// Sort by: downloads, likes, modified (default: downloads)
        #[arg(long, default_value = "downloads")]
        sort: String,
        /// Maximum number of results (default: 20)
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,
        /// Immediately pull a selected result
        #[arg(long)]
        pull: bool,
    },
```

- [ ] **Step 2: Add the match arm in `model.rs`**

```rust
ModelCommands::Search { query, sort, limit, pull } => {
    cmd_search(config, &query, &sort, limit, pull).await
}
```

- [ ] **Step 3: Implement `cmd_search`**

```rust
async fn cmd_search(
    config: &Config,
    query: &str,
    sort: &str,
    limit: usize,
    pull: bool,
) -> Result<()> {
    use kronk_core::models::search::{self, SortBy};

    let sort_by = match sort {
        "likes" => SortBy::Likes,
        "modified" => SortBy::Modified,
        _ => SortBy::Downloads,
    };

    println!("  Searching HuggingFace for GGUF models: \"{}\"...", query);
    println!();

    let results = search::search_models(query, sort_by, limit).await?;

    if results.is_empty() {
        println!("  No GGUF models found for \"{}\".", query);
        return Ok(());
    }

    // Display results as a formatted table
    println!(
        "  {:<50} {:>12} {:>8}",
        "MODEL", "DOWNLOADS", "LIKES"
    );
    println!("  {}", "-".repeat(74));

    for (i, result) in results.iter().enumerate() {
        let id = if result.model_id.len() > 48 {
            format!("{}...", &result.model_id[..45])
        } else {
            result.model_id.clone()
        };
        println!(
            "  {:<50} {:>12} {:>8}",
            id,
            format_downloads(result.downloads),
            result.likes,
        );
    }

    println!();

    if pull {
        // Let user pick a result to pull
        let options: Vec<String> = results.iter().map(|r| r.model_id.clone()).collect();
        let selected = inquire::Select::new("Pull which model?", options)
            .prompt()
            .context("Selection cancelled")?;

        // Delegate to cmd_pull
        cmd_pull(config, &selected).await?;
    } else {
        println!("  Pull one:  kronk model pull <model-id>");
    }

    Ok(())
}

fn format_downloads(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles

- [ ] **Step 5: Manual test**

Run: `cargo run -p kronk -- model search "llama 8b"`
Expected: Table of GGUF models with download counts

Run: `cargo run -p kronk -- model search "coding" --sort likes -n 5`
Expected: Top 5 GGUF coding models by likes

Run: `cargo run -p kronk -- model search "mistral" --pull`
Expected: Shows results, then picker to pull directly

- [ ] **Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 7: Commit**

```bash
git add crates/kronk-cli/src/main.rs crates/kronk-cli/src/commands/model.rs
git commit -m "feat: add 'kronk model search' to discover GGUF models from HuggingFace"
```
