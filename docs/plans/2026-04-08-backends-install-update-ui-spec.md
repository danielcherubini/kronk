# Backends Install / Update UI — Spec

**Date:** 2026-04-08
**Status:** Draft
**Scope:** Web UI + HTTP API + minimal `koji-core` changes to support installing, updating, and uninstalling the two known inference backends (`llama_cpp`, `ik_llama`) from the Koji web config page, with live build/download progress.

---

## 1. Motivation

Today the "Backends" section in the web config page (`crates/koji-web/src/pages/config_editor.rs::BackendsForm`) only edits the `config.backends` TOML table: `path`, `default_args`, `health_check_url`, and `version` pin. It has no awareness of whether a backend is actually installed, what version is installed, whether an update is available, or how to install one.

Meanwhile the CLI (`crates/koji-cli/src/commands/backend.rs`) has a full `install / update / list / remove / check-updates` flow backed by `koji_core::backends` (`install_backend`, `check_latest_version`, `check_updates`, `update_backend`, `BackendRegistry`).

This spec surfaces that flow in the web UI so users never have to drop to the terminal to install or update a backend.

---

## 2. Goals

- Surface the two known backends (`llama_cpp`, `ik_llama`) as cards in the existing Backends section.
- Reflect live install state from the `BackendRegistry` (sled DB) in each card.
- Provide Install, Update, Uninstall, and Release-notes actions per card.
- Show live build/download logs during install and update jobs via SSE.
- Reuse the existing `config.backends` TOML editing for `default_args`, `health_check_url`, `version` pin, and (optional) custom path — moved under an "Advanced" disclosure on each card.
- Keep the CLI's behavior unchanged.

## 3. Non-goals (YAGNI)

- Custom backend types (`BackendType::Custom`) are not surfaced in the UI.
- Commit pinning (`--commit`) is CLI-only.
- Custom install names (`--name`) are CLI-only; the UI always installs to the default name matching the backend type.
- Persistent job history across process restarts — jobs live in memory only.
- Concurrent installs — a single in-flight install/update job is allowed at a time.
- Markdown rendering of release notes — external links only.
- Auto-check-for-updates on page load — manual button only.
- Caching of GitHub responses — `check-updates` is user-initiated and infrequent.

---

## 4. Data model & sources of truth

Two existing stores are joined by the UI:

| Store | Write path | Fields |
|---|---|---|
| `config.backends` (`BTreeMap<String, BackendConfig>` in `config.toml`) | Existing `POST /api/config/structured` | `path`, `default_args`, `health_check_url`, `version` (pin) |
| `BackendRegistry` (sled DB at `<base_dir>/backends/registry`) | New backend API endpoints | `BackendInfo` (`name`, `backend_type`, `version`, `path`, `installed_at`, `gpu_type`, `source`) |

The UI displays **two cards**, keyed by the hard-coded set `{ llama_cpp, ik_llama }`:

- Registry has an entry for this name → "Installed" state.
- Registry has no entry → "Not installed" state.

Config-side fields continue to live in `config.backends` and are edited via the existing structured-config save flow from within an "Advanced" disclosure on each card. **No TOML schema changes.**

A new in-memory `JobManager` lives in `AppState` to track install/update jobs.

---

## 5. HTTP API surface

All new routes live under `/api/backends` and `/api/system`. Errors follow the existing `{ "error": "..." }` + appropriate status convention.

### 5.1 Read endpoints

**`GET /api/system/capabilities`** — system/GPU detection snapshot.
```json
{
  "os": "linux",
  "arch": "x86_64",
  "git_available": true,
  "cmake_available": true,
  "compiler_available": true,
  "detected_cuda_version": "12.4",
  "detected_rocm_version": null
}
```
Backed by `koji_core::gpu::detect_build_prerequisites()` + `detect_cuda_version()`. Cheap, no caching.

**`GET /api/backends`** — joined view of known backends.
```json
{
  "active_job": { "id": "j_abc123", "kind": "install", "backend_type": "llama_cpp" },
  "backends": [
    {
      "type": "llama_cpp",
      "display_name": "llama.cpp",
      "installed": true,
      "info": {
        "name": "llama_cpp",
        "version": "b8407",
        "path": "/home/u/.koji/backends/llama_cpp/llama-server",
        "installed_at": 1733000000,
        "gpu_type": { "kind": "cuda", "version": "12.4" },
        "source": { "kind": "prebuilt", "version": "b8407" }
      },
      "update": { "checked": false, "latest_version": null, "update_available": null },
      "release_notes_url": "https://github.com/ggml-org/llama.cpp/releases"
    },
    {
      "type": "ik_llama",
      "display_name": "ik_llama.cpp",
      "installed": false,
      "info": null,
      "update": { "checked": false, "latest_version": null, "update_available": null },
      "release_notes_url": "https://github.com/ikawrakow/ik_llama.cpp/commits/main"
    }
  ]
}
```

Notes:
- `update` is left unfilled here — the UI calls `/check-updates` explicitly to avoid hitting GitHub on every page load.
- `active_job` lets the UI rehydrate a running job after a page reload without polling.

**`POST /api/backends/check-updates`** (empty body) — calls `check_updates` for each installed backend. Synchronous (~1s). Returns the same shape as `GET /api/backends`, with `update` populated for installed backends.

### 5.2 Mutation endpoints

**`POST /api/backends/install`**
```json
{
  "backend_type": "llama_cpp",
  "version": null,
  "gpu_type": { "kind": "cuda", "version": "12.4" },
  "build_from_source": false,
  "force": false
}
```
Returns `202 Accepted`:
```json
{ "job_id": "j_abc123", "kind": "install", "backend_type": "llama_cpp" }
```

For `ik_llama`, `build_from_source` is forced to `true` server-side regardless of request.

Returns `409 Conflict` if another install/update job is already running:
```json
{ "error": "another backend job is already running", "job_id": "j_existing" }
```

**`POST /api/backends/:name/update`** (empty body) — reuses the existing registry's `gpu_type` and `source` (mirroring CLI `cmd_update`). Returns the same shape as install. 409 on concurrent job.

**`DELETE /api/backends/:name`** — synchronous. Removes registry entry **and** binary files, reusing the CLI's canonical-path safety check (only paths under the managed `backends_dir()` are deleted). Returns:
```json
{ "removed": true }
```
or 404 on unknown name.

### 5.3 Job endpoints

**`GET /api/backends/jobs/:id`** — current snapshot.
```json
{
  "id": "j_abc123",
  "kind": "install",
  "status": "running",
  "backend_type": "llama_cpp",
  "started_at": 1733000000,
  "finished_at": null,
  "error": null
}
```
`status` ∈ `queued | running | succeeded | failed`.

**`GET /api/backends/jobs/:id/events`** — SSE stream.
```
event: log
data: {"line": "Cloning into 'llama.cpp'..."}

event: status
data: {"status": "succeeded"}
```
On connect: replay the buffered log lines (bounded, ~500) then attach to the live `tokio::sync::broadcast` channel. If the job is already terminal at connect time, send the buffered lines + final status, then close. Otherwise close when a terminal `status` event is sent.

### 5.4 Router changes

In `crates/koji-web/src/server.rs::build_router`:

```
.route("/api/system/capabilities", get(api::system_capabilities))
.route("/api/backends", get(api::list_backends))
.route("/api/backends/check-updates", post(api::check_backend_updates))
.route("/api/backends/install", post(api::install_backend))
.route("/api/backends/:name/update", post(api::update_backend))
.route("/api/backends/:name", delete(api::remove_backend))
.route("/api/backends/jobs/:id", get(api::get_job))
.route("/api/backends/jobs/:id/events", get(api::job_events_sse))
```

---

## 6. Job manager & progress streaming

### 6.1 `JobManager` (`crates/koji-web/src/jobs.rs`)

```rust
pub struct JobManager {
    inner: Arc<RwLock<HashMap<JobId, Arc<Job>>>>,
    // Enforces single in-flight job policy.
    active: Arc<Mutex<Option<JobId>>>,
}

pub struct Job {
    pub id: JobId,
    pub kind: JobKind,                 // Install | Update
    pub backend_type: BackendType,
    pub state: RwLock<JobState>,
    pub log_buffer: RwLock<VecDeque<String>>,  // bounded, 500 lines
    pub log_tx: broadcast::Sender<JobEvent>,    // capacity ~256
}

pub struct JobState {
    pub status: JobStatus,             // Queued | Running | Succeeded | Failed
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

pub enum JobKind { Install, Update }
pub enum JobEvent { Log(String), Status(JobStatus) }
```

**Policy:**
- **Single in-flight job.** Second submission while one is running returns `JobError::AlreadyRunning(existing_id)` → 409 from the handler.
- **Retention.** Finished jobs are kept so the UI can still fetch their final log. FIFO eviction keeps at most **8 finished jobs**.
- **Log buffer.** Each `Job` keeps at most **500 lines** (`VecDeque::pop_front` on overflow). This is the replay buffer for late SSE subscribers.
- **Broadcast channel.** Live tailing. Capacity ~256 events; slow subscribers receive `broadcast::error::RecvError::Lagged` and are handled gracefully (log a warning line, continue).
- **No persistence** across process restarts.

`JobManager` is added to `AppState` as `pub jobs: Arc<JobManager>`.

### 6.2 Progress propagation from `koji-core`

One minimal change to `koji-core`: a `ProgressSink` trait so the installer/updater can emit progress lines without `println!`.

```rust
// crates/koji-core/src/backends/mod.rs
pub trait ProgressSink: Send + Sync {
    fn log(&self, line: &str);
}

pub struct StdoutSink;
impl ProgressSink for StdoutSink {
    fn log(&self, line: &str) { println!("{line}"); }
}
```

`InstallOptions` gains an optional field (not serialized):

```rust
pub struct InstallOptions {
    // ... existing fields ...
    #[cfg_attr(feature = "serde", serde(skip))]
    pub progress: Option<Arc<dyn ProgressSink>>,
}
```

Internally the installer routes milestone lines and captured child-process stdout/stderr through the sink. If `progress` is `None`, the existing `println!` behavior is preserved (CLI path unchanged).

**Scope guard:** we do not rewrite every `println!` in one pass. We route:
1. High-level milestones in `install_backend` ("Cloning…", "Building…", "Downloading…", "Extracting…", "Installation complete").
2. Child-process output from `git clone`, `cmake`, `make`/`ninja`, and the prebuilt download progress.
3. Extract-step output.

Anything missed still goes to stdout/stderr unchanged (captured by the koji log file).

**Child-process capture** replaces the current inherited stdio on spawned commands (`git`, `cmake`, `make`/`ninja`) with piped stdout/stderr, line-buffered readers that forward each line via `progress.log(...)`. Exit-code error handling is preserved.

**Prebuilt download progress** is throttled to at most one emitted line per ~250 ms to avoid flooding the log.

### 6.3 SSE handler

```rust
async fn job_events_sse(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Sse<...>, StatusCode> {
    let job = state.jobs.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let mut rx = job.log_tx.subscribe();
    let buffered: Vec<String> = job.log_buffer.read().await.iter().cloned().collect();
    let terminal_at_connect = {
        let s = job.state.read().await;
        matches!(s.status, JobStatus::Succeeded | JobStatus::Failed).then_some(s.status)
    };

    let stream = async_stream::stream! {
        for line in buffered {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line }))?);
        }
        if let Some(status) = terminal_at_connect {
            yield Ok(Event::default().event("status").json_data(json!({ "status": status }))?);
            return;
        }
        loop {
            match rx.recv().await {
                Ok(JobEvent::Log(line)) => {
                    yield Ok(Event::default().event("log").json_data(json!({ "line": line }))?);
                }
                Ok(JobEvent::Status(status)) => {
                    yield Ok(Event::default().event("status").json_data(json!({ "status": status }))?);
                    if matches!(status, JobStatus::Succeeded | JobStatus::Failed) { return; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
```

Uses `axum::response::sse::Sse`. Replay-on-connect lets clients reconnect mid-install without losing context.

---

## 7. UI structure

Lives in `crates/koji-web/src/pages/config_editor.rs::BackendsForm` (replaced). The unused `crates/koji-web/src/components/backends_section.rs` is deleted.

### 7.1 Layout

```
┌─ Backends ──────────────────────────────────────────────────┐
│  Inference backend binaries.                                │
│  [ Check for updates ]                                      │
│                                                             │
│  ┌─ llama.cpp ───────────────────── [Installed: b8407] ──┐  │
│  │  GPU: CUDA 12.4    Source: prebuilt                   │  │
│  │                                                       │  │
│  │  ⓘ Update available → b8410   [Update]  [Notes ↗]  ⋮ │  │
│  │                                                       │  │
│  │  ▸ Advanced (path / args / health URL / version pin)  │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│  ┌─ ik_llama.cpp ─────────────────── [Not installed] ────┐  │
│  │  ik_llama builds from source from main.               │  │
│  │  [ Install ]                                [Notes ↗] │  │
│  └───────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

### 7.2 Card states

| State | Header badge | Body | Primary actions |
|---|---|---|---|
| Not installed | `Not installed` (gray) | One-line description | `Install`, `Notes ↗` |
| Installed, no check | `Installed: <ver>` (blue) | GPU + source summary | `Update`, `Notes ↗`, `⋮` (Uninstall) |
| Installed, up-to-date | `Installed: <ver>` (green ✓) | "Up to date" line | `Notes ↗`, `⋮` |
| Update available | `Update available` (amber) | "Current b8407 → Latest b8410" | `Update`, `Notes ↗`, `⋮` |
| Job running | `Installing…` / `Updating…` (spinner) | Embedded log tail (~6 lines, monospaced) + "View full log" expander | Buttons disabled |
| Job failed | `Install failed` (red) | Error summary + log tail | `Retry`, `Dismiss` |

### 7.3 Advanced disclosure

Inside each installed card, under a collapsible "Advanced" section, the existing `config.backends[name]` fields remain editable via the existing structured-config flow:

- **Path** — read-only by default (auto-populated from the registry). Editable with a "Reset to managed path" button for users with custom binaries.
- **Default args** — free text.
- **Health check URL** — free text.
- **Version pin** — free text.

### 7.4 Install modal (`install_modal.rs`)

Opens on Install click. Fetches `/api/system/capabilities` on open.

Fields:
- **GPU acceleration** — radio group (CUDA / ROCm / Vulkan / Metal / CPU). Default selected from detection.
- **CUDA version** dropdown — shown only when CUDA selected. Options: `11.1`, `12.4`, `13.1`. Default from `detected_cuda_version`.
- **ROCm version** dropdown — shown only when ROCm selected. Options: `5.7`, `6.1`.
- **Version** — free text, placeholder `latest`. Hidden for `ik_llama`.
- **Build from source** checkbox. Forced on + disabled for `ik_llama` with help text "ik_llama always builds from source".
- **Force overwrite** checkbox.

Warning banner shown at the top when `cmake_available` / `git_available` / `compiler_available` is false **and** source build is selected (forced for `ik_llama`):
> ⚠ cmake not found — source build will fail.

Buttons: `Cancel`, `Install`. Submitting POSTs `/api/backends/install`, closes the modal, transitions the card to "Job running" state.

### 7.5 Job-running flow

1. POST returns `{ job_id }`.
2. Card renders in Job-running state and opens `new EventSource('/api/backends/jobs/<id>/events')`.
3. `event: log` appends lines to a bounded ring (inline shows last 6, expandable full panel shows up to ~500).
4. `event: status` terminal → close EventSource, refresh `GET /api/backends`, card rerenders with new state.
5. On page reload mid-install, `GET /api/backends` returns `active_job`; UI re-attaches the SSE stream immediately.

### 7.6 Update flow

Clicking **Update** does **not** open a modal. The request reuses the existing registry's `gpu_type` and `source` (matching CLI `cmd_update`). It POSTs immediately and the card transitions to Job-running state.

### 7.7 Uninstall flow

Kebab menu `⋮` → Uninstall → confirm modal:
> Remove **llama.cpp** (b8407)?
> This will delete the registry entry and binary files at `<path>`.
> `Cancel`  `Remove`

DELETE `/api/backends/:name`. Synchronous. On success, refresh the card.

### 7.8 Check for updates

Top-right of the Backends section. POSTs `/api/backends/check-updates`, populates each card's `update` field, updates the badge. Manual only.

---

## 8. File-level changes

### 8.1 New files

| File | Purpose |
|---|---|
| `crates/koji-web/src/jobs.rs` | `JobManager`, `Job`, `JobState`, `JobEvent`, `JobStatus`, `JobKind`, single-job policy, FIFO eviction |
| `crates/koji-web/src/api/backends.rs` (or `api_backends.rs`) | New API handlers: `list_backends`, `check_backend_updates`, `install_backend`, `update_backend`, `remove_backend`, `get_job`, `job_events_sse`, `system_capabilities` |
| `crates/koji-web/src/components/backend_card.rs` | Per-backend Leptos card with state badges, advanced disclosure, action buttons |
| `crates/koji-web/src/components/install_modal.rs` | Install modal Leptos component |
| `crates/koji-web/src/components/job_log_panel.rs` | Reusable embedded log-tail / expandable panel that consumes an `EventSource` |

### 8.2 Modified files

**`crates/koji-core/src/backends/mod.rs`**
- Add `pub trait ProgressSink` and `pub struct StdoutSink`.
- Re-export from `koji_core::backends`.

**`crates/koji-core/src/backends/installer/mod.rs`**
- Add `progress: Option<Arc<dyn ProgressSink>>` to `InstallOptions` (skipped from any serde).
- Internal `emit!` helper / function that routes to sink or `println!`.
- Replace top-level milestone `println!`s with `emit!(...)`.

**`crates/koji-core/src/backends/installer/source.rs`**
- Replace inherited stdio on `git`, `cmake`, `make`/`ninja` with piped stdout/stderr.
- Line-reader tasks forward each line through the progress sink.
- Preserve existing exit-code error handling.

**`crates/koji-core/src/backends/installer/prebuilt.rs`**
- Route download progress reporter through the sink, throttled to ~1 line per 250 ms.
- Route extract-step output through the sink.

**`crates/koji-core/src/backends/updater.rs`**
- No signature change; `update_backend` already takes `InstallOptions`, so the sink rides along.

**`crates/koji-cli/src/commands/backend.rs`**
- No behavioral change. `progress: None` (or `..Default::default()`); existing `println!`s in CLI code remain for CLI-local output.

**`crates/koji-web/src/server.rs`**
- Add `pub jobs: Arc<JobManager>` to `AppState`.
- Initialize in the constructor.
- Register the eight new routes in `build_router`.

**`crates/koji-web/src/api.rs`**
- Add `pub mod backends;` (or a sibling module) with the new handlers. Existing handlers untouched.

**`crates/koji-web/src/pages/config_editor.rs`**
- Replace the body of `BackendsForm` with a fixed two-card render (`llama_cpp`, `ik_llama`) via `BackendCard`.
- Move existing path/args/health/version inputs into the card's Advanced disclosure.
- Add a `Resource` for `GET /api/backends`, refetched on job completion.

### 8.3 Deleted files

- `crates/koji-web/src/components/backends_section.rs` (unused dead code).

### 8.4 Cargo deps

- `axum::response::sse::Sse` — already available via existing `axum`. No new dep.
- `async-stream` — add under `koji-web` if not already transitively present. Fallback: a hand-rolled `Stream` impl.
- `tokio::sync::broadcast` — already in scope via `tokio`.

---

## 9. Test plan

### 9.1 `koji-core` unit tests

1. `ProgressSink` trait object: a `MockSink` collects lines into a `Vec<String>`. Install and update code paths emit the expected milestone lines.
2. `InstallOptions::default()` still works without progress sink.
3. CLI path still compiles; existing backend command tests pass unchanged.

### 9.2 `koji-web` job manager unit tests

4. `JobManager::submit` creates a job; state transitions `Running → Succeeded` on a fake worker.
5. Concurrent `submit` while a job is running returns `JobError::AlreadyRunning(existing_id)`.
6. FIFO eviction: after finishing 9 jobs, the oldest is evicted; at most 8 finished jobs retained.
7. `Job::log_buffer` is bounded at 500 lines; the 501st line evicts the 1st.
8. Broadcast channel delivers live events to active subscribers; `Lagged` errors are skipped gracefully.

### 9.3 `koji-web` API integration tests

9. `GET /api/system/capabilities` returns the expected shape.
10. `GET /api/backends` with empty registry → both known backends with `installed: false`.
11. `GET /api/backends` with a fake registry entry → `installed: true` + populated `info`.
12. `POST /api/backends/install` returns 202 + `job_id`; second call while running → 409 with existing `job_id`.
13. `POST /api/backends/check-updates` populates `update` for installed backends only.
14. `DELETE /api/backends/:name` → 404 on unknown; 200 + `{ "removed": true }` on known.
15. `GET /api/backends/jobs/:id/events` SSE — connect after a job has completed → receives buffered logs + final status, then closes. Connect mid-job → receives buffered logs + live logs.

### 9.4 `koji-web` component tests

16. `BackendCard` renders correctly in each of the 6 states.
17. `InstallModal` builds the correct request body for each GPU type / source selection.
18. `InstallModal` for `ik_llama` disables the source toggle (forced on) and hides the version field.
19. `InstallModal` displays the cmake-missing warning when capabilities report it and source build is selected.

### 9.5 Manual smoke tests

20. Install `llama.cpp` prebuilt CUDA from a fresh registry — observe live logs, completion, card state transition.
21. Install `ik_llama` from source — observe build logs and final success.
22. Trigger update on an installed backend — observe version bump.
23. Trigger uninstall — verify files removed under `backends_dir()` and card returns to "Not installed".
24. Reload the page mid-install — confirm card rehydrates with running job and re-attaches the log stream.
25. Force a failure (disconnect network during prebuilt download) — confirm Failed state + Retry button.

---

## 10. Open questions

None outstanding at spec time.

## 11. Acceptance criteria

- `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all -- --check`, `cargo test --workspace` all pass.
- Both backends can be installed, updated, and uninstalled from the web UI end-to-end on Linux/CUDA.
- Live build/download logs appear in the UI during install/update.
- Page reload mid-install rehydrates the job state and log stream.
- CLI `koji backend install / update / list / remove` still works with no behavior change.
