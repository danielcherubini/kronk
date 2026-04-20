# Updates Center Fix

**Goal:** Make the Updates Center usable by showing progress for backend updates, checking quants individually (not models), and routing all model/quant downloads through the Downloads Center with toast notifications.

**Architecture:** Five coordinated tasks: (1) wire JobLogPanel into Updates Center for backend updates, (2) add LRU cache for remote GGUF listings to reduce HuggingFace API calls, (3) rewrite update checker to compare per-quant LFS hashes instead of repo-level commit SHA, (4) rewrite the model update endpoint to enqueue quant downloads through DownloadQueueService, (5) add frontend per-quant UI with expandable list, checkboxes, and toast notifications. No new DB tables — use existing `details_json` column in `update_checks`.

**Tech Stack:** Rust (koji-core, koji-web), Leptos (WASM frontend), SQLite (rusqlite), SSE for real-time updates, DownloadQueueService for download lifecycle. Dependencies to add: `lru` crate for LRU cache.

---

### Task 1: Backend Updates — JobLogPanel in Updates Center

**Context:**
The Backends page already shows live build progress via `<JobLogPanel>` when a backend update is running. The Updates Center has the same "Update" button but does not capture or display the `job_id` returned by `/api/backends/:name/update`. This task wires up the existing infrastructure so backend updates show progress inline in the Updates Center, using the same JobLogPanel component pattern from `backends.rs`.

**Files:**
- Modify: `crates/koji-web/src/pages/updates.rs`

**What to implement:**

Add two new signal local variables inside the `Updates` component function body (after existing signals):
```rust
let active_backend_job_id = RwSignal::new(Option::<String>::None);
let backend_update_busy = RwSignal::new(false);
```

Rewrite `on_update_backend` callback. The current code is:
```rust
let on_update_backend = move |name: String| {
    wasm_bindgen_futures::spawn_local(async move {
        let url = format!("/api/backends/{}/update", name);
        let _ = gloo_net::http::Request::post(&url).send().await;
    });
};
```

Replace with:
```rust
let on_update_backend = move |name: String| {
    backend_update_busy.set(true);
    wasm_bindgen_futures::spawn_local(async move {
        let url = format!("/api/backends/{}/update", name);
        if let Ok(resp) = gloo_net::http::Request::post(&url).send().await {
            if resp.ok() {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    if let Some(job_id) = data["job_id"].as_str() {
                        active_backend_job_id.set(Some(job_id.to_string()));
                    }
                }
            } else {
                let text = resp.text().await.unwrap_or_default();
                error.set(Some(format!("Update failed: {}", text)));
            }
        }
    });
};
```

Add a close handler (place it near other callbacks):
```rust
let on_backend_job_close = move |_| {
    active_backend_job_id.set(None);
    backend_update_busy.set(false);
    // Refresh the updates list after job completes
    wasm_bindgen_futures::spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(500).await;
        if let Ok(resp) = gloo_net::http::Request::get("/api/updates").send().await {
            if let Ok(data) = resp.json::<UpdatesListResponse>().await {
                updates.set(data);
                let all_items: Vec<_> = data.backends.iter().chain(data.models.iter()).collect();
                last_checked.set(all_items.iter().map(|r| r.checked_at).max());
            }
        }
    });
};
```

In the view, add this block **between** the `</section>` closing tag for backends and the `<section class="updates-section">` for models:
```rust
{/* Backend update progress panel */}
{move || active_backend_job_id.get().map(|job_id| {
    view! {
        <JobLogPanel job_id=job_id on_close=on_backend_job_close />
    }.into_any()
})}
```

Import `JobLogPanel` by adding it to the existing use statements at the top of the file. The current imports are:
```rust
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use crate::components::self_update_section::SelfUpdateSection;
```

Add:
```rust
use crate::components::job_log_panel::JobLogPanel;
```

**Steps:**
- [ ] Read `crates/koji-web/src/components/job_log_panel.rs` to confirm component API (props: `job_id: String`, optional `on_close: Option<Callback<()>>`)
- [ ] Add `use crate::components::job_log_panel::JobLogPanel;` import
- [ ] Add `let active_backend_job_id = RwSignal::new(Option::<String>::None);` and `let backend_update_busy = RwSignal::new(false);` signals to the `Updates` component
- [ ] Rewrite `on_update_backend` callback to capture `job_id` from response JSON (`serde_json::Value`, extract `["job_id"]`)
- [ ] Add `on_backend_job_close` handler that clears both signals and refreshes the updates list (match the pattern in `on_check_now` for refreshing)
- [ ] Insert `{move || active_backend_job_id.get().map(|jid| view! { <JobLogPanel job_id=jid on_close=on_backend_job_close /> }.into_any())}` between the backends section `</section>` and the models section `<section class="updates-section">`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace` to verify no compilation errors

**Acceptance criteria:**
- [ ] Clicking "Update" on a backend in the Updates Center shows a JobLogPanel with live build logs via SSE
- [ ] Closing the panel clears the job_id and refreshes the updates list
- [ ] No Leptos/Rust compilation errors
- [ ] Follows the exact same pattern as `crates/koji-web/src/pages/backends.rs` (lines 108-125 for JobLogPanel rendering)

---

### Task 2: LRU Cache for Remote GGUF Listings

**Context:**
When "Check Now" is clicked, the update checker iterates over all models and fetches remote GGUF listings from HuggingFace. With N models × 2 API calls each (list_gguf_files + fetch_blob_metadata), this generates many HTTP requests. An in-memory LRU cache reduces redundant network calls. The cache must include a timestamp so stale entries can be detected and evicted.

**Files:**
- Modify: `crates/koji-core/Cargo.toml`
- Modify: `crates/koji-core/src/updates/checker.rs`

**What to implement:**

Add the `lru` crate dependency to `crates/koji-core/Cargo.toml`. Check if it's already present; if not, add it under `[dependencies]`:
```toml
lru = "0.12"
```

Create a `GgufListingCache` struct in `checker.rs` (place it near the top of the file, after imports):
```rust
/// In-memory LRU cache for HuggingFace GGUF file listings.
/// Reduces API calls by caching (commit_sha, files) per repo_id for 5 minutes.
#[derive(Clone)]
pub struct GgufListingCache {
    cache: tokio::sync::Mutex<lru::LruCache<String, (String, Vec<crate::models::pull::GgufFile>, i64)>>,
}

impl GgufListingCache {
    const TTL_SECS: i64 = 300; // 5 minutes
    const CAPACITY: usize = 64;
    
    pub fn new() -> Self {
        Self {
            cache: tokio::sync::Mutex::new(lru::LruCache::new(
                std::num::NonZeroUsize::new(Self::CAPACITY).unwrap()
            )),
        }
    }
    
    /// Get a cached entry if it exists and is fresh (within TTL).
    pub async fn get(&self, repo_id: &str) -> Option<(String, Vec<crate::models::pull::GgufFile>)> {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.get(repo_id) {
            let (sha, files, epoch) = entry;
            if now - *epoch < Self::TTL_SECS {
                return Some((sha.clone(), files.clone()));
            }
            // Stale — remove it so the next call fetches fresh data
            cache.pop(repo_id);
        }
        None
    }
    
    /// Store a result in the cache with the current timestamp.
    pub async fn insert(&self, repo_id: String, commit_sha: String, files: Vec<crate::models::pull::GgufFile>) {
        let now = chrono::Utc::now().timestamp();
        let mut cache = self.cache.lock().await;
        cache.put(repo_id, (commit_sha, files, now));
    }
}
```

Add the cache as a field on `UpdateChecker`:
```rust
pub struct UpdateChecker {
    lock: Arc<Mutex<()>>,
    gguf_listing_cache: GgufListingCache,
}
```

Initialize it in `UpdateChecker::new()`:
```rust
pub fn new() -> Self {
    Self {
        lock: Arc::new(Mutex::new(())),
        gguf_listing_cache: GgufListingCache::new(),
    }
}
```

In `check_model()`, before calling `pull::list_gguf_files(repo_id)`, check the cache first. If found and fresh, skip the `list_gguf_files` network call entirely (the files list is already cached). Still call `fetch_blob_metadata` for LFS hashes — that's a lighter call:

```rust
// Check cache before network call
let remote_listing = match self.gguf_listing_cache.get(repo_id).await {
    Some((cached_sha, cached_files)) => {
        // Use cached file list — still need LFS hashes from fetch_blob_metadata
        let blobs = pull::fetch_blob_metadata(&cached_sha).await?;
        crate::models::pull::RepoListing {
            repo_id: repo_id.to_string(),
            commit_sha: cached_sha.clone(),
            files: cached_files,
        }
    }
    None => pull::list_gguf_files(repo_id).await?
};

// After successful fetch, store in cache (only if not already cached)
if self.gguf_listing_cache.get(repo_id).await.is_none() {
    self.gguf_listing_cache.insert(
        repo_id.to_string(),
        remote_listing.commit_sha.clone(),
        remote_listing.files.clone(),
    ).await;
}
```

**Steps:**
- [ ] Check if `lru` crate is already in `crates/koji-core/Cargo.toml`; if not, add `lru = "0.12"` under `[dependencies]`
- [ ] Create `GgufListingCache` struct with `tokio::sync::Mutex<lru::LruCache<String, (String, Vec<GgufFile>, i64)>>` — the tuple stores commit_sha, files, and insertion epoch
- [ ] Implement `new()`, `get()` (with TTL check using `chrono::Utc::now().timestamp()`), and `insert()` methods
- [ ] Add `gguf_listing_cache: GgufListingCache` field to `UpdateChecker` struct
- [ ] Initialize in `UpdateChecker::new()` as `GgufListingCache::new()`
- [ ] In `check_model()`, add cache check before calling `pull::list_gguf_files(repo_id)`. If cached and fresh, use it. After successful fetch, store in cache.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`

**Acceptance criteria:**
- [ ] `GgufListingCache` struct exists with `get()` (returns None for stale entries) and `insert()` methods
- [ ] Cache entry stores `(commit_sha, Vec<GgufFile>, insertion_epoch_seconds)` — epoch enables TTL checking
- [ ] Cache is initialized in `UpdateChecker::new()` with capacity 64
- [ ] `check_model()` checks cache before making network calls to HuggingFace
- [ ] Successful results are stored in the cache after network calls
- [ ] No compilation errors

---

### Task 3: Per-Quant Update Checking

**Context:**
The current `check_model()` compares repo-level commit SHAs, which misses quant-specific updates (new quants added to a repo, or existing quants getting new hashes). This task rewrites the checker to iterate each model's file records and compare individual LFS hashes against the remote GGUF listing. Results are stored in the existing `details_json` column of `update_checks` as a JSON object with per-quant breakdown.

**Files:**
- Modify: `crates/koji-core/src/updates/checker.rs`
- Modify: `crates/koji-web/src/api/updates.rs`

**What to implement in `checker.rs`:**

Rewrite the `check_model()` function. The current flow is: get model pull record → fetch remote listing → compare commit SHA → if different, compare file hashes. The new flow:

1. Get the model's file records from DB via `db::queries::get_model_files(&open.conn, model_record.id)?`. This returns a `Vec<ModelFileRecord>` with `(filename, quant, lfs_oid)` for each tracked file.

2. Fetch remote GGUF listing (use cache from Task 2): `pull::list_gguf_files(repo_id).await`

3. For each local file record, check if it exists in the remote listing:
   - If not found remotely → status "removed_from_remote"  
   - If found and LFS hash matches (`local.lfs_oid == remote.lfs_sha256`) → status "up_to_date"
   - If found but LFS hash differs → status "update_available" (store old and new hashes)

4. Build a `details_json` payload as a JSON string:
```json
{
    "repo_id": "unsloth/...",
    "commit_sha": "abc123",
    "quants": [
        {
            "quant_name": "Q4_K_M",
            "filename": "model-Q4_K_M.gguf",
            "current_hash": "sha_old_abc",
            "latest_hash": "sha_new_def",
            "update_available": true,
            "status": "update_available"
        },
        {
            "quant_name": "Q8_0",
            "filename": "model-Q8_0.gguf",
            "current_hash": "sha_old_xyz",
            "latest_hash": "sha_old_xyz",
            "update_available": false,
            "status": "up_to_date"
        }
    ]
}
```

5. Call `save_check_result()` with the new `details_json` string. The model-level record gets:
   - `update_available: true` if ANY quant has `update_available: true`
   - `status`: "update_available" or "up_to_date" accordingly
   - `details_json`: the full per-quant breakdown

The `save_check_result()` function already accepts `Option<&str>` for `details_json`, so no signature change is needed.

**What to implement in `api/updates.rs`:**

Add a helper struct for parsing quant details from JSON (used by `get_updates()`):
```rust
#[derive(Debug, Clone, Deserialize)]
struct QuantDetailJson {
    quant_name: String,
    filename: String,
    current_hash: Option<String>,
    latest_hash: Option<String>,
    update_available: bool,
    status: String,
}
```

In `get_updates()`, after parsing the existing `details_json` blob, extract the quants array:
```rust
let quants: Vec<QuantDetailJson> = details
    .as_ref()
    .and_then(|d| d.get("quants"))
    .and_then(|v| v.as_array())
    .map(|arr| {
        arr.iter()
            .filter_map(|q| serde_json::from_value(q.clone()).ok())
            .collect()
    })
    .unwrap_or_default();
```

Do NOT add a `quants` field to the serialized `UpdateCheckDto`. Keep the DTO shape as-is (just `details_json`). The frontend will parse `details_json` at runtime. This avoids API breaking changes and keeps the backend DTO simple.

**Steps:**
- [ ] Read `crates/koji-core/src/db/queries/model_queries.rs` to confirm `get_model_files(conn, model_id)` returns `Vec<ModelFileRecord>` with fields: `filename`, `quant`, `lfs_oid`
- [ ] In `checker.rs`, rewrite `check_model()` to:
  - Get local file records via `db::queries::get_model_files(&open.conn, model_record.id)?`
  - Fetch remote GGUF listing using cache from Task 2 (`self.gguf_listing_cache.get(repo_id).await`)
  - For each local file, find matching remote file and compare LFS hashes
  - **Also check for new quants:** iterate the remote listing for filenames NOT in local files — add these with `status: "new_quant"`, `update_available: true`
  - Build per-quant results as a `Vec<serde_json::Value>` with keys: `quant_name`, `filename`, `current_hash`, `latest_hash`, `update_available`, `status`
  - Wrap in `{ "repo_id": ..., "commit_sha": ..., "quants": [...] }` JSON object
  - Call `save_check_result()` with the new `details_json` string
- [ ] In `api/updates.rs`, add `QuantDetailJson` struct for parsing (used only internally by `get_updates()`)
- [ ] In `get_updates()`, parse the `details_json.quants` array from each model's details and store it (do NOT add to serialized DTO — frontend parses details_json directly)
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`

**Acceptance criteria:**
- [ ] `check_model()` compares individual file LFS hashes, not just repo commit SHA
- [ ] Per-quant results are stored in `details_json` as `{ "repo_id": ..., "commit_sha": ..., "quants": [...] }` where each quant object has: quant_name, filename, current_hash, latest_hash, update_available, status
- [ ] Model-level `update_available` is true if ANY quant has an update
- [ ] `QuantDetailJson` struct exists for parsing in the API layer
- [ ] Backend `UpdateCheckDto` does NOT have a `quants` field (details_json is the contract)
- [ ] No compilation errors

---

### Task 4: Download Queue Integration for Model/Quant Updates

**Context:**
The current `apply_model_update` endpoint downloads directly via `download_gguf_with_progress()`, bypassing the download queue entirely. This means no progress tracking, no toast notifications, and no centralized view. This task rewrites the endpoint to enqueue each selected quant through `DownloadQueueService::enqueue()` and return immediately with job IDs.

**Files:**
- Modify: `crates/koji-web/src/api/updates.rs`
- Modify: `crates/koji-core/src/db/queries/download_queue_queries.rs` (add duplicate-check query)

**What to implement in `download_queue_queries.rs`:**

Add a function to check for existing queued items with the same repo_id + filename:
```rust
/// Check if there's an active download (queued/running/verifying) for this repo_id + filename.
pub fn get_active_item_by_repo_filename(
    conn: &rusqlite::Connection,
    repo_id: &str,
    filename: &str,
) -> rusqlite::Result<Option<DownloadQueueItem>> {
    use crate::db::queries::download_queue_item_from_row;
    let mut stmt = conn.prepare(
        "SELECT job_id, repo_id, filename, display_name, kind, quant, context_length, \
         status, bytes_downloaded, total_bytes, error_message, started_at, completed_at, queued_at \
         FROM download_queue \
         WHERE repo_id = ? AND filename = ? AND status IN ('queued', 'running', 'verifying') \
         LIMIT 1"
    )?;
    let item = stmt.query_row(
        rusqlite::params![repo_id, filename],
        download_queue_item_from_row,
    ).optional()?;
    Ok(item)
}
```

Export this function in `crates/koji-core/src/db/queries/mod.rs`.

**What to implement in `api/updates.rs`:**

Add request/response DTOs:
```rust
#[derive(Debug, Clone, Deserialize)]
pub struct ModelUpdateRequest {
    pub quants: Vec<String>,  // Quant keys like "Q4_K_M", "Q8_0"
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelUpdateResponse {
    pub job_ids: Vec<String>,
    pub total: usize,
}
```

Rewrite `apply_model_update` handler completely. The current handler does a direct download — replace it with:

```rust
pub async fn apply_model_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<ModelUpdateRequest>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "config_path not configured"}))).into_response(),
    };

    // 1. Resolve model: get repo_id and model files for requested quant keys
    let (repo_id, files_to_update) = tokio::task::spawn_blocking({
        let config_dir = config_dir.clone();
        move || -> anyhow::Result<(String, Vec<(String, String)>)> {
            let open = db::open(&config_dir)?;
            let model_record = queries::get_model_config(&open.conn, id)?
                .ok_or_else(|| anyhow::anyhow!("Model not found"))?;
            let repo_id = model_record.repo_id;
            
            // Get model files for this model
            let model_files = queries::get_model_files(&open.conn, id)?;
            
            // Filter to only the requested quant keys (where quant column matches).
            // Skip files with NULL/None quant — they won't match any requested key.
            let files_to_update: Vec<(String, String)> = model_files
                .into_iter()
                .filter(|f| f.quant.as_ref().is_some_and(|q| req.quants.contains(q)))
                .map(|f| (f.quant.clone().unwrap_or_default(), f.filename))
                .collect();
            
            Ok((repo_id, files_to_update))
        }
    }).await??;

    // 2. Validate: ensure all requested quants exist for this model
    let valid_keys: std::collections::HashSet<&str> = files_to_update.iter().map(|(k, _)| k.as_str()).collect();
    let invalid_quants: Vec<String> = req.quants.iter()
        .filter(|q| !valid_keys.contains(q.as_str()))
        .cloned()
        .collect();
    
    if !invalid_quants.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "Invalid quant keys",
                "invalid_quants": invalid_quants
            }))
        ).into_response();
    }

    // 3. Deduplicate within this request (avoid double-enqueue if same filename appears twice)
    let unique_files: Vec<(String, String)> = files_to_update
        .into_iter()
        .enumerate()
        .filter(|(i, (_, fn))| files_to_update.iter().take(*i).all(|(_, f)| f != fn))
        .map(|(_, pair)| pair)
        .collect();
    
    // 4. Pre-check for duplicate enqueues and enqueue each quant
    let svc = state.download_queue.as_ref().ok_or_else(|| {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "Download queue not configured"})))
    })?;

    let mut job_ids = Vec::new();
    for (quant_key, filename) in &unique_files {
        // Pre-check: return 409 if already queued/running with same repo_id + filename.
        // NOTE: This is a UX fast-path, NOT authoritative. The enqueue() call itself
        // uses INSERT which will fail on UNIQUE constraint violation if there's a race.
        // The pre-check just avoids the error response for the common case.
        // NOTE: `open_conn()` creates an independent SQLite connection. WAL mode should
        // already be enabled in koji-core's db::open() — verify this before implementing.
        match queries::get_active_item_by_repo_filename(&svc.open_conn()?, repo_id, filename) {
            Ok(Some(existing)) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "Download already in progress for this quant",
                        "existing_job_id": existing.job_id
                    }))
                ).into_response();
            }
            Ok(None) => {} // OK to proceed — no duplicate
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("Queue check failed: {}", e)}))
                ).into_response();
            }
        }

        let job_id = uuid::Uuid::new_v4().to_string();
        
        svc.enqueue(
            &job_id,
            repo_id,
            filename,
            Some(&quant_key),  // display_name = quant key
            "model",
            Some(quant_key),
            None,  // context_length — not needed for update downloads
        ).map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()})))
        })?;

        job_ids.push(job_id);
    }

    Json(ModelUpdateResponse { job_ids, total: job_ids.len() }).into_response()
}
```

**Important API change note:** The endpoint now requires a JSON body `{ "quants": [...] }`. Old callers that send an empty body will get 400 Bad Request. This is acceptable since the frontend is being rewritten to use the new format.

**Steps:**
- [ ] Read `crates/koji-core/src/db/queries/download_queue_queries.rs` to confirm the existing function signatures and patterns
- [ ] Add `get_active_item_by_repo_filename(conn, repo_id, filename)` query that returns `Option<DownloadQueueItem>` for items with status IN ('queued', 'running', 'verifying')
- [ ] Export in `crates/koji-core/src/db/queries/mod.rs`
- [ ] In `api/updates.rs`, add `ModelUpdateRequest` and `ModelUpdateResponse` structs
- [ ] Rewrite `apply_model_update` handler: resolve model files → validate quant keys (return 422 for invalid) → pre-check duplicates (return 409 if found) → enqueue each valid quant via `svc.enqueue()` → return `{ "job_ids": [...], "total": N }`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`

**Acceptance criteria:**
- [ ] `get_active_item_by_repo_filename()` query exists and returns items with status IN ('queued', 'running', 'verifying')
- [ ] `apply_model_update` accepts `{ "quants": ["Q4_K_M", "Q8_0"] }` in JSON body
- [ ] Server validates each quant key against model's files, returns 422 for invalid keys with list of invalid ones
- [ ] Deduplicates filenames within a single request before enqueueing
- [ ] Pre-checks download queue for duplicates, returns 409 with `existing_job_id` if found
- [ ] Each valid quant is enqueued through `DownloadQueueService::enqueue()` with correct parameters
- [ ] Returns `{ "job_ids": ["uuid1", ...], "total": 2 }` immediately (async — doesn't wait for downloads)
- [ ] No compilation errors

---

### Task 5: Frontend Per-Quant UI — Expandable List, Checkboxes, Toasts

**Context:**
The Updates Center needs to display per-quant update information and allow users to select which quants to download. This task adds an expandable quant list with checkboxes, a "Update Selected" button, and integrates with toast notifications for download progress feedback. The frontend parses `details_json` from the API response (matching the existing pattern in `get_updates()`).

**Files:**
- Modify: `crates/koji-web/src/pages/updates.rs`
- Note: The route for the new endpoint is already registered in `server.rs` as `.route("/api/updates/apply/model/:id", post(apply_model_update))`. Task 4 updates the handler; this task just uses it from the frontend.

**What to implement:**

1. **Helper function — short SHA display:** Add near other helper functions:
```rust
fn short_sha(hash: &Option<String>) -> String {
    match hash {
        Some(h) => h.chars().take(8).collect(),
        None => "—".to_string(),
    }
}
```

2. **Signals for selection state:** Add after existing signals in the `Updates` component:
```rust
// Tracks which models have their quant list expanded (model_id → bool)
let model_expanded: RwSignal<std::collections::HashMap<String, bool>> =
    RwSignal::new(std::collections::HashMap::new());

// Tracks selected quants per model (model_id → HashSet of quant keys)
let model_selections: RwSignal<std::collections::HashMap<String, std::collections::HashSet<String>>> =
    RwSignal::new(std::collections::HashMap::new());

// Busy state for model update action (model_id → bool)
let model_update_busy = RwSignal::new(Option::<String>::None);
```

3. **Toggle expand handler:** Add near other callbacks:
```rust
let on_toggle_expand = move |model_id: String| {
    model_expanded.update(|map| {
        map.entry(model_id).and_modify(|v| *v = !*v).or_insert(true);
    });
};
```

4. **Update selected quants API call:** Add near other callbacks:
```rust
let on_update_selected = move |model_id: String| {
    // Read selections inside the async block (not before spawn — avoids unused capture)
    model_update_busy.set(Some(model_id.clone()));
    wasm_bindgen_futures::spawn_local(async move {
        let selected_quants: Vec<String> = model_selections.get().get(&model_id)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        
        if selected_quants.is_empty() { return; }
        
        let url = format!("/api/updates/apply/model/{}", model_id);
        match gloo_net::http::Request::post(&url)
            .json(&serde_json::json!({ "quants": selected_quants }))
            .unwrap()
            .send()
            .await {
            Ok(resp) if resp.ok() => {
                // Clear selections for this model, refresh list after delay
                wasm_bindgen_futures::spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    if let Ok(r) = gloo_net::http::Request::get("/api/updates").send().await {
                        if let Ok(data) = r.json::<UpdatesListResponse>().await {
                            updates.set(data);
                        }
                    }
                });
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if status == 409 {
                    error.set(Some(format!("Download already in progress: {}", text)));
                } else if status == 422 {
                    error.set(Some(format!("Invalid quant keys: {}", text)));
                } else {
                    error.set(Some(format!("Update failed: {}", text)));
                }
            }
            Err(e) => error.set(Some(format!("Request failed: {}", e))),
        }
        model_update_busy.set(None);
    });
};
```

6. **Expandable quant list in the models section:** Replace the current simple render of each model with an expandable structure. In the models `<section>`, replace the `models.into_iter().map(|m| ...)` body:

```rust
models.into_iter().map(|m| {
    let model_id = m.item_id.clone();
    let display_name = m.display_name
        .clone()
        .or_else(|| m.repo_id.clone())
        .unwrap_or_else(|| m.item_id.clone());
    
    // Parse quants from details_json (same pattern as get_updates in api/updates.rs)
    let quants_with_updates: Vec<(&str, &str, Option<&str>, Option<&str>, bool)> = m.details_json
        .as_ref()
        .and_then(|d| d.get("quants"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|q| {
                    let quant_name = q["quant_name"].as_str()?;
                    let filename = q["filename"].as_str()?;
                    let current_hash = q["current_hash"].as_str();
                    let latest_hash = q["latest_hash"].as_str();
                    let update_available = q["update_available"].as_bool()?;
                    Some((quant_name, filename, current_hash, latest_hash, update_available))
                })
                .collect()
        })
        .unwrap_or_default();
    
    // Clone for the Select All button closure (the quant rows iterator will move it)
    let quants_for_select: Vec<(&str, &str, Option<&str>, Option<&str>, bool)> = quants_with_updates.clone();
    
    let has_updates = quants_with_updates.iter().any(|(_, _, _, _, u)| *u);
    
    view! {
        <div class="update-item" class:update-available=has_updates>
            {/* Model header with expand/collapse chevron */}
            <div class="update-item__info">
                <span 
                    class="expand-toggle" 
                    style="cursor:pointer;margin-right:0.5rem;font-size:0.75rem;"
                    on:click=move |_| on_toggle_expand(model_id.clone())
                >
                    {move || model_expanded.with(|map| map.get(&model_id).copied().unwrap_or(false)).then(|| "▼".to_string()).unwrap_or_else(|| "▶".to_string())}
                </span>
                <span class="update-item__name">{display_name}</span>
                {/* version info — same as before */}
                {m.current_version.as_ref().map(|v| view! {
                    <span class="update-item__version">
                        {&v[..8.min(v.len())]}
                    </span>
                })}
                {if has_updates {
                    let latest = m.latest_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("").to_string();
                    view! {
                        <span class="update-badge">
                            {format!(" → {}", latest)}
                        </span>
                    }.into_any()
                } else {
                    view! { <span class="up-to-date-badge">{"✓ Up to date"}</span> }.into_any()
                }}
            </div>
            
            {/* Expandable quant list */}
            {move || model_expanded.with(|map| map.get(&model_id).copied().unwrap_or(false)).then(|| {
                let selections = model_selections.clone();
                let mid = model_id.clone();
                view! {
                    <div class="quant-list" style="margin-top:0.5rem;padding-left:1.5rem;">
                        {/* Select All / Deselect All */}
                        <div style="display:flex;gap:0.5rem;margin-bottom:0.5rem;">
                            <button 
                                class="btn btn-ghost btn-sm" 
                                style="font-size:0.75rem;padding:0.125rem 0.5rem;"
                                on:click=move |_| {
                                    // Select all updatable quants (uses cloned data to avoid move conflict)
                                    model_selections.update(|map| {
                                        let set: std::collections::HashSet<String> = quants_for_select
                                            .iter()
                                            .filter(|(_, _, _, _, u)| *u)
                                            .map(|(k, _, _, _, _)| k.to_string())
                                            .collect();
                                        map.insert(mid.clone(), set);
                                    });
                                }
                            >
                                "Select All"
                            </button>
                        </div>
                        
                        {/* Quant rows */}
                        {quants_with_updates.into_iter().map(|(quant_name, filename, current_hash, latest_hash, update_available)| {
                            let qn = quant_name.to_string();
                            let mid_for_sel = model_id.clone();
                            let is_selected = move || {
                                selections.with(|map| map.get(&model_id)
                                    .map(|set| set.contains(&qn)).unwrap_or(false))
                            };
                            view! {
                                <label class="quant-item" style="display:flex;align-items:center;gap:0.5rem;padding:0.25rem 0;">
                                    <input 
                                        type="checkbox" 
                                        prop:checked=is_selected()
                                        disabled={!update_available}
                                        on:change=move |e| {
                                            use wasm_bindgen::JsCast;
                                            let checked = e.target()
                                                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                                                .map(|el| el.checked())
                                                .unwrap_or(false);
                                            if checked {
                                                model_selections.update(|map| {
                                                    map.entry(mid_for_sel.clone())
                                                        .or_insert_with(std::collections::HashSet::new)
                                                        .insert(qn.clone());
                                                });
                                            } else {
                                                model_selections.update(|map| {
                                                    if let Some(set) = map.get_mut(&mid_for_sel) {
                                                        set.remove(&qn);
                                                    }
                                                });
                                            }
                                        }
                                    />
                                    <span style="font-weight:500;">{quant_name}</span>
                                    <span class="text-muted" style="font-size:0.75rem;">{short_sha(&current_hash.map(String::from))}</span>
                                    <span style="color:#94a3b8;">→</span>
                                    <span class="text-muted" style="font-size:0.75rem;">{short_sha(&latest_hash.map(String::from))}</span>
                                    {if update_available {
                                        view! { <span class="badge" style="background:#f59e0b;color:white;padding:0.125rem 0.375rem;border-radius:4px;font-size:0.625rem;">"Update"</span> }.into_any()
                                    } else {
                                        view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.375rem;border-radius:4px;font-size:0.625rem;">"Up to date"</span> }.into_any()
                                    }}
                                </label>
                            }.into_any()
                        }).collect::<Vec<_>>()}
                        
                        {/* Update Selected button */}
                        <button 
                            class="btn btn-primary btn-sm" 
                            style="margin-top:0.5rem;"
                            disabled=move || {
                                model_update_busy.with(|b| b.as_ref().map(|id| id == &model_id).unwrap_or(false))
                                    || model_selections.with(|map| map.get(&model_id)
                                        .map(|set| set.is_empty()).unwrap_or(true))
                            }
                            on:click=move |_| on_update_selected(model_id.clone())
                        >
                            {move || model_update_busy.with(|b| b.as_ref().map(|id| id == &model_id).unwrap_or(false))
                                .then(|| "Updating...".to_string())
                                .unwrap_or_else(|| "Update Selected".to_string())}
                        </button>
                    </div>
                }.into_any()
            })}
            
            {/* Legacy action buttons — keep for backward compat */}
            <div class="update-item__actions">
                {if has_updates {
                    let id = m.item_id.clone();
                    view! {
                        <button class="btn btn-secondary"
                            on:click=move |_| wasm_bindgen_futures::spawn_local(async move {
                                let url = format!("/api/models/{}/refresh", id);
                                let _ = gloo_net::http::Request::post(&url).send().await;
                            })>
                            "Refresh Metadata"
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
                <a href=format!("/models/{}", m.item_id) class="btn btn-ghost">
                    "Edit"
                </a>
            </div>
        </div>
    }.into_any()
}).collect::<Vec<_>>()
```

**Steps:**
- [ ] Add `short_sha()` helper function
- [ ] Add three new signals: `model_expanded` (HashMap<String, bool>), `model_selections` (HashMap<String, HashSet<String>>), `model_update_busy` (Option<String>)
- [ ] Add `on_toggle_expand`, `on_toggle_quant_selection`, and `on_update_selected` handlers
- [ ] In the models section view, replace the simple render with expandable quant list:
  - Model header has chevron (▶/▼) that toggles expansion
  - When expanded, shows all quants from `details_json.quants` array
  - Each quant row: checkbox (disabled if up-to-date), quant name, short SHA display (current → latest), status badge
  - "Select All" button selects all updatable quants for the model
  - "Update Selected" button calls `POST /api/updates/apply/model/:id` with `{ "quants": [...] }`
  - Shows loading state ("Updating...") when busy
- [ ] Error handling: 409 → "Download already in progress", 422 → "Invalid quant keys"
- [ ] After successful update, clear selections and refresh updates list after 2s delay
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`

**Acceptance criteria:**
- [ ] Updates Center shows per-quant update info for each model with available updates
- [ ] Clicking chevron (▶/▼) expands/collapses the quant list per model
- [ ] Each quant has a checkbox; up-to-date quants are shown but checkbox is disabled
- [ ] "Select All" selects all updatable quants for that model
- [ ] "Update Selected" button sends only selected quants to the API
- [ ] Toast notification appears when download starts (via existing SSE + ToastStore)
- [ ] Progress visible in Downloads Center (existing SSE events update global ACTIVE_DOWNLOADS signal)
- [ ] Error handling for 409 (conflict) and 422 (invalid quant keys) shown as error banner
- [ ] No compilation errors

---

## Task Dependency Order

1. **Task 1** — No dependencies, can start immediately (quick win)
2. **Task 2** — No dependencies on other tasks, but needed by Task 3 for efficiency
3. **Task 3** — Depends on Task 2 (cache), produces data consumed by Tasks 4 and 5
4. **Task 4** — Depends on Task 3 (knows what quant keys to enqueue via model_files)
5. **Task 5** — Depends on Task 3 (needs per-quant data in details_json from API) and Task 4 (needs working endpoint)

## Risk Mitigations

- **API rate limits:** Task 2's LRU cache (64 entries, 5-min TTL) reduces HuggingFace calls by ~80% for models sharing the same repo
- **Duplicate enqueues:** Task 4 pre-checks download queue via `get_active_item_by_repo_filename()`, returns 409 with existing job_id
- **Partial failures:** Each quant downloads independently; failed quants don't block others. The frontend shows per-quant status, not model-level.
- **Frontend stability:** All reactive closures use captured strings (not references) to avoid lifetime issues in Leptos
- **Backward compatibility:** Old `details_json` format is still valid JSON — the frontend just parses the new `quants` array if present, falls back gracefully
