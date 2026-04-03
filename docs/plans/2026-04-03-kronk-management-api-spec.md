# Kronk Management API — Spec

**Date:** 2026-04-03  
**Status:** Draft  
**Prefix:** `/kronk/v1/`

## OpenAPI Specs

The machine-readable OpenAPI 3.1.0 specs for all Kronk endpoints live in [`docs/openapi/`](../openapi/):

| File | Covers |
|---|---|
| [`docs/openapi/kronk-api.yaml`](../openapi/kronk-api.yaml) | All `/kronk/v1/` management endpoints defined in this spec |
| [`docs/openapi/openai-compat.yaml`](../openapi/openai-compat.yaml) | `/v1/` chat & model routes + `/health`, `/status`, `/metrics` |

> **The OpenAPI specs are the authoritative request/response contract.** The endpoint descriptions
> below document intent and implementation detail; the YAML files define the exact schemas.

---

## Background

Kronk exposes an OpenAI-compatible API under `/v1/` (chat completions, model listing). It also has
`/health`, `/status`, and `/metrics` at the root. What it lacks is an explicit **management plane**
for operations like loading/unloading model backends, pulling models from HuggingFace, and
restarting the proxy.

This spec defines a new `GET`/`POST` surface under `/kronk/v1/` that is clearly Kronk-native and
not part of the OpenAI spec.

---

## Guiding Principles

1. **No breakage.** Existing `/health`, `/status`, `/metrics`, and `/v1/...` routes are untouched.
2. **Explicit versioning.** `/kronk/v1/` allows a future `/kronk/v2/` without path conflict.
3. **Long ops are async.** Pull/download is async: POST returns a `job_id`, poll for progress.
4. **Consistent error shape.** All errors return `{ "error": { "message": "...", "type": "..." } }`.
5. **Thin handlers.** Handlers call existing `ProxyState` methods. No business logic in HTTP layer.

---

## Endpoints

### 1. Model Management

#### `GET /kronk/v1/models`

List all configured models with their runtime state.

**Response 200:**
```json
{
  "models": [
    {
      "id": "llama3",
      "backend": "llama-cpp",
      "model": "meta-llama/Meta-Llama-3-8B-Instruct",
      "quant": "Q4_K_M",
      "enabled": true,
      "loaded": true,
      "backend_pid": 12345,
      "load_time_secs": 1712000000,
      "last_accessed_secs_ago": 42,
      "idle_timeout_remaining_secs": 258,
      "consecutive_failures": 0
    },
    {
      "id": "mistral",
      "backend": "llama-cpp",
      "model": "mistralai/Mistral-7B-v0.1",
      "quant": "Q5_K_M",
      "enabled": true,
      "loaded": false,
      "backend_pid": null,
      "load_time_secs": null,
      "last_accessed_secs_ago": null,
      "idle_timeout_remaining_secs": null,
      "consecutive_failures": null
    }
  ]
}
```

**Notes:** This is richer than `/v1/models` (which is OpenAI-spec only) and mirrors the per-model
data already produced by `build_status_response`. The array format (vs the keyed-object format
in `/status`) is intentional: it is pagination-friendly and matches the OpenAI `/v1/models` shape
that clients already understand.

---

#### `GET /kronk/v1/models/:id`

Get runtime state for a single configured model.

**Response 200:** Single model object (same shape as above).  
**Response 404:** `{ "error": { "message": "Model 'foo' not found", "type": "NotFoundError" } }`

---

#### `POST /kronk/v1/models/:id/load`

Start the backend process for a configured model.

**Request body:** empty (or `{}`)

**Response 200:**
```json
{ "id": "llama3", "loaded": true }
```

**Response 404:** model not configured.  
**Response 500:** backend failed to start.

**Note:** Load is idempotent. If the model is already loaded or starting, the handler returns 200
immediately without re-launching the backend (mirrors the existing `load_model` behaviour in
`lifecycle.rs`).

**Implementation:**
```rust
let model_card = state.get_model_card(&id).await;
state.load_model(&id, model_card.as_ref()).await
```

`load_model` blocks until the backend health check passes (or times out), bounded by
`proxy.startup_timeout_secs`.

---

#### `POST /kronk/v1/models/:id/unload`

Gracefully stop the backend for a loaded model.

**Request body:** empty

**Response 200:**
```json
{ "id": "llama3", "loaded": false }
```

**Response 404:** model not configured or not currently loaded.  
**Response 500:** failed to stop backend.

**Implementation:** `unload_model` is keyed by *server name* (the config key), not user-facing
model name. The handler resolves the server name first:
```rust
let server_name = state.get_available_server_for_model(&id).await
    .ok_or(/* 404 */)?;
state.unload_model(&server_name).await
```

> **Why POST, not DELETE?** POST is used for both load and unload because these are state
> transitions, not resource deletions. Using `POST .../load` / `POST .../unload` as action verbs
> is intentional and leaves room to add a request body with options (e.g. `force: true`) later.

---

### 2. Pull / Download

> **Route note:** Pull jobs live under `/kronk/v1/pulls/` (not `/kronk/v1/models/pull/`) to avoid
> an axum path conflict between the literal segment `pull` and the `:id` capture in
> `/kronk/v1/models/:id`.

#### `POST /kronk/v1/pulls`

Start an async download of a GGUF model from HuggingFace.

**Request body:**
```json
{
  "repo_id": "Tesslate/OmniCoder-9B-GGUF",
  "filename": "OmniCoder-9B-Q4_K_M.gguf"
}
```

`filename` is optional. If omitted, the server will list available GGUFs (`list_gguf_files`) and
pick a sensible default (e.g. Q4_K_M if available, otherwise first file alphabetically).

**Response 202 Accepted:**
```json
{
  "job_id": "pull-abc123",
  "status": "pending",
  "repo_id": "Tesslate/OmniCoder-9B-GGUF",
  "filename": "OmniCoder-9B-Q4_K_M.gguf",
  "bytes_downloaded": 0,
  "total_bytes": null,
  "error": null
}
```

**Implementation notes:**
- A `PullJobStore` (`Arc<RwLock<HashMap<String, PullJob>>>`) is added to `ProxyState`.
- The handler inserts a `PullJob { status: Pending, .. }` and spawns a `tokio::spawn` task.
- The task calls `download_gguf(repo_id, filename, dest_dir)`. On completion it updates
  `PullJob.status` to `Completed` (with `bytes_downloaded = size_bytes` from `DownloadResult`) or
  `Failed { error }`.
- **Progress caveat:** `download_gguf` / `download_chunked` have no external progress callback.
  `bytes_downloaded` will be `0` while running and populated only upon completion. A future
  iteration can refactor `download_chunked` to accept an `Arc<AtomicU64>` progress counter.
  For now the poll endpoint reports `pending` → `running` → `completed`/`failed` states only.
- `job_id` is `"pull-" + uuid_v4`.
- **Eviction:** Completed and failed jobs are retained in memory for 1 hour from completion, then
  removed by a background cleanup task that runs every 5 minutes.

---

#### `GET /kronk/v1/pulls/:job_id`

Poll the status of an in-progress or completed pull job.

**Response 200:**
```json
{
  "job_id": "pull-abc123",
  "status": "running",
  "repo_id": "Tesslate/OmniCoder-9B-GGUF",
  "filename": "OmniCoder-9B-Q4_K_M.gguf",
  "bytes_downloaded": 0,
  "total_bytes": null,
  "error": null
}
```

`status` values: `"pending"` | `"running"` | `"completed"` | `"failed"`

**Note:** `bytes_downloaded` and `total_bytes` are only meaningful on completion (see progress
caveat above). Pollers should track status transitions, not byte counts.

**Response 404:** job not found.

---

### 3. System

#### `GET /kronk/v1/system/health`

Richer health check than the root `/health` liveness probe. Returns overall system health
including VRAM and currently loaded model count.

**Response 200:**
```json
{
  "status": "ok",
  "service": "kronk",
  "models_loaded": 2,
  "vram": {
    "used_mib": 8192,
    "total_mib": 16384
  }
}
```

**Notes:** Root `/health` is unchanged (returns `{ "status": "ok", "service": "kronk-proxy" }`)
and continues to serve as a lightweight liveness probe.

---

#### `POST /kronk/v1/system/restart`

Trigger a graceful restart of the kronk proxy process.

**Request body:** empty

**Response 200:**
```json
{ "message": "Restarting kronk..." }
```

**Implementation notes:**
- The handler sends the response, then spawns a short-delay task (`tokio::time::sleep(500ms)`)
  before triggering shutdown.
- Shutdown is triggered by sending `SIGTERM` to self (`libc::kill(libc::getpid(), libc::SIGTERM)`)
  so the normal signal handler (if registered) runs destructors and flushes I/O. Avoids
  `std::process::exit(0)` which skips destructors.
- Assumes kronk is managed by a process supervisor (systemd, launchd, etc.) that will restart it.
- A `force` query param (`?force=true`) skips graceful model unloading; default is graceful (allow
  in-flight requests to drain and unload all models before exiting).

---

## Router Changes

The new routes are added to `build_router` in `crates/kronk-core/src/proxy/server/router.rs`:

```rust
// Kronk management API — model lifecycle
.route("/kronk/v1/models",                    get(handle_kronk_list_models))
.route("/kronk/v1/models/:id",                get(handle_kronk_get_model))
.route("/kronk/v1/models/:id/load",           post(handle_kronk_load_model))
.route("/kronk/v1/models/:id/unload",         post(handle_kronk_unload_model))
// Pull jobs live under /kronk/v1/pulls/ to avoid path conflict with /models/:id
.route("/kronk/v1/pulls",                     post(handle_kronk_pull_model))
.route("/kronk/v1/pulls/:job_id",             get(handle_kronk_get_pull_job))
// System
.route("/kronk/v1/system/health",             get(handle_kronk_system_health))
.route("/kronk/v1/system/restart",            post(handle_kronk_system_restart))
```

Handlers live in a new file: `crates/kronk-core/src/proxy/kronk_handlers.rs`.

---

## State Changes

`ProxyState` in `crates/kronk-core/src/proxy/types.rs` gains one new field:

```rust
pub pull_jobs: Arc<RwLock<HashMap<String, PullJob>>>,
```

A new file `crates/kronk-core/src/proxy/pull_jobs.rs` defines:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct PullJob {
    pub job_id: String,
    pub repo_id: String,
    pub filename: String,
    pub status: PullJobStatus,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub error: Option<String>,
    /// Set when status transitions to Completed or Failed; used for eviction.
    pub completed_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PullJobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
```

A background cleanup task (spawned in `ProxyState::new`) runs every 5 minutes and removes jobs
where `completed_at` is older than 1 hour.

---

## File Changes Summary

| File | Change |
|---|---|
| `crates/kronk-core/src/proxy/server/router.rs` | Add 8 new routes |
| `crates/kronk-core/src/proxy/kronk_handlers.rs` | New file: all 8 handler functions |
| `crates/kronk-core/src/proxy/pull_jobs.rs` | New file: `PullJob` + `PullJobStatus` types |
| `crates/kronk-core/src/proxy/types.rs` | Add `pull_jobs` field to `ProxyState` |
| `crates/kronk-core/src/proxy/state.rs` | Init `pull_jobs` in `ProxyState::new` |
| `crates/kronk-core/src/proxy/mod.rs` | `pub mod` for new files |

---

## Out of Scope (for this iteration)

- Authentication / API key gating on management endpoints
- Pagination on model or job lists
- WebSocket progress streaming for pull (SSE or polling is sufficient)
- Pull from sources other than HuggingFace
