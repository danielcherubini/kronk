# Interactive Model Pull Wizard

**Goal:** Replace the bare-bones pull form with a multi-step interactive wizard that lets users browse HF quants, select what to download, set context per quant, watch real download progress via SSE, and automatically get a model card + config entries on completion — all from the web UI without touching the terminal.

**Architecture:**
Three new proxy endpoints provide the data layer: `GET /koji/v1/hf/:repo_id/quants` lists available quants from HuggingFace, `POST /koji/v1/pulls` is extended to accept multiple quants and context lengths per quant, and `GET /koji/v1/pulls/:job_id/stream` streams `text/event-stream` events with live `bytes_downloaded` / `total_bytes` updates. Downloads are wired to the real `download_chunked` function with an `Arc<AtomicU64>` progress counter. On completion the handler writes/updates the model card (`configs/<repo>--<model>.toml`) and inserts `[models.*]` entries into `config.toml`. The Leptos pull page is replaced with a 6-step wizard component.

**Tech Stack:** Rust, Axum (`text/event-stream` via `axum::response::Sse`), `tokio::sync::watch`, Leptos (WASM), `gloo-net` for SSE, existing `download_chunked`, `ModelCard`, `Config::save`.

---

## Task 1: Add `GET /koji/v1/hf/:repo_id/quants` endpoint

**Context:**
The web wizard needs to fetch available GGUF quants for a given HuggingFace repo before the user selects what to download. A new endpoint wraps the existing `fetch_blob_metadata` / `parse_blob_siblings` logic in `crates/koji-core/src/models/pull.rs` and returns a JSON array of quant objects with filename, inferred quant name, and size in bytes.

`fetch_blob_metadata(repo_id)` returns `HashMap<String, BlobInfo>` where `BlobInfo` has:
- `filename: String`
- `blob_id: Option<String>`
- `size: Option<i64>`
- `lfs_sha256: Option<String>`

`infer_quant_from_filename(filename)` returns `Option<String>` (e.g. `"Q4_K_M"`, `"IQ3_S"`).

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` — add handler + response type
- Modify: `crates/koji-core/src/proxy/server/router.rs` — register new route

**What to implement:**

Add this response type near the top of `koji_handlers.rs`:
```rust
#[derive(Debug, Serialize)]
pub struct QuantEntry {
    pub filename: String,
    pub quant: Option<String>,
    pub size_bytes: Option<i64>,
}
```

Add the handler:
```rust
pub async fn handle_hf_list_quants(
    Path(repo_id): Path<String>,
) -> Response {
    match crate::models::pull::fetch_blob_metadata(&repo_id).await {
        Ok(blobs) => {
            let mut quants: Vec<QuantEntry> = blobs
                .into_values()
                .map(|b| QuantEntry {
                    quant: crate::models::pull::infer_quant_from_filename(&b.filename),
                    filename: b.filename,
                    size_bytes: b.size,
                })
                .collect();
            quants.sort_by(|a, b| a.filename.cmp(&b.filename));
            (StatusCode::OK, Json(quants)).into_response()
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
```

Note: `handle_hf_list_quants` does NOT take `State<Arc<ProxyState>>` — it makes a direct HF API call and needs no server state. The `Path` extractor uses `repo_id` as a **path segment**, but HF repo IDs contain a `/` (e.g. `bartowski/Qwen3-8B-GGUF`). Use a wildcard route `"/koji/v1/hf/*repo_id"` in the router so that the slash is captured, and strip any leading `/` from `repo_id` in the handler:
```rust
// In router.rs:
.route("/koji/v1/hf/*repo_id", get(handle_hf_list_quants))

// In handler, Path(repo_id) arrives as "bartowski/Qwen3-8B-GGUF" (no leading slash with Axum wildcard)
```

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/koji_handlers.rs` (imports section, lines 1–25) and `crates/koji-core/src/proxy/server/router.rs` fully before making changes.
- [ ] Write a failing test `test_quant_entry_serializes` in the `#[cfg(test)]` block of `koji_handlers.rs`: construct a `QuantEntry`, serialize to JSON, assert `"filename"`, `"quant"`, and `"size_bytes"` keys are present.
- [ ] Run `cargo test --package koji-core test_quant_entry_serializes` — expect failure.
- [ ] Add `QuantEntry` struct and `handle_hf_list_quants` to `koji_handlers.rs`.
- [ ] Register the route in `router.rs`: `.route("/koji/v1/hf/*repo_id", get(handle_hf_list_quants))`.
- [ ] Run `cargo test --package koji-core test_quant_entry_serializes` — must pass.
- [ ] Run `cargo build --workspace` — must succeed.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `"feat: add GET /koji/v1/hf/*repo_id endpoint to list HF quants"`

**Acceptance criteria:**
- [ ] `GET /koji/v1/hf/bartowski/Qwen3-8B-GGUF` returns a JSON array of `{ filename, quant, size_bytes }` objects
- [ ] Returns `502` with `{ "error": "..." }` if HF API fails
- [ ] `cargo build --workspace` succeeds

---

## Task 2: Wire up real downloads with progress tracking

**Context:**
`handle_koji_pull_model` in `crates/koji-core/src/proxy/koji_handlers.rs` currently spawns a stub task that sleeps 2 seconds then marks the job `Completed`. The real `download_chunked` function exists in `crates/koji-core/src/models/download/mod.rs` but is never called. `PullJob` has `bytes_downloaded: u64` and `total_bytes: Option<u64>` fields that are never updated.

The `download_chunked` function signature:
```rust
pub async fn download_chunked(
    url: &str,
    dest: &Path,
    connections: usize,
    auth_header: Option<&str>,
) -> Result<u64>  // returns total bytes
```

It uses `indicatif` internally — there is no progress callback. To expose progress without refactoring `download_chunked`, use `tokio::sync::watch` channel to carry `(bytes_downloaded: u64, total_bytes: Option<u64>)`.

The plan: do a `HEAD` request first (same logic already in `download_chunked`) to get `Content-Length`, set `total_bytes` on the `PullJob`, then call `download_chunked` in a `spawn_blocking`-like wrapper. Since `download_chunked` is async, run it directly inside the `tokio::spawn` task. After completion, update `bytes_downloaded` to equal `total_bytes` and mark `Completed`.

For real progress mid-download, add a `progress_tx: Option<tokio::sync::watch::Sender<u64>>` field to `PullJob` (skip-serialized) and update it from a wrapper. **For this task**, keep it simpler: do a HEAD for `total_bytes` upfront, start the download, and update `bytes_downloaded` = `total_bytes` when done. True byte-by-byte progress can be added later. What matters here is that the download is real, the file lands on disk, and the job completes with accurate final size.

**Important:** The `PullRequest` currently accepts a single `quant: Option<String>`. For the wizard, it needs to accept **multiple quants with context lengths**. Extend `PullRequest` to:
```rust
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo_id: String,
    // Legacy single-quant support (kept for backward compat):
    #[serde(default)]
    pub quant: Option<String>,
    // New multi-quant wizard format:
    #[serde(default)]
    pub quants: Vec<QuantDownloadSpec>,
    #[serde(default)]
    pub context_length: Option<u32>,  // legacy single context
}

#[derive(Debug, Deserialize, Clone)]
pub struct QuantDownloadSpec {
    pub filename: String,
    pub quant: Option<String>,
    pub context_length: Option<u32>,
}
```

When `quants` is non-empty, spawn a separate `PullJob` per entry and return a **JSON array** of `{ job_id, filename, status }` objects. When only `quant` is set (legacy), behave as before but with real download AND **return the same single-object JSON format** as today (`{ job_id, status, repo_id, filename }`). The response format for the legacy path must NOT change — the existing `pull.rs` web page is still in use until Task 5 replaces it.

The destination directory for downloads: `config.models_dir()? / repo_id_slug` where `repo_id_slug` replaces `/` with `--` (consistent with how card files are named). To get the config, load it fresh inside the spawn task: `crate::config::Config::load()`.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` — extend `PullRequest`, replace stub with real download, add `QuantDownloadSpec`
- Modify: `crates/koji-core/src/proxy/pull_jobs.rs` — no struct changes needed

**What to implement:**

Replace the `tokio::spawn` stub body with:
```rust
tokio::spawn(async move {
    // Update status to Running
    {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = PullJobStatus::Running;
        }
    }

    let config = match crate::config::Config::load() {
        Ok(c) => c,
        Err(e) => { /* set Failed */ return; }
    };
    let models_dir = match config.models_dir() {
        Ok(d) => d,
        Err(e) => { /* set Failed */ return; }
    };
    let repo_slug = repo_id_clone.replace('/', "--");
    let dest_dir = models_dir.join(&repo_slug);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) { /* set Failed */ return; }

    let dest_path = dest_dir.join(&filename_clone);
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        repo_id_clone, filename_clone
    );

    // HEAD request to get total_bytes
    let client = reqwest::Client::new();
    if let Ok(resp) = client.head(&url).send().await {
        let total = crate::models::download::parse_content_length(resp.headers());
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.total_bytes = total;
        }
    }

    // Real download
    match crate::models::download::download_chunked(&url, &dest_path, 8, None).await {
        Ok(bytes) => {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.bytes_downloaded = bytes;
                job.total_bytes = Some(bytes);
                job.status = PullJobStatus::Completed;
                job.completed_at = Some(std::time::Instant::now());
            }
        }
        Err(e) => {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = PullJobStatus::Failed;
                job.error = Some(e.to_string());
            }
        }
    }
});
```

`parse_content_length` is already public in `crates/koji-core/src/models/download/mod.rs`.

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/koji_handlers.rs` (the full `handle_koji_pull_model` function) and `crates/koji-core/src/models/download/mod.rs` fully.
- [ ] Read `crates/koji-core/src/proxy/pull_jobs.rs` fully.
- [ ] Add `QuantDownloadSpec` struct and extend `PullRequest` to include `quants: Vec<QuantDownloadSpec>` and `context_length: Option<u32>`.
- [ ] Replace the stub `tokio::spawn` body with the real download logic shown above.
- [ ] When `request.quants` is non-empty: iterate, spawn one job per entry, return a JSON array of `{ job_id, filename, status }` objects.
- [ ] When only legacy `quant` is set: behave as before but with real download.
- [ ] Run `cargo build --workspace` — must succeed.
- [ ] Run `cargo test --workspace` — all must pass.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `"feat: wire up real downloads in pull handler with multi-quant support"`

**Acceptance criteria:**
- [ ] `POST /koji/v1/pulls` with `{ repo_id, quants: [{ filename, quant, context_length }] }` creates one `PullJob` per quant
- [ ] Real `download_chunked` is called; files land in `models_dir/<repo_slug>/`
- [ ] `total_bytes` is set from a HEAD request before download starts
- [ ] `bytes_downloaded` equals `total_bytes` when job is `Completed`
- [ ] `status` transitions `pending → running → completed/failed`
- [ ] All existing tests still pass

---

## Task 3: Add SSE streaming for pull job progress

**Context:**
The current polling approach (`GET /koji/v1/pulls/:job_id` every 1s) works but SSE is cleaner for streaming progress. Axum supports SSE via `axum::response::Sse` and the `futures_util::stream` machinery. We add a new endpoint `GET /koji/v1/pulls/:job_id/stream` that streams `PullJob` snapshots as `text/event-stream` events every 500ms until the job reaches a terminal state (`Completed` or `Failed`), then sends a final event and closes.

The SSE event format:
```
event: progress
data: {"job_id":"...","status":"running","bytes_downloaded":1234567,"total_bytes":4800000000,"filename":"...","error":null}

event: done
data: {"job_id":"...","status":"completed","bytes_downloaded":4800000000,...}
```

Axum SSE example pattern. The state tuple carries `(Arc<ProxyState>, job_id, done: bool)`. On the first iteration the state is checked immediately (no initial sleep) so clients connected after completion see the `done` event right away. The `done` boolean is set to `true` after a terminal event is emitted; the next iteration sees `done == true` and returns `None` to close the stream:

```rust
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream};
use std::convert::Infallible;

pub async fn handle_pull_job_stream(
    state: State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // State: (proxy_state, job_id, just_emitted_done)
    let stream = stream::unfold(
        (state.0, job_id, false),
        |(state, job_id, just_done)| async move {
            // If the previous iteration already emitted a done event, close the stream.
            if just_done {
                return None;
            }
            // Sleep before the next poll (skipped on very first call implicitly — the sleep
            // is placed here so the first event is emitted after 500ms; for instant completion
            // the client will wait at most 500ms before seeing the done event).
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let jobs = state.pull_jobs.read().await;
            let Some(job) = jobs.get(&job_id).cloned() else {
                // Job not found — close the stream.
                return None;
            };
            drop(jobs);
            let is_terminal = matches!(job.status, PullJobStatus::Completed | PullJobStatus::Failed);
            let event_name = if is_terminal { "done" } else { "progress" };
            let data = serde_json::to_string(&job).unwrap_or_default();
            let event = Event::default().event(event_name).data(data);
            // If terminal, set just_done=true so the next iteration closes the stream.
            Some((Ok(event), (state, job_id, is_terminal)))
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}
```

Note: `futures_util` is already a workspace dependency (`futures-util.workspace = true` in `koji-core/Cargo.toml`).

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` — add `handle_pull_job_stream`
- Modify: `crates/koji-core/src/proxy/server/router.rs` — add route `.route("/koji/v1/pulls/:job_id/stream", get(handle_pull_job_stream))`

**What to implement:**

Add the `handle_pull_job_stream` function as shown above. The `PullJob` struct already derives `Serialize`, so `serde_json::to_string(&job)` works directly.

The `stream::unfold` state machine carries `(state, job_id, just_done: bool)`:
1. If `just_done == true`: return `None` (close stream — previous iteration already emitted the `done` event)
2. Sleep 500ms
3. Reads `state.pull_jobs` (read lock)
4. If job not found: return `None` (close stream)
5. If terminal: emit `done` event with `just_done = true` in next state — stream closes on next iteration
6. Otherwise: emit `progress` event, `just_done = false`, continue

**Steps:**
- [ ] Read `crates/koji-core/src/proxy/koji_handlers.rs` (imports section) and `crates/koji-core/src/proxy/server/router.rs` fully.
- [ ] Write a failing test `test_pull_job_serializes_for_sse` in the `#[cfg(test)]` block: construct a `PullJob`, call `serde_json::to_string`, assert `"bytes_downloaded"` and `"status"` are present in the output.
- [ ] Run `cargo test --package koji-core test_pull_job_serializes_for_sse` — expect failure.
- [ ] Add `handle_pull_job_stream` to `koji_handlers.rs`. Add necessary imports: `use axum::response::sse::{Event, KeepAlive, Sse}; use futures_util::stream;`.
- [ ] Add `use std::convert::Infallible;` if not already present.
- [ ] Register route in `router.rs`.
- [ ] Run `cargo test --package koji-core test_pull_job_serializes_for_sse` — must pass.
- [ ] Run `cargo build --workspace` — must succeed.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `"feat: add SSE streaming endpoint GET /koji/v1/pulls/:job_id/stream"`

**Acceptance criteria:**
- [ ] `GET /koji/v1/pulls/:job_id/stream` returns `Content-Type: text/event-stream`
- [ ] Emits `progress` events every 500ms while job is running
- [ ] Emits a final `done` event when job reaches `Completed` or `Failed`, then closes
- [ ] Returns no events (stream closes immediately) if `job_id` not found
- [ ] `cargo build --workspace` succeeds

---

## Task 4: Post-download: auto-create model card and config entries

**Context:**
After all quants for a repo finish downloading, the handler should automatically:
1. Load or create a `ModelCard` at `<configs_dir>/<repo_slug>.toml`
2. Add/update a `QuantInfo` entry for each downloaded quant
3. Try to fetch the HF community card via `fetch_community_card(repo_id)` — if it returns `Some`, merge its `model.name`, `sampling`, and `default_context_length` into our card (without overwriting existing quant entries)
4. Save the card
5. Add `[models.<slug>-<quant_lower>]` entries to `config.toml` for each quant, pointing at the repo and quant
6. Save config

The repo slug for file naming: `repo_id.replace('/', "--")` (e.g. `bartowski--Qwen3-8B-GGUF`). The card path: `config.configs_dir()? / format!("{}.toml", repo_slug)`.

A `ModelConfig` entry to insert looks like (note: `ModelConfig` does NOT implement `Default`, so all fields must be specified explicitly):
```rust
ModelConfig {
    backend: "llama_cpp".to_string(),
    model: Some(repo_id.to_string()),
    quant: Some(quant_key.clone()),
    context_length: spec.context_length,
    enabled: true,
    args: vec![],
    profile: None,
    sampling: None,
    port: None,
    health_check: None,
}
```

The key in `config.models` HashMap: derive a slug from the repo + quant, e.g. `format!("{}-{}", repo_slug.to_lowercase(), quant.to_lowercase())` with `/` and `_` replaced by `-`.

This post-download logic should live in a new private async function `setup_model_after_pull(repo_id, downloaded_specs, config_dir)` called at the end of the `tokio::spawn` task in `handle_koji_pull_model` (after all downloads for a repo complete).

`fetch_community_card` returns `Option<ModelCard>` — it's best-effort. If it returns `None`, skip it silently. If it returns `Some`, only copy `model.name` and `sampling` entries to the new card (do not overwrite `quants`).

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs` — add `setup_model_after_pull`, call it from the spawn task

**What to implement:**

```rust
async fn setup_model_after_pull(
    repo_id: &str,
    specs: &[QuantDownloadSpec],
    dest_dir: &std::path::Path,  // the models_dir/<repo_slug>/ path where files landed
) {
    let Ok(mut config) = crate::config::Config::load() else { return };
    let Ok(configs_dir) = config.configs_dir() else { return };
    let repo_slug = repo_id.replace('/', "--");
    let card_path = configs_dir.join(format!("{}.toml", repo_slug));

    // Load existing or build a new card
    let mut card = crate::models::card::ModelCard::load(&card_path).unwrap_or_else(|_| {
        crate::models::card::ModelCard {
            model: crate::models::card::ModelMeta {
                name: repo_id.split('/').last().unwrap_or(repo_id).to_string(),
                source: repo_id.to_string(),
                default_context_length: None,
                default_gpu_layers: None,
            },
            sampling: Default::default(),
            quants: Default::default(),
        }
    });

    // Try community card for name + sampling
    if let Some(community) = crate::models::pull::fetch_community_card(repo_id).await {
        if !community.model.name.is_empty() {
            card.model.name = community.model.name;
        }
        for (k, v) in community.sampling {
            card.sampling.entry(k).or_insert(v);
        }
        if card.model.default_context_length.is_none() {
            card.model.default_context_length = community.model.default_context_length;
        }
    }

    // Add quant entries
    for spec in specs {
        let quant_key = spec.quant.clone().unwrap_or_else(|| {
            crate::models::pull::infer_quant_from_filename(&spec.filename)
                .unwrap_or_else(|| spec.filename.trim_end_matches(".gguf").to_string())
        });
        let size_bytes = std::fs::metadata(dest_dir.join(&spec.filename))
            .ok()
            .map(|m| m.len());
        card.quants.insert(quant_key.clone(), crate::models::card::QuantInfo {
            file: spec.filename.clone(),
            size_bytes,
            context_length: spec.context_length,
        });
        // Add model config entry
        let model_key = format!(
            "{}-{}",
            repo_slug.to_lowercase().replace('/', "-"),
            quant_key.to_lowercase().replace('_', "-")
        );
        config.models.entry(model_key).or_insert_with(|| crate::config::types::ModelConfig {
            backend: "llama_cpp".to_string(),
            model: Some(repo_id.to_string()),
            quant: Some(quant_key),
            context_length: spec.context_length,
            enabled: true,
            args: vec![],
            profile: None,
            sampling: None,
            port: None,
            health_check: None,
        });
    }

    let _ = std::fs::create_dir_all(&configs_dir);
    let _ = card.save(&card_path);
    let _ = config.save();
}
```

Call `setup_model_after_pull` from the spawn task after all downloads for the batch complete (not per-quant — once when the last job finishes).

Since each `PullJob` is independent and there is no batch coordination yet, for the multi-quant case: call `setup_model_after_pull` inside each individual job's spawn task, passing only its own `spec`. This means each quant triggers its own card update (which is idempotent since we `or_insert`).

**Steps:**
- [ ] Read the full current `handle_koji_pull_model` function and `crates/koji-core/src/config/types.rs` (ModelConfig struct + Default impl).
- [ ] Check if `ModelConfig` implements `Default` — if not, add `#[derive(Default)]` or use field initialization as shown above.
- [ ] Write a failing test `test_setup_model_creates_card` in `#[cfg(test)]` block: use `tempfile::tempdir()` as config dir, call `setup_model_after_pull` with a mock spec, assert the card TOML is created at the expected path.
- [ ] Run test — expect failure.
- [ ] Add `setup_model_after_pull` to `koji_handlers.rs`.
- [ ] Call it from the download spawn task for each completed job.
- [ ] Run the test — must pass.
- [ ] Run `cargo build --workspace && cargo test --workspace`.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `"feat: auto-create model card and config entries after successful pull"`

**Acceptance criteria:**
- [ ] After a successful pull, `configs/<repo_slug>.toml` exists with `[quants.<quant>]` entry
- [ ] `config.toml` has a new `[models.<slug>]` entry for each downloaded quant
- [ ] Community card name/sampling is merged if HF returns one
- [ ] Existing quant entries in the card are not overwritten (idempotent `or_insert`)
- [ ] If card/config save fails, the download is still marked `Completed` (best-effort setup)
- [ ] All tests pass

---

## Task 5: Replace pull page with interactive wizard UI

**Context:**
The current `crates/koji-web/src/pages/pull.rs` is a 142-line bare form. Replace it entirely with a 6-step wizard:

**Step 1 — Enter repo ID:** Text input + "Search" button. Validates non-empty.

**Step 2 — Loading quants:** Shows a spinner while `GET /koji/v1/hf/*repo_id` is in flight.

**Step 3 — Select quants:** Renders a list of quants from the API response as checkboxes. Each row shows: checkbox | quant name | filename | size (human-readable). "Select All" convenience button. "Next" disabled until at least one is checked.

**Step 4 — Set context lengths:** For each selected quant, shows a number input labelled with the quant name. Pre-fills `32768` as default. "Start Download" button.

**Step 5 — Downloading:** For each quant being downloaded, shows a progress row: filename | progress bar | bytes/total | status badge. Uses SSE (`GET /koji/v1/pulls/:job_id/stream`) for live updates. When all jobs reach terminal state, shows "Setup complete ✓" and "Go to Models" link.

**Step 6 — Done:** Summary of what was downloaded + links to `/models`.

**State machine (Leptos signals):**
```
wizard_step: RwSignal<WizardStep>  (enum: RepoInput, LoadingQuants, SelectQuants, SetContext, Downloading, Done)
repo_id: RwSignal<String>
available_quants: RwSignal<Vec<QuantEntry>>
selected_filenames: RwSignal<HashSet<String>>
context_lengths: RwSignal<HashMap<String, u32>>  // keyed by filename
download_jobs: RwSignal<Vec<JobProgress>>
```

`QuantEntry` (local struct mirroring the API response):
```rust
#[derive(Deserialize, Clone)]
struct QuantEntry {
    filename: String,
    quant: Option<String>,
    size_bytes: Option<i64>,
}
```

`JobProgress` (local struct for download tracking):
```rust
#[derive(Clone)]
struct JobProgress {
    job_id: String,
    filename: String,
    status: String,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
}
```

For SSE in Leptos/WASM, use `gloo-net`'s `EventSource`:
```rust
use gloo_net::eventsource::futures::EventSource;
let mut es = EventSource::new(&format!("/koji/v1/pulls/{}/stream", job_id)).unwrap();
let stream = es.subscribe("progress").unwrap();
// spawn_local + for_each to update JobProgress signal
```

For formatting file sizes: add a helper `fn format_bytes(bytes: i64) -> String` that formats to MiB/GiB with one decimal place.

**Files:**
- Rewrite: `crates/koji-web/src/pages/pull.rs`

**What to implement:**

Full replacement of `pull.rs`. Key sections:

1. **`#[component] pub fn Pull()`** — single top-level component with all signals declared at the top.
2. **Step rendering** via `move || match wizard_step.get() { ... }` returning different view fragments per step.
3. **Step 1 view:** `<input>` bound to `repo_id`, "Search" button that sets `wizard_step = LoadingQuants` and triggers a `spawn_local` to call the quants endpoint.
4. **Step 2 view:** Loading spinner `<p>"Searching HuggingFace..."</p>`.
5. **Step 3 view:** List of checkboxes. Use `For` component over `available_quants`. "Next" button → `wizard_step = SetContext`.
6. **Step 4 view:** `For` over selected quants, each with a `<input type="number">` for context. "Start Download" → `spawn_local` that POSTs `{ repo_id, quants: [...] }` to `/koji/v1/pulls`, collects `job_id`s, sets `wizard_step = Downloading`.
7. **Step 5 view:** `For` over `download_jobs`. Each row: filename, progress bar (`<progress value=bytes max=total>`), status. In a `create_effect`, open SSE for each job and update `download_jobs` signal on each event.
8. **Step 6 view:** "All downloads complete!" with link to `/models`.

For the POST to `/koji/v1/pulls` returning multiple jobs (one per quant), the handler (from Task 2) returns a JSON array. The wizard POST body:
```json
{
  "repo_id": "bartowski/Qwen3-8B-GGUF",
  "quants": [
    { "filename": "Qwen3-8B-Q4_K_M.gguf", "quant": "Q4_K_M", "context_length": 8192 },
    { "filename": "Qwen3-8B-Q8_0.gguf", "quant": "Q8_0", "context_length": 16384 }
  ]
}
```

Response (JSON array):
```json
[
  { "job_id": "uuid1", "filename": "Qwen3-8B-Q4_K_M.gguf", "status": "pending" },
  { "job_id": "uuid2", "filename": "Qwen3-8B-Q8_0.gguf", "status": "pending" }
]
```

**Steps:**
- [ ] Read `crates/koji-web/src/pages/pull.rs` (current full content) and `crates/koji-web/src/pages/models.rs` (for Leptos patterns used in this codebase) before writing.
- [ ] **Required:** Add `"eventsource"` feature to `gloo-net` in the workspace root `Cargo.toml`. The current workspace declaration is `gloo-net = { version = "0.6", features = ["http"] }` — change it to `gloo-net = { version = "0.6", features = ["http", "eventsource"] }`. Without this, `gloo_net::eventsource` will not compile. Run `cargo build --package koji-web` after this change to confirm it picks up the feature.
- [ ] Rewrite `pull.rs` with the wizard component.
- [ ] Run `cargo build --package koji-web` — fix any type/import errors.
- [ ] Run `cargo build --workspace` — must succeed.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `"feat: replace pull page with interactive multi-step download wizard"`

**Acceptance criteria:**
- [ ] Step 1: user can enter a repo ID and click Search
- [ ] Step 3: quants are listed as checkboxes with filename, quant name, and size
- [ ] Step 4: each selected quant has a context length number input
- [ ] Step 5: SSE progress updates are shown live per quant with a `<progress>` bar
- [ ] Step 6: completion state with link to `/models`
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes
