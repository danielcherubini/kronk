# Parallel Downloads Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Increase GGUF download speed from ~30 MiB/s (single-connection) to near line-speed by using multi-connection HTTP Range downloads.

**Architecture:** Replace the single-connection `hf-hub` download in `pull.rs` with a custom chunked downloader that splits the file into N ranges and downloads them in parallel using `reqwest`. Keeps `hf-hub` for repo listing/info (which works fine) but bypasses it for the actual file download. Uses `indicatif` for a combined progress bar across all chunks. Falls back to single-stream if the server doesn't support Range requests.

**Tech Stack:** Rust, `reqwest` (already a dependency), `tokio` (already a dependency), `indicatif` (already a dependency), `futures` for `join_all`

**Why not `hf-fetch-model`:** Only 102 downloads, young crate (v0.7). The parallel download logic is straightforward enough (~150 lines) that rolling our own avoids a dependency risk and gives us full control over progress reporting, error handling, and integration with our existing `DownloadResult` type.

---

## File Structure

### New files to create

| File | Responsibility |
|------|---------------|
| `crates/kronk-core/src/models/download.rs` | Chunked parallel downloader with progress |

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/models/mod.rs` | Add `pub mod download;` |
| `crates/kronk-core/src/models/pull.rs` | Replace `download_gguf` body to use chunked downloader, keep `DownloadResult` |

---

## Chunk 1: Chunked Downloader

### Task 1: Create the parallel download module

**Files:**
- Create: `crates/kronk-core/src/models/download.rs`

- [ ] **Step 1: Write the `ChunkedDownloader` implementation**

```rust
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const DEFAULT_CONNECTIONS: usize = 8;
const MIN_CHUNK_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB — don't bother chunking smaller

/// Download a file using parallel HTTP Range requests.
/// Falls back to single-stream if Range is not supported.
pub async fn download_chunked(
    url: &str,
    dest: &Path,
    connections: usize,
) -> Result<u64> {
    let client = Client::new();

    // HEAD request to get Content-Length and check Range support
    let head = client
        .head(url)
        .send()
        .await
        .with_context(|| format!("HEAD request failed for {}", url))?;

    let total_size = head
        .content_length()
        .with_context(|| "Server did not return Content-Length")?;

    let accept_ranges = head
        .headers()
        .get("accept-ranges")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("none");

    let use_chunked = accept_ranges != "none" && total_size > MIN_CHUNK_SIZE;
    let num_connections = if use_chunked {
        connections.min((total_size / MIN_CHUNK_SIZE) as usize).max(1)
    } else {
        1
    };

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec})")
            .unwrap()
            .progress_chars("=>-"),
    );

    if num_connections == 1 {
        // Single-stream fallback
        download_single(&client, url, dest, total_size, &pb).await?;
    } else {
        // Parallel chunked download
        download_parallel(&client, url, dest, total_size, num_connections, &pb).await?;
    }

    pb.finish_with_message("done");
    Ok(total_size)
}

async fn download_single(
    client: &Client,
    url: &str,
    dest: &Path,
    _total_size: u64,
    pb: &ProgressBar,
) -> Result<()> {
    use futures_util::StreamExt;

    let resp = client.get(url).send().await?;
    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::File::create(dest).await?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }

    file.flush().await?;
    Ok(())
}

async fn download_parallel(
    client: &Client,
    url: &str,
    dest: &Path,
    total_size: u64,
    num_connections: usize,
    pb: &ProgressBar,
) -> Result<()> {
    use futures_util::StreamExt;

    let chunk_size = total_size / num_connections as u64;

    // Download each chunk to a temp file
    let tmp_dir = dest.parent().unwrap_or(Path::new("."));
    let mut handles = Vec::new();

    for i in 0..num_connections {
        let start = i as u64 * chunk_size;
        let end = if i == num_connections - 1 {
            total_size - 1
        } else {
            (i as u64 + 1) * chunk_size - 1
        };

        let client = client.clone();
        let url = url.to_string();
        let tmp_path = tmp_dir.join(format!(
            ".{}.part{}",
            dest.file_name().unwrap().to_string_lossy(),
            i
        ));
        let pb = pb.clone();

        let handle = tokio::spawn(async move {
            let range = format!("bytes={}-{}", start, end);
            let resp = client
                .get(&url)
                .header("Range", &range)
                .send()
                .await
                .with_context(|| format!("Range request failed for chunk {}", i))?;

            let mut stream = resp.bytes_stream();
            let mut file = tokio::fs::File::create(&tmp_path).await?;

            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                file.write_all(&chunk).await?;
                pb.inc(chunk.len() as u64);
            }

            file.flush().await?;
            Ok::<PathBuf, anyhow::Error>(tmp_path)
        });

        handles.push(handle);
    }

    // Wait for all chunks
    let mut chunk_paths = Vec::new();
    for handle in handles {
        let path = handle.await??;
        chunk_paths.push(path);
    }

    // Reassemble chunks into final file
    let mut dest_file = tokio::fs::File::create(dest).await?;
    for chunk_path in &chunk_paths {
        let chunk_data = tokio::fs::read(chunk_path).await?;
        dest_file.write_all(&chunk_data).await?;
        tokio::fs::remove_file(chunk_path).await.ok();
    }
    dest_file.flush().await?;

    Ok(())
}
```

- [ ] **Step 2: Add `pub mod download;` to `models/mod.rs`**

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles (function is unused at this point)

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/models/download.rs crates/kronk-core/src/models/mod.rs
git commit -m "feat: add chunked parallel downloader with Range requests"
```

### Task 2: Write tests for the downloader

**Files:**
- Modify: `crates/kronk-core/src/models/download.rs`

- [ ] **Step 1: Add integration test using a public URL**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Test with a small public file to verify the download logic works
    #[tokio::test]
    async fn test_download_single_small_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("test.txt");

        // Use a small known file from HuggingFace (a config.json)
        let url = "https://huggingface.co/gpt2/resolve/main/config.json";
        let size = download_chunked(url, &dest, 1).await.unwrap();

        assert!(dest.exists());
        assert!(size > 0);
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), size);
    }
}
```

Note: This test requires network access and may be slow. Mark with `#[ignore]` if needed for CI.

- [ ] **Step 2: Run tests**

Run: `cargo test -p kronk-core -- download --ignored`
Expected: Test passes (downloads a small file)

- [ ] **Step 3: Commit**

```bash
git add crates/kronk-core/src/models/download.rs
git commit -m "test: add integration test for chunked downloader"
```

### Task 3: Wire into `download_gguf`

**Files:**
- Modify: `crates/kronk-core/src/models/pull.rs`

- [ ] **Step 1: Build the HuggingFace download URL from repo_id and filename**

HuggingFace file download URLs follow the pattern:
```
https://huggingface.co/{repo_id}/resolve/main/{filename}
```

- [ ] **Step 2: Replace the hf-hub download + hard_link/copy flow**

In `download_gguf`, replace the body after `create_dir_all` with:

```rust
    let dest_path = dest_dir.join(filename);

    // Check if already downloaded with matching size
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        repo_id, filename
    );

    // Use chunked parallel download
    let size_bytes = crate::models::download::download_chunked(
        &url,
        &dest_path,
        8, // connections
    )
    .await
    .with_context(|| format!("Failed to download '{}' from '{}'", filename, repo_id))?;

    Ok(DownloadResult {
        path: dest_path,
        size_bytes,
    })
```

- [ ] **Step 3: Add existing-file size check before downloading**

Before calling `download_chunked`, check if `dest_path` exists and has the expected size (via a HEAD request or cached metadata). Skip download if sizes match.

- [ ] **Step 4: Keep `hf-hub` for `list_gguf_files` and `fetch_community_card`**

Only the actual file download changes. Repo info/listing still uses `hf-hub`'s `repo.info()` API.

- [ ] **Step 5: Add `futures-util` to kronk-core dependencies if not already present**

In `crates/kronk-core/Cargo.toml`:

```toml
futures-util = "0.3"
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles

- [ ] **Step 7: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat: use parallel chunked downloads for GGUF files (~3x speedup)"
```

---

## Chunk 2: Polish and Edge Cases

### Task 4: Handle edge cases

**Files:**
- Modify: `crates/kronk-core/src/models/download.rs`

- [ ] **Step 1: Add resume support**

If the destination file exists but is smaller than expected, use a Range request starting from the existing file size to resume where we left off. This handles interrupted downloads.

- [ ] **Step 2: Add retry with backoff for individual chunks**

If a chunk download fails, retry up to 3 times with exponential backoff (1s, 2s, 4s) before failing the whole download.

- [ ] **Step 3: Clean up partial files on error**

If any chunk fails after retries, remove all `.partN` temp files and the incomplete destination file.

- [ ] **Step 4: Add HF token support**

Read `HF_TOKEN` environment variable (or `~/.cache/huggingface/token`) and pass as `Authorization: Bearer <token>` header for gated model downloads.

- [ ] **Step 5: Verify and commit**

Run: `cargo test --workspace`
Expected: All tests pass

```bash
git add -A
git commit -m "fix: add resume, retry, cleanup, and HF token support to downloader"
```

### Task 5: Configurable connection count

**Files:**
- Modify: `crates/kronk-core/src/config.rs`
- Modify: `crates/kronk-cli/src/commands/model.rs`

- [ ] **Step 1: Add `download_connections` to General config**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    /// Number of parallel connections for model downloads (default: 8).
    #[serde(default)]
    pub download_connections: Option<usize>,
}
```

- [ ] **Step 2: Pass connection count from config to download_gguf**

- [ ] **Step 3: Add `--connections` flag to `kronk model pull`**

```rust
Pull {
    repo: String,
    /// Number of parallel download connections (default: 8)
    #[arg(long, short = 'j')]
    connections: Option<usize>,
},
```

- [ ] **Step 4: Verify and commit**

Run: `cargo test --workspace`

```bash
git add -A
git commit -m "feat: configurable parallel download connections (default 8)"
```
