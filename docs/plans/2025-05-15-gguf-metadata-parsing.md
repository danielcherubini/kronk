# GGUF Metadata Parsing + Context Step Redesign

**Goal:** Parse GGUF files for authoritative metadata after download, replace per-quant context length with a single model-level setting, and combine context + KV cache config in the pull wizard.

**Architecture:** Wizard flow reorders to `SelectQuants → Downloading → SetContext → Done`. After downloads complete, the first GGUF file is parsed (header-only, ~100KB read from a multi-GB file) to extract context_length, architecture, layers, etc. The SetContext step appears pre-filled from parsed values. User can override context length and set KV cache quantization. On save, a PATCH call updates the model config (already auto-created by setup_model_after_pull).

**Tech Stack:** Rust `gguf-parser` crate (Defilan), Leptos 0.7, SQLite, axum

---

## Pre-work: gguf-parser crate (VERIFIED)

After verification: `gguf-parser` is available as a git dependency from `https://github.com/Defilan/gguf-parser`.
- **License:** MIT ✓
- **API:** `GgufFile::parse(&mut reader)` — matches our usage ✓
- **Not on crates.io** — must use git dependency

Add to `Cargo.toml`:
```toml
gguf-parser = { git = "https://github.com/Defilan/gguf-parser", branch = "main" }
```

---

### Task 1: GGUF parsing module

**Context:**
Tama currently extracts model metadata from HuggingFace README markdown (fragile regex) and community TOML cards. GGUF files contain authoritative metadata in the file header that can be parsed in milliseconds without loading tensor data. This task creates a new module in `tama-core` that wraps the `gguf-parser` crate and exposes a simple API for extracting the metadata fields Tama needs.

**Files:**
- Modify: `crates/tama-core/Cargo.toml` — add `gguf-parser` dependency
- Create: `crates/tama-core/src/models/gguf.rs` — new GGUF parsing module
- Modify: `crates/tama-core/src/models/mod.rs` — add `pub mod gguf;`
- Modify: `crates/tama-core/src/models/pull/mod.rs` — add `GgufMetadata` re-export

**What to implement:**

1. In `crates/tama-core/Cargo.toml`, add `gguf-parser` to `[dependencies]`. Use git dependency from `https://github.com/Defilan/gguf-parser` if not on crates.io.

2. Create `crates/tama-core/src/models/gguf.rs` with:

```rust
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Metadata extracted from a GGUF file header.
/// Only reads the header (~100KB), never loads tensor data.
#[derive(Debug, Clone, Default)]
pub struct GgufMetadata {
    pub architecture: Option<String>,       // general.architecture (e.g. "llama")
    pub context_length: Option<u64>,        // {arch}.context_length
    pub embedding_length: Option<u64>,      // {arch}.embedding_length
    pub block_count: Option<u64>,           // {arch}.block_count
    pub head_count: Option<u64>,            // {arch}.attention.head_count
    pub quantization: Option<String>,       // from file_type mapping (e.g. "Q4_K_M")
    pub name: Option<String>,               // general.name
}

/// Parse GGUF metadata from a file on disk.
///
/// Returns `Err` only if the file cannot be read or is not a valid GGUF file.
/// Individual missing metadata keys are handled gracefully (fields are `None`).
pub fn parse_gguf_metadata(path: &Path) -> Result<GgufMetadata> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open GGUF file: {}", path.display()))?;
    let mut reader = BufReader::new(file);

    let gguf = gguf_parser::GgufFile::parse(&mut reader)
        .with_context(|| format!("Failed to parse GGUF header: {}", path.display()))?;

    Ok(GgufMetadata {
        architecture: gguf.architecture().map(|s| s.to_string()),
        context_length: gguf.context_length(),
        embedding_length: gguf.embedding_length(),
        block_count: gguf.block_count(),
        head_count: gguf.head_count(),
        quantization: gguf.quantization_name().map(|s| s.to_string()),
        name: gguf.name().map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_invalid_path() {
        let result = parse_gguf_metadata(Path::new("/nonexistent/file.gguf"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_non_gguf_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "this is not a GGUF file").unwrap();
        let result = parse_gguf_metadata(tmp.path());
        assert!(result.is_err());
    }
}
```

3. Add `pub mod gguf;` to `crates/tama-core/src/models/mod.rs`.

4. In `crates/tama-core/src/models/pull/mod.rs`, add to the re-exports section:
```rust
pub use super::gguf::GgufMetadata;
```

**Steps:**
- [ ] Add `gguf-parser` dependency to `crates/tama-core/Cargo.toml`
- [ ] Create `crates/tama-core/src/models/gguf.rs` with the code above
- [ ] Add `pub mod gguf;` to `crates/tama-core/src/models/mod.rs`
- [ ] Add `GgufMetadata` re-export to `crates/tama-core/src/models/pull/mod.rs`
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix dependency issues and re-run.
- [ ] Run `cargo test --package tama-core gguf::tests`
  - Did both tests pass (invalid path + non-gguf file)? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add GGUF header parsing module"

**Acceptance criteria:**
- [ ] `parse_gguf_metadata(path)` returns `Result<GgufMetadata>` for any valid GGUF file
- [ ] Invalid/non-GGUF files return `Err` with context
- [ ] `GgufMetadata` is re-exported from `tama_core::models::pull`
- [ ] All tests pass, clippy clean

---

### Task 2: Integrate GGUF parsing into download pipeline

**Context:**
After a GGUF file is downloaded and verified (SHA256 hash matches), we need to parse its header to extract metadata. This metadata is used to auto-populate the model config (`setup_model_after_pull`) and streamed to the frontend wizard via SSE for the SetContext step. The parsing must be a soft failure — if it fails, the download still succeeds, we just don't get auto-detected metadata.

**Files:**
- Modify: `crates/tama-core/src/proxy/pull_jobs.rs` — add `gguf_metadata` field to `PullJob`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs` — parse GGUF after verification
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs` — update `QuantDownloadSpec` (keep `context_length` but document it's now from GGUF, not request)

**What to implement:**

1. In `crates/tama-core/src/proxy/pull_jobs.rs`, add to `PullJob`:
```rust
    /// Full GGUF metadata parsed after download verification. `None` if parsing failed
    /// or the file is not a GGUF (e.g. mmproj). Not serialized to SSE.
    #[serde(skip)]
    pub gguf_metadata: Option<crate::models::pull::GgufMetadata>,
    /// GGUF-parsed context length, serialized to SSE events for the wizard.
    /// This is the only field from GgufMetadata that the frontend needs inline.
    #[serde(default)]
    pub gguf_context_length: Option<u64>,
```
Also update the `Default` impl to include `gguf_metadata: None` and `gguf_context_length: None`.

2. In `crates/tama-core/src/proxy/tama_handlers/pull/download.rs`, in `start_download_from_queue`, after `run_verification` returns with `outcome.passed == true` and BEFORE `setup_model_after_pull`:

```rust
// Parse GGUF metadata (soft failure — don't fail the download)
let gguf_metadata = if outcome.passed {
    match crate::models::gguf::parse_gguf_metadata(&dest_path) {
        Ok(meta) => {
            tracing::info!(
                job_id = %job_id_clone,
                architecture = ?meta.architecture,
                context_length = ?meta.context_length,
                "GGUF metadata parsed"
            );
            Some(meta)
        }
        Err(e) => {
            tracing::warn!(
                job_id = %job_id_clone,
                error = %e,
                "GGUF metadata parsing failed — using defaults"
            );
            None
        }
    }
} else {
    None
};

// Store GGUF metadata in PullJob for SSE streaming
{
    let mut jobs = pull_jobs_arc.write().await;
    if let Some(job) = jobs.get_mut(&job_id_clone) {
        job.gguf_metadata = gguf_metadata.clone();
        // Also set the serialized field for SSE events (frontend reads this)
        job.gguf_context_length = gguf_metadata.as_ref().and_then(|m| m.context_length);
    }
}
```

3. In `setup_model_after_pull` (called after the above), the existing `spec.context_length` is still used. We need to modify `_setup_model_after_pull_with_config` to accept an optional `GgufMetadata` parameter and use it as the default context_length.

Add a new parameter to `_setup_model_after_pull_with_config`:
```rust
pub(crate) async fn _setup_model_after_pull_with_config(
    configs_dir: &std::path::Path,
    model_configs: &mut std::collections::HashMap<String, crate::config::ModelConfig>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
    gguf_metadata: Option<&GgufMetadata>,  // NEW
) -> Option<String> {
```

In the model config creation, use GGUF context_length as default:
```rust
// Determine context_length: GGUF parsed value > spec value > None
let context_length = gguf_metadata
    .and_then(|m| m.context_length.map(|v| v as u32))
    .or(spec.context_length);
```

Then use `context_length` instead of `spec.context_length` throughout the function.

Also update the `setup_model_after_pull` wrapper to pass the gguf_metadata:
```rust
pub(crate) async fn setup_model_after_pull(
    state: Arc<ProxyState>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
    gguf_metadata: Option<GgufMetadata>,  // NEW
) -> Option<i64> {
```

Update the call site in `start_download_from_queue` to pass `gguf_metadata`.

4. Also populate `hf_*` informational fields from GGUF metadata in `setup_model_after_pull`:
```rust
if let Some(ref meta) = gguf_metadata {
    entry.hf_architecture_type = meta.architecture.clone();
    entry.hf_context_length = meta.context_length.map(|v| v as u32);
    entry.hf_num_layers = meta.block_count.map(|v| v as u32);
}
```

**Steps:**
- [ ] Add `gguf_metadata` field to `PullJob` struct in `pull_jobs.rs`
- [ ] Update `PullJob::default()` to include `gguf_metadata: None`
- [ ] Add GGUF parsing after `run_verification` in `start_download_from_queue`
- [ ] Store parsed metadata in `PullJob` for SSE streaming
- [ ] Add `gguf_metadata` parameter to `_setup_model_after_pull_with_config` and `setup_model_after_pull`
- [ ] Use GGUF context_length as default in model config creation
- [ ] Populate `hf_architecture_type`, `hf_context_length`, `hf_num_layers` from GGUF
- [ ] Run `cargo build --package tama-core`
- [ ] Run `cargo test --package tama-core`
  - All existing tests must still pass. If any fail, fix the integration points.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: parse GGUF metadata after download verification"

**Acceptance criteria:**
- [ ] After successful download + verification, GGUF metadata is parsed and stored in `PullJob`
- [ ] Parse failure is a soft error (logged as warning, download still succeeds)
- [ ] Model config uses GGUF context_length as default
- [ ] `hf_architecture_type`, `hf_context_length`, `hf_num_layers` populated from GGUF
- [ ] All existing tests pass

---

### Task 3: Update API types — remove per-quant context_length

**Context:**
The current `QuantRequest` (frontend) and `QuantDownloadSpec` (backend) both carry `context_length` per quant. Since context length is a model-level property (same for all quants), this is redundant. The frontend `PullRequest` sends per-quant context values that are no longer needed — the backend gets context from GGUF parsing instead.

**Files:**
- Modify: `crates/tama-web/src/components/pull_wizard/mod.rs` — update `PullRequest`, `QuantRequest`
- Modify: `crates/tama-core/src/proxy/tama_handlers/types.rs` — update `QuantDownloadSpec`, `PullRequest`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs` — update handler to not expect context_length
- Modify: `crates/tama-web/src/components/pull_wizard/mod.rs` — update `CompletedQuant`

**What to implement:**

1. In `crates/tama-web/src/components/pull_wizard/mod.rs`:

Update `PullRequest`:
```rust
#[derive(Serialize)]
pub struct PullRequest {
    pub repo_id: String,
    pub filenames: Vec<String>,
    pub mmproj_filenames: Vec<String>,
}
```

Remove `QuantRequest` entirely (no longer needed — just filenames).

Update `CompletedQuant`:
```rust
#[derive(Clone, Debug)]
pub struct CompletedQuant {
    pub repo_id: String,
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<u64>,
    // context_length removed — it's model-level now
}
```

2. In `crates/tama-core/src/proxy/tama_handlers/types.rs`:

Update `QuantDownloadSpec`:
```rust
#[derive(Debug, Deserialize, Clone)]
pub struct QuantDownloadSpec {
    pub filename: String,
    pub quant: Option<String>,
    // context_length removed from request — populated from GGUF parsing
    #[serde(default)]
    pub context_length: Option<u32>,  // kept for backward compat with DB queue, always None from new requests
}
```

Update `PullRequest`:
```rust
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo_id: String,
    /// Legacy single-quant support (kept for backward compat).
    #[serde(default)]
    pub quant: Option<String>,
    /// New multi-quant wizard format: list of quants to download.
    #[serde(default)]
    pub quants: Vec<QuantDownloadSpec>,
    /// New simplified format: just filenames
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Vision projector files
    #[serde(default)]
    pub mmproj_filenames: Vec<String>,
    #[serde(default)]
    pub context_length: Option<u32>,
}
```

3. In `crates/tama-core/src/proxy/tama_handlers/pull/handlers.rs`, update `handle_tama_pull_model`:

Handle the new `filenames` + `mmproj_filenames` format. When `filenames` is non-empty, use it instead of `quants`:
```rust
// New simplified format: filenames + mmproj_filenames
if !request.filenames.is_empty() || !request.mmproj_filenames.is_empty() {
    let all_files: Vec<_> = request.filenames.iter()
        .chain(request.mmproj_filenames.iter())
        .cloned()
        .collect();
    // Validate filenames against HF listing...
    // Create QuantDownloadSpec with context_length = None (will be filled from GGUF)
    for filename in &all_files {
        let spec = QuantDownloadSpec {
            filename: filename.clone(),
            quant: infer_quant_from_filename(filename),
            context_length: None,  // will be filled from GGUF parsing
        };
        // ... create job, enqueue, etc.
    }
}
```

Keep the existing `quants` path for backward compatibility.

4. Update the `on_complete` callback in `pull_quant_wizard.rs` to not include `context_length` in `CompletedQuant`.

**Steps:**
- [ ] Update `PullRequest` and remove `QuantRequest` in `pull_wizard/mod.rs`
- [ ] Update `CompletedQuant` to remove `context_length`
- [ ] Update `QuantDownloadSpec` and `PullRequest` in `types.rs`
- [ ] Update `handle_tama_pull_model` to handle new `filenames` format
- [ ] Update `pull_quant_wizard.rs` on_complete callback
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: remove per-quant context_length from API types"

**Acceptance criteria:**
- [ ] `PullRequest` accepts `filenames` + `mmproj_filenames` (no per-quant context)
- [ ] `QuantDownloadSpec.context_length` defaults to `None` (filled from GGUF)
- [ ] Legacy `quants` path still works for backward compat
- [ ] All tests pass, including existing integration tests

---

### Task 4: Rewrite SetContext step — single context + KV cache

**Context:**
The current SetContext step shows a table with per-quant context length dropdowns. The new design shows a single context length field for the whole model (pre-filled from GGUF parsing) and KV cache quantization settings. mmproj files are excluded from this step entirely.

**Files:**
- Modify: `crates/tama-web/src/components/pull_wizard/components/context_step.rs` — complete rewrite
- Modify: `crates/tama-web/src/components/pull_wizard/mod.rs` — add new types for the step

**What to implement:**

1. In `crates/tama-web/src/components/pull_wizard/mod.rs`, add new types:

```rust
/// Settings configured in the SetContext step.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ContextSettings {
    pub context_length: Option<u32>,
    pub kv_unified: bool,
    pub cache_type_k: Option<String>,
    pub cache_type_v: Option<String>,
}

/// KV quantization options for the dropdown.
pub const KV_QUANT_OPTIONS: &[&str] = &[
    "f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1",
];
```

2. Rewrite `context_step.rs`:

```rust
use crate::components::context_length_selector::ContextLengthSelector;
use crate::components::pull_wizard::*;
use leptos::prelude::*;

#[component]
pub fn ContextStep(
    /// GGUF-parsed context length (native max for the model).
    gguf_context_length: Signal<Option<u64>>,
    /// Downloaded quant files (model quants only, no mmproj).
    download_jobs: Signal<Vec<JobProgress>>,
    /// The settings the user configures.
    settings: RwSignal<ContextSettings>,
    on_next: Callback<()>,
    on_back: Callback<()>,
) -> impl IntoView {
    // Pre-fill context_length from GGUF if not already set
    Effect::new(move |_| {
        if settings.get().context_length.is_none() {
            if let Some(gguf_ctx) = gguf_context_length.get() {
                settings.update(|s| {
                    s.context_length = Some(gguf_ctx as u32);
                });
            }
        }
    });

    // Max context for the dropdown (capped at GGUF native value)
    let max_context = Signal::derive(move || {
        gguf_context_length.get().unwrap_or(262144) as u32
    });

    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Configure Model"</h2>
            <p class="form-card__desc text-muted">
                "Set context length and KV cache settings for this model."
            </p>
        </div>

        // ── Section A: Context Length ──────────────────────────────────────
        <div class="form-section mb-4">
            <h3 class="form-label">"Context Length"</h3>
            <p class="text-muted text-sm mb-2">
                {move || {
                    if let Some(native) = gguf_context_length.get() {
                        format!("Native context: {} tokens. Set lower to use less RAM.", native)
                    } else {
                        "Set the context window size. Higher values use more RAM.".to_string()
                    }
                }}
            </p>
            <ContextLengthSelector
                class="input-narrow".to_string()
                value=Signal::derive(move || settings.get().context_length)
                on_change=Callback::new(move |v| {
                    settings.update(|s| s.context_length = v);
                })
                reset_key=Signal::derive(move || "wizard-context".to_string())
                max_context=Signal::derive(Some(max_context.get()))
            />
        </div>

        // ── Section B: KV Cache Quantization ───────────────────────────────
        <div class="form-section mb-4">
            <h3 class="form-label">"KV Cache Quantization"</h3>
            <p class="text-muted text-sm mb-2">
                "Quantize the KV cache to reduce memory usage. Leave as default (none) for best quality."
            </p>

            <div class="form-group mb-2">
                <label class="form-label text-sm">"Unified K/V Cache"</label>
                <label class="toggle">
                    <input
                        type="checkbox"
                        prop:checked=move || settings.get().kv_unified
                        on:change=move |e| {
                            settings.update(|s| s.kv_unified = event_target_checked(&e));
                        }
                    />
                    <span class="toggle-slider"></span>
                </label>
            </div>

            <div class="form-group mb-2">
                <label class="form-label text-sm">"K Cache Type"</label>
                <select
                    class="form-select input-narrow"
                    prop:value=move || settings.get().cache_type_k.clone().unwrap_or_default()
                    on:change=move |e| {
                        let v = crate::utils::target_value(&e);
                        settings.update(|s| {
                            s.cache_type_k = if v.is_empty() { None } else { Some(v) };
                        });
                    }
                >
                    <option value="">"Default (none)"</option>
                    {KV_QUANT_OPTIONS.iter().map(|opt| {
                        view! { <option value={*opt}>{opt}</option> }
                    }).collect::<Vec<_>>()}
                </select>
            </div>

            <Show when=move || !settings.get().kv_unified>
                <div class="form-group mb-2">
                    <label class="form-label text-sm">"V Cache Type"</label>
                    <select
                        class="form-select input-narrow"
                        prop:value=move || settings.get().cache_type_v.clone().unwrap_or_default()
                        on:change=move |e| {
                            let v = crate::utils::target_value(&e);
                            settings.update(|s| {
                                s.cache_type_v = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    >
                        <option value="">"Default (none)"</option>
                        {KV_QUANT_OPTIONS.iter().map(|opt| {
                            view! { <option value={*opt}>{opt}</option> }
                        }).collect::<Vec<_>>()}
                    </select>
                </div>
            </Show>
        </div>

        // ── Downloaded files summary ───────────────────────────────────────
        // Filter out mmproj files — this step is for model config only.
        // mmproj files don't have context_length or KV cache settings.
        <div class="form-section mb-3">
            <h3 class="form-label">"Downloaded Files"</h3>
            <div class="download-summary">
                {move || {
                    download_jobs.get().iter()
                        .filter(|job| !job.filename.starts_with("mmproj"))
                        .map(|job| {
                        let badge_class = if job.status == "completed" {
                            "badge badge-success"
                        } else {
                            "badge badge-error"
                        };
                        view! {
                            <div class="flex-between mb-1">
                                <span class="text-mono text-sm">{&job.filename}</span>
                                <span class=badge_class>
                                    {if job.status == "completed" { "Done ✓" } else { "Failed" }}
                                </span>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>
        </div>

        <div class="form-actions mt-3">
            <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                "Back"
            </button>
            <button class="btn btn-primary" on:click=move |_| on_next.run(())>
                "Save & Finish"
            </button>
        </div>
    }
}
```

**Steps:**
- [ ] Add `ContextSettings` struct and `KV_QUANT_OPTIONS` to `pull_wizard/mod.rs`
- [ ] Rewrite `context_step.rs` with the new component
- [ ] Run `cargo build --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: rewrite SetContext step with model-level context + KV cache"

**Acceptance criteria:**
- [ ] SetContext shows a single context length field (not per-quant)
- [ ] Context length pre-filled from GGUF-parsed value
- [ ] KV cache section with kv_unified toggle, cache_type_k, cache_type_v dropdowns
- [ ] V cache type hidden when kv_unified is true
- [ ] Downloaded files summary shown at bottom
- [ ] Build succeeds, clippy clean

---

### Task 5: Reorder wizard steps + wire new flow

**Context:**
The wizard orchestration (`pull_quant_wizard.rs`) needs to be updated to:
1. Reorder steps: Download before Context
2. Send simplified `PullRequest` (filenames only, no context_length)
3. After downloads complete, extract GGUF metadata from PullJob and pre-fill SetContext
4. On SetContext save, PUT to the existing model update endpoint with user's settings

**Files:**
- Modify: `crates/tama-web/src/components/pull_quant_wizard.rs` — main wizard orchestration
- Modify: `crates/tama-web/src/components/pull_wizard/mod.rs` — update `WizardStep` enum, `step_class` order array, `SsePayload`, signals

**What to implement:**

1. In `pull_wizard/mod.rs`, update `WizardStep`:
```rust
#[derive(Clone, Debug, PartialEq)]
pub enum WizardStep {
    RepoInput,
    LoadingQuants,
    SelectQuants,
    Downloading,
    SetContext,
    Done,
}
```

**CRITICAL:** Also update the `step_class()` function's `order` array to match the new enum order:
```rust
pub fn step_class(current: &WizardStep, target: &WizardStep, target_idx: usize) -> &'static str {
    let order = [
        WizardStep::RepoInput,
        WizardStep::LoadingQuants,
        WizardStep::SelectQuants,
        WizardStep::Downloading,     // was index 4, now index 3
        WizardStep::SetContext,      // was index 3, now index 4
        WizardStep::Done,
    ];
    // ... rest of function unchanged
}
```

**CRITICAL:** Also update `SsePayload` to include `gguf_context_length`:
```rust
#[derive(Deserialize, Clone)]
pub struct SsePayload {
    pub job_id: String,
    pub status: String,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    /// GGUF-parsed context length from the backend (set during download completion).
    #[serde(default)]
    pub gguf_context_length: Option<u64>,
}
```

2. In `pull_quant_wizard.rs`:

Update signals — remove `context_lengths` (per-quant map), add new signals:
```rust
let context_settings = RwSignal::new(ContextSettings::default());
let gguf_context_length = RwSignal::new(None::<u64>);
```

**CRITICAL:** Update the Reset Effect to include new signals:
```rust
// In the Reset Effect (when is_open transitions closed→open):
context_settings.set(ContextSettings::default());
gguf_context_length.set(None);
```

Update step ordering in the step indicator:
```rust
<div class=step_class(&step, &WizardStep::RepoInput, 0)>"1. Repo"</div>
<div class=step_class(&step, &WizardStep::LoadingQuants, 1)>"2. Loading"</div>
<div class=step_class(&step, &WizardStep::SelectQuants, 2)>"3. Select"</div>
<div class=step_class(&step, &WizardStep::Downloading, 3)>"4. Download"</div>
<div class=step_class(&step, &WizardStep::SetContext, 4)>"5. Configure"</div>
<div class=step_class(&step, &WizardStep::Done, 5)>"6. Done"</div>
```

Update `SelectQuants → on_next`:
```rust
on_next=Callback::new(move |_| {
    let sel = selected_filenames.get();
    let mmprojs = selected_mmproj_filenames.get();

    let body = PullRequest {
        repo_id: repo_id.get(),
        filenames: sel.iter().cloned().collect(),
        mmproj_filenames: mmprojs.iter().cloned().collect(),
    };

    wasm_bindgen_futures::spawn_local(async move {
        let build_result = post_request("/tama/v1/pulls").json(&body);
        // ... send request, start SSE streaming, transition to Downloading
    });
})
```

Update `Downloading → SetContext` transition (the Effect that watches for terminal state):
```rust
Effect::new(move |_| {
    let jobs = download_jobs.get();
    if jobs.is_empty() { return; }
    let all_terminal = jobs.iter().all(|j| j.status == "completed" || j.status == "failed");
    if !all_terminal { return; }

    let current_step = wizard_step.get();
    if current_step == WizardStep::Downloading {
        // The gguf_context_length signal was populated by the SSE handler.
        // All jobs are from the same model, so they share the same context_length.
        wizard_step.set(WizardStep::SetContext);
    }
});
```

Update SSE handler in `spawn_sse_streams` to extract `gguf_context_length`:
```rust
// In the SSE event processing loop, when deserializing SsePayload:
if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
    dj.update(|jobs| {
        if let Some(j) = jobs.iter_mut().find(|j| j.job_id == p.job_id) {
            j.bytes_downloaded = p.bytes_downloaded;
            j.total_bytes = p.total_bytes;
            j.status = p.status.clone();
            j.error = p.error.clone();
        }
    });
    // Capture GGUF context_length from any job (they're all the same model)
    if let Some(ctx) = p.gguf_context_length {
        gguf_context_length.set(Some(ctx));
    }
}
```

Update `SetContext → on_next` (save settings via **PUT** to existing `/tama/v1/models/:id` endpoint):

The existing `PUT /tama/v1/models/:id` endpoint (in `tama-web/src/api/models/crud/update.rs`) accepts a `ModelBody` with `context_length`, `kv_unified`, `cache_type_k`, `cache_type_v`. Use this endpoint — no new backend code needed.

```rust
on_next=Callback::new(move |_| {
    let settings = context_settings.get();
    let repo = repo_id.get();

    wasm_bindgen_futures::spawn_local(async move {
        let payload = serde_json::json!({
            "context_length": settings.context_length,
            "kv_unified": Some(settings.kv_unified),
            "cache_type_k": settings.cache_type_k,
            "cache_type_v": settings.cache_type_v,
        });

        // Model key is the repo slug (lowercase, / replaced with --)
        let model_key = repo.replace('/', "--").to_lowercase();

        match post_request(&format!("/tama/v1/models/{}", model_key))
            .method(reqwest::Method::PUT)
            .json(&payload)
        {
            Ok(req) => {
                match req.send().await {
                    Ok(resp) => {
                        if resp.status().is_success() {
                            wizard_step.set(WizardStep::Done);
                        } else {
                            error_msg.set(Some(format!("Failed to save settings (HTTP {})", resp.status())));
                        }
                    }
                    Err(e) => {
                        error_msg.set(Some(format!("Failed to save settings: {}", e)));
                    }
                }
            }
            Err(e) => {
                error_msg.set(Some(format!("Failed to build request: {}", e)));
            }
        }
    });
})
```

Update the `on_complete` callback to not include `context_length`:
```rust
CompletedQuant {
    repo_id: repo.clone(),
    filename: j.filename.clone(),
    quant,
    size_bytes: Some(j.bytes_downloaded),
    // context_length removed — it's model-level now
}
```

**Steps:**
- [ ] Update `WizardStep` enum in `pull_wizard/mod.rs`
- [ ] **Update `step_class()` order array** in `pull_wizard/mod.rs` (CRITICAL — must match new enum order: Downloading=3, SetContext=4)
- [ ] **Update `SsePayload`** to include `gguf_context_length: Option<u64>` (CRITICAL — otherwise GGUF data never reaches frontend)
- [ ] Update signals in `pull_quant_wizard.rs` (remove context_lengths, add context_settings + gguf_context_length)
- [ ] **Update Reset Effect** to reset new signals: `context_settings.set(ContextSettings::default())` and `gguf_context_length.set(None)`
- [ ] Reorder step indicator
- [ ] Update SelectQuants → on_next to send simplified PullRequest
- [ ] Update Downloading → SetContext transition
- [ ] Update SSE handler in `spawn_sse_streams` to extract `gguf_context_length` from `SsePayload`
- [ ] Update SetContext → on_next to **PUT** to existing `/tama/v1/models/:id` endpoint
- [ ] Update on_complete callback (remove context_length from CompletedQuant)
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: reorder wizard steps, wire GGUF metadata to SetContext"

**Acceptance criteria:**
- [ ] Wizard flow: SelectQuants → Downloading → SetContext → Done
- [ ] Downloads start immediately after quant selection
- [ ] SetContext pre-filled with GGUF context_length from SSE stream
- [ ] SetContext save sends **PUT** to existing `/tama/v1/models/:id` endpoint (no new backend code)
- [ ] `step_class()` order array matches new enum order (Downloading=3, SetContext=4)
- [ ] `SsePayload` includes `gguf_context_length` field
- [ ] Reset Effect clears `context_settings` and `gguf_context_length`
- [ ] All existing tests pass

---

### Task 6: README parsing — keep as fallback

**Context:**
The README parsing function (`parse_readme_metadata`) currently extracts context_length, num_layers, architecture_type, total_params, and active_params. With GGUF parsing as the primary source, README parsing becomes a fallback. We keep it for ALL fields (including context_length, num_layers, architecture_type) because GGUF parsing can fail (corrupt file, non-GGUF file, crate bug). README parsing also provides total_params and active_params which are NOT in GGUF.

**Files:**
- Modify: `crates/tama-core/src/models/pull/metadata.rs` — no functional changes, just update documentation
- Modify: `crates/tama-core/src/models/pull/api.rs` — ensure README fallback is used when GGUF fails

**What to implement:**

1. In `metadata.rs`, update the doc comment on `parse_readme_metadata`:
```rust
/// Parse a HuggingFace README markdown to extract model metadata.
///
/// This is a FALLBACK when GGUF header parsing is unavailable (file not yet
/// downloaded, parse failure, etc.). GGUF-parsed values take priority.
/// README parsing provides total_params and active_params which are NOT in GGUF.
```

2. In `api.rs` (`fetch_hf_metadata`), the existing README merge logic already fills `None` fields. No changes needed — it naturally acts as a fallback.

3. In `setup_model_after_pull` (from Task 2), ensure the fallback chain is:
```
GGUF context_length > spec.context_length (from request, backward compat) > README context_length > None
```

**Steps:**
- [ ] Update doc comment on `parse_readme_metadata` in `metadata.rs`
- [ ] Verify fallback chain in `setup_model_after_pull` (Task 2 already handles this)
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "docs: clarify README parsing as GGUF fallback"

**Acceptance criteria:**
- [ ] README parsing kept as fallback for all fields
- [ ] Documentation clarifies the fallback relationship
- [ ] No functional changes to parsing logic

---

## Verification

After all tasks complete:

1. **End-to-end test flow:**
   - Open pull wizard → enter repo ID → select quants → downloads start
   - After downloads complete → SetContext appears with context pre-filled from GGUF
   - Change context length → save → model config updated via PATCH
   - Model appears in models list with correct context length

2. **Backward compatibility:**
   - Legacy `quants` path in `PullRequest` still works
   - Existing card TOML files with per-quant context_length still load
   - DB queue rows with context_length still processed

3. **Edge cases:**
   - GGUF parse failure → context_length is `None`, user sets it manually
   - mmproj-only pull → SetContext shows no model quants, just completes
   - Multiple quants downloaded → all share the same model-level context_length

4. **Commands:**
   ```bash
   cargo build --workspace
   cargo test --workspace
   cargo fmt --all
   cargo clippy --workspace -- -D warnings
   ```
