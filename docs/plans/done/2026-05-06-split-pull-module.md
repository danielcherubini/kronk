# Split `models/pull.rs` Into Focused Submodules

**Goal:** Split the 1,693-line `crates/tama-core/src/models/pull.rs` into 5 focused modules under a `pull/` directory.

**Architecture:** Convert `pull.rs` into a `pull/` module directory with `mod.rs` (types + re-exports + singleton), and 4 sub-modules: `api.rs` (HF API calls), `download.rs` (download logic), `metadata.rs` (README parsing + community cards), `quant.rs` (quant inference).

**Tech Stack:** Rust, existing `hf-hub`, `reqwest`, `tokio` dependencies (no new deps).

---

## Context

The file `crates/tama-core/src/models/pull.rs` is 1,693 lines and mixes 4 distinct concerns:
1. **HF API interaction** — listing files, fetching blob/metadata, pipeline tags
2. **Download logic** — hf-hub downloads, chunked downloads, cache cleanup
3. **Metadata parsing** — README markdown parsing, community card fetching
4. **Quant inference** — pattern-based quant type detection from filenames

External consumers import via `crate::models::pull::X` paths. The split must preserve all public APIs through `pub use` re-exports so no consumer code needs changing.

### External Consumers (must not break)
- `tama-cli/src/commands/model/update.rs` — `list_gguf_files`, `fetch_blob_metadata`, `download_gguf`
- `tama-core/src/models/mod.rs` — re-exports `infer_quant_from_filename`
- `tama-core/src/models/update.rs` — `pull::`, `BlobInfo`, `HfModelMetadata`, `list_gguf_files`, `fetch_blob_metadata`, `fetch_hf_metadata`
- `tama-core/src/proxy/tama_handlers/mod.rs` — re-exports from `pull`
- `tama-core/src/proxy/tama_handlers/system.rs` — `fetch_blob_metadata`, `infer_quant_from_filename`
- `tama-core/src/db/backfill.rs` — `list_gguf_files`, `fetch_blob_metadata`, `infer_quant_from_filename`, `HfModelMetadata`, `fetch_hf_metadata`
- `tama-core/src/updates/checker.rs` — `pull::`, `BlobInfo`, `RemoteGguf`, `RepoGgufListing`, `list_gguf_files`

---

### Task 1: Create `pull/` module structure with `mod.rs`

**Context:** Replace the single `pull.rs` file with a `pull/` directory. The `mod.rs` holds shared types, the HF API singleton, and re-exports from sub-modules. This is the foundation — all other tasks depend on this module existing.

**Files:**
- Create: `crates/tama-core/src/models/pull/mod.rs`
- Create: `crates/tama-core/src/models/pull/api.rs` (placeholder)
- Create: `crates/tama-core/src/models/pull/download.rs` (placeholder)
- Create: `crates/tama-core/src/models/pull/metadata.rs` (placeholder)
- Create: `crates/tama-core/src/models/pull/quant.rs` (placeholder)
- Verify: `crates/tama-core/src/models/mod.rs` — no change needed, `pub mod pull;` auto-resolves to `pull/mod.rs`
- Delete: `crates/tama-core/src/models/pull.rs` (after all code is moved)

**What to implement:**

1. Create `crates/tama-core/src/models/pull/` directory.

2. Create `pull/mod.rs` with:
   - Module declarations: `pub mod api; pub mod download; pub mod metadata; pub mod quant;`
   - The `HF_API` static + `hf_api()` function (move from pull.rs):
     ```rust
     use tokio::sync::OnceCell;
     use anyhow::Context;
     static HF_API: OnceCell<hf_hub::api::tokio::Api> = OnceCell::const_new();
     
     pub(crate) async fn hf_api() -> anyhow::Result<&'static hf_hub::api::tokio::Api> {
         HF_API.get_or_try_init(|| async {
             hf_hub::api::tokio::ApiBuilder::new()
                 .with_max_files(8)
                 .build()
                 .context("Failed to initialise HuggingFace API client")
         }).await
     }
     ```
   - All shared types (move from pull.rs, keep exact definitions):
     - `RemoteGguf` (pub struct with `filename: String`, `quant: Option<String>`)
     - `RepoGgufListing` (pub struct with `repo_id: String`, `commit_sha: String`, `files: Vec<RemoteGguf>`)
     - `BlobInfo` (pub struct with `filename: String`, `blob_id: Option<String>`, `size: Option<i64>`, `lfs_sha256: Option<String>`)
     - `HfModelMetadata` (pub struct with all hf_* fields)
   - Re-exports from sub-modules:
     ```rust
     pub use api::{list_gguf_files, fetch_blob_metadata, fetch_hf_metadata, fetch_model_pipeline_tag, parse_blob_siblings, infer_modalities_from_pipeline};
     pub use download::{download_gguf, download_gguf_with_progress, cleanup_hf_cache, DownloadResult, ProgressAdapter};
     pub use metadata::{parse_readme_metadata, fetch_community_card};
     pub use quant::infer_quant_from_filename;
     ```

3. Create placeholder files for each sub-module (just `// placeholder` for now).

4. Do NOT run `cargo check` yet — the re-exports will fail because sub-modules are empty placeholders. This is expected.

5. Do NOT delete the old `pull.rs` yet — wait until all code is moved.

**Steps:**
- [ ] Create `crates/tama-core/src/models/pull/` directory
- [ ] Create `pull/mod.rs` with module decls, HF_API singleton, shared types, and re-exports
- [ ] Create placeholder `api.rs`, `download.rs`, `metadata.rs`, `quant.rs`
- [ ] Do NOT run `cargo check` — re-exports won't resolve until sub-modules are populated in subsequent tasks. This is expected.

**Acceptance criteria:**
- [ ] `pull/` directory exists with `mod.rs` and 4 sub-module files
- [ ] `mod.rs` contains types, singleton, module declarations, and re-exports
- [ ] Re-exports are declared (they'll resolve as sub-modules are populated)

---

### Task 2: Move HF API functions to `pull/api.rs`

**Context:** Extract all HuggingFace API interaction code into a focused module. This includes file listing, blob metadata, full metadata fetching, pipeline tag fetching, and the pure JSON parsing helper.

**Files:**
- Modify: `crates/tama-core/src/models/pull/api.rs`
- Modify: `crates/tama-core/src/models/pull.rs` (remove moved code)

**What to implement:**

Move these items from `pull.rs` into `pull/api.rs`:
- `list_gguf_files(repo_id: &str) -> Result<RepoGgufListing>` — GGUF file listing with `-GGUF` fallback
- `fetch_blob_metadata(repo_id: &str) -> Result<HashMap<String, BlobInfo>>` — blobs API call
- `fetch_hf_metadata(repo_id: &str) -> Result<HfModelMetadata>` — full metadata + README fetch
- `fetch_model_pipeline_tag(repo_id: &str) -> Result<Option<String>>` — pipeline tag only
- `parse_blob_siblings(value: &serde_json::Value) -> HashMap<String, BlobInfo>` — pure JSON parsing
- `infer_modalities_from_pipeline(pipeline_tag: Option<&str>) -> Option<ModelModalities>` — pipeline tag → modalities

Add required imports at top of `api.rs`:
```rust
use std::collections::HashMap;
use anyhow::{Context, Result};
use serde_json;
use crate::models::pull::{hf_api, BlobInfo, HfModelMetadata, RemoteGguf, RepoGgufListing};
use crate::models::pull::quant::infer_quant_from_filename;
use crate::models::pull::metadata::parse_readme_metadata;
use crate::config::ModelModalities;
```

After moving, remove these functions from the old `pull.rs`.

Also move the blob-siblings tests from the `#[cfg(test)] mod tests` block:
- `test_parse_blob_siblings_basic`
- `test_parse_blob_siblings_no_lfs`
- `test_parse_blob_siblings_empty`
- `test_parse_blob_siblings_no_siblings_key`

After moving tests, replace `use super::*;` with explicit imports or verify it still resolves correctly.

**Steps:**
- [ ] Copy the 6 functions from `pull.rs` into `api.rs`
- [ ] Add required imports to `api.rs`
- [ ] Remove the 6 functions from `pull.rs`
- [ ] Run `cargo check --package tama-core` — fix any import/path errors
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package tama-core` — verify no test regressions

**Acceptance criteria:**
- [ ] `api.rs` contains exactly the 6 listed functions
- [ ] `cargo check --package tama-core` passes
- [ ] All tests pass

---

### Task 3: Move download functions to `pull/download.rs`

**Context:** Extract all download-related code — hf-hub downloads, reqwest chunked downloads, and post-download cache cleanup.

**Files:**
- Modify: `crates/tama-core/src/models/pull/download.rs`
- Modify: `crates/tama-core/src/models/pull.rs` (remove moved code)

**What to implement:**

Move these items from `pull.rs` into `pull/download.rs`:
- `DownloadResult` struct (pub, with `path: PathBuf`, `size_bytes: u64`)
- `ProgressAdapter` struct + `impl ProgressAdapter` + `impl hf_hub::api::tokio::Progress for ProgressAdapter`
- `download_gguf_with_progress(repo_id, filename, dest_dir, callback) -> Result<DownloadResult>`
- `download_gguf(client, repo_id, filename, dest_dir) -> Result<DownloadResult>` (marked `#[allow(dead_code)]`)
- `cleanup_hf_cache(source_path, dest_path) -> Result<()>`

Add required imports at top of `download.rs`:
```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use anyhow::{Context, Result};
use reqwest;
use crate::models::pull::hf_api;
use crate::models::download::ProgressCallback;
```

Also move the download-related tests from the `#[cfg(test)] mod tests` block:
- `test_cleanup_hf_cache_success`
- `test_cleanup_hf_cache_dest_missing`
- `test_cleanup_hf_cache_source_missing`
- `test_cleanup_hf_cache_size_mismatch`
- Any other `test_cleanup_*` or `test_download_*` tests

After moving tests, replace `use super::*;` with explicit imports or verify it still resolves correctly.

After moving, remove these items from the old `pull.rs`.

**Steps:**
- [ ] Copy the structs, impls, and functions from `pull.rs` into `download.rs`
- [ ] Add required imports to `download.rs`
- [ ] Remove the items from `pull.rs`
- [ ] Run `cargo check --package tama-core` — fix any import/path errors
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package tama-core` — verify no test regressions

**Acceptance criteria:**
- [ ] `download.rs` contains DownloadResult, ProgressAdapter, download_gguf*, cleanup_hf_cache
- [ ] `cargo check --package tama-core` passes
- [ ] All tests pass

---

### Task 4: Move metadata parsing to `pull/metadata.rs`

**Context:** Extract README parsing, community card fetching, and all helper parsing functions.

**Files:**
- Modify: `crates/tama-core/src/models/pull/metadata.rs`
- Modify: `crates/tama-core/src/models/pull.rs` (remove moved code)

**What to implement:**

Move these items from `pull.rs` into `pull/metadata.rs`:
- `parse_readme_metadata(markdown: &str) -> HfModelMetadata` — main README parser
- `extract_param_value(markdown: &str, labels: &[&str]) -> Option<String>` — helper
- `extract_first_param_shorthand(s: &str) -> Option<String>` — helper
- `parse_param_shorthand(s: &str) -> Option<String>` — helper
- `parse_context_length(s: &str) -> Option<u32>` — helper
- `parse_u32(s: &str) -> Option<u32>` — helper
- `fetch_community_card(repo_id: &str) -> Option<ModelCard>` — community card fetch
- `MODELCARDS_BASE_URL` constant

Add required imports:
```rust
use std::time::Duration;
use anyhow::Result;
use reqwest;
use toml;
use crate::models::pull::HfModelMetadata;
use crate::models::card::ModelCard;
```

Also move the README parsing tests from the `#[cfg(test)] mod tests` block:
- `test_parse_readme_moe_model`
- `test_parse_readme_dense_model`
- `test_parse_readme_mamba_model`
- `test_parse_readme_context_k_tokens`
- `test_parse_readme_context_comma`
- `test_parse_readme_table_style`
- `test_parse_readme_empty`
- `test_parse_readme_activated_pattern`
- `test_parse_readme_context_m_suffix`
- Any other `test_parse_readme_*` tests

After moving tests, replace `use super::*;` with explicit imports or verify it still resolves correctly.

After moving, remove these items from the old `pull.rs`.

**Steps:**
- [ ] Copy the functions and constant from `pull.rs` into `metadata.rs`
- [ ] Add required imports to `metadata.rs`
- [ ] Remove the items from `pull.rs`
- [ ] Run `cargo check --package tama-core` — fix any import/path errors
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo test --package tama-core` — verify no test regressions

**Acceptance criteria:**
- [ ] `metadata.rs` contains parse_readme_metadata, helpers, fetch_community_card
- [ ] `cargo check --package tama-core` passes
- [ ] All tests pass

---

### Task 5: Move quant inference to `pull/quant.rs` and delete old `pull.rs`

**Context:** Move the quant inference function (with its large pattern array) and all quant-related tests. Then delete the old `pull.rs` since all code has been moved out.

**Files:**
- Modify: `crates/tama-core/src/models/pull/quant.rs`
- Modify: `crates/tama-core/src/models/pull.rs` (remove moved code, then delete file)

**What to implement:**

Move these items from `pull.rs` into `pull/quant.rs`:
- `infer_quant_from_filename(filename: &str) -> Option<String>` — the main function with the quant_patterns array

Move the `#[cfg(test)] mod tests` block from `pull.rs` — but ONLY the quant-related tests:
- `test_infer_quant_q4_k_m`
- `test_infer_quant_q8_0`
- `test_infer_quant_non_standard_name`
- `test_infer_quant_with_underscore`
- `test_infer_quant_lowercase`
- `test_infer_quant_f16`
- `test_infer_quant_none`
- `test_infer_quant_dot_separator`
- `test_infer_quant_iq`
- `test_infer_quant_xl`
- `test_infer_quant_xl_lowercase`
- `test_infer_quant_ud`
- `test_infer_quant_apex_patterns`
- `test_infer_quant_apex_semantic`
- `test_infer_quant_ud_semantic`
- `test_infer_quant_semantic_without_prefix`
- Any other `test_infer_quant_*` tests

After moving tests, replace `use super::*;` with explicit imports or verify it still resolves correctly.

The blob parsing tests (`test_parse_blob_siblings_*`) should move into `api.rs` instead.

Add required imports to `quant.rs`:
```rust
// No external imports needed — it's a pure function
```

After moving:
1. Check if `pull.rs` has any remaining code beyond imports. If it's empty or just imports, delete it.
2. The `pull/mod.rs` file is the module entry point — it stays.

**Steps:**
- [ ] Copy `infer_quant_from_filename` into `quant.rs`
- [ ] Move quant-related tests into `quant.rs` under `#[cfg(test)] mod tests`
- [ ] Move blob-siblings tests into `api.rs` under `#[cfg(test)] mod tests`
- [ ] Remove all moved code from `pull.rs`
- [ ] Delete `crates/tama-core/src/models/pull.rs` (the old file)
- [ ] Run `cargo check --package tama-core` — fix any remaining import errors
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo test --package tama-core` — verify ALL tests pass

**Acceptance criteria:**
- [ ] `quant.rs` contains `infer_quant_from_filename` + quant tests
- [ ] `api.rs` has blob-siblings tests
- [ ] Verify `pull.rs` has zero remaining code before deletion
- [ ] Old `pull.rs` is deleted
- [ ] `cargo clippy --package tama-core -- -D warnings` passes
- [ ] `cargo test --package tama-core` passes (all tests)
- [ ] All external consumer paths still work (verify by `cargo check --workspace`)

---

### Task 6: Final verification — workspace build, clippy, and tests

**Context:** After the split, verify the entire workspace builds, lints, and tests correctly. This catches any consumer code that might have broken.

**Files:**
- No file changes expected

**Steps:**
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Verify file sizes — no sub-module file should exceed 500 lines (the original was 1,693)

**Acceptance criteria:**
- [ ] `cargo build --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No file exceeds 500 lines

---

## Verification

After all tasks are complete:
```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

All external import paths must still work:
- `crate::models::pull::list_gguf_files`
- `crate::models::pull::fetch_blob_metadata`
- `crate::models::pull::BlobInfo`
- `crate::models::pull::infer_quant_from_filename`
- `crate::models::pull::HfModelMetadata`
- `crate::models::infer_quant_from_filename` (re-export from `models/mod.rs`)
