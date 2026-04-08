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
- Reflect live install state from the `BackendRegistry` (SQLite-backed, see §4) in each card.
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
| `BackendRegistry` — **SQLite** at `<config_dir>/koji.db`, table `backend_installations` (opened via `koji_core::db`) | New backend API endpoints | `BackendInfo` (`name`, `backend_type`, `version`, `path`, `installed_at`, `gpu_type`, `source`) |

The UI displays **two cards**, keyed by the hard-coded set `{ llama_cpp, ik_llama }`:

- Registry has an entry for this name → "Installed" state.
- Registry has no entry → "Not installed" state.

`BackendType::Custom` entries (possible via direct DB writes or future CLI flags) are **listed but not actionable** in the UI — no Install/Update/Uninstall buttons, just a read-only row at the bottom of the section.

Config-side fields continue to live in `config.backends` and are edited via the existing structured-config save flow from within an "Advanced" disclosure on each card. **No TOML schema changes.**

A new in-memory `JobManager` lives in `AppState` to track install/update jobs.

### 4.1 Wire DTOs vs. core types

`koji_core` types use serde defaults that are not web-friendly:

- `BackendType::LlamaCpp` → `"LlamaCpp"` (externally tagged)
- `BackendSource` → `#[serde(tag = "source", content = "content")]` → `{"source":"Prebuilt","content":{"version":"b8407"}}`
- `GpuType::Cuda { version }` → `{"Cuda":{"version":"12.4"}}`
- `JobStatus::Running` → `"Running"`

Changing the derives in `koji-core` would require migrating the stored JSON columns in `backend_installations`. Instead, **`koji-web` defines its own DTOs** in `crates/koji-web/src/api/backends.rs` that serialize with `#[serde(rename_all = "snake_case")]` and flatten tagging, and converts via `From`/`TryFrom`. The JSON shapes in §5 are **wire DTOs, not core types**. Integration tests in §9.3 snapshot the wire shape against fixtures to catch drift.

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
  "detected_cuda_version": "12.4"
}
```
Backed by `koji_core::gpu::detect_build_prerequisites()` + `detect_cuda_version()`. Both use blocking `std::process::Command` to probe `cmake`, `git`, `g++`/`vswhere`, `nvcc`, and `nvidia-smi`, so the handler must wrap the cold-cache miss in `tokio::task::spawn_blocking` to avoid stalling the runtime. ROCm detection does not exist in `koji_core::gpu` today and is intentionally omitted from the response — the UI defaults ROCm to the hardcoded version used by `urls.rs` (`7.2`).

Each call spawns up to ~6 subprocesses (`cmake --version`, `git --version`, compiler probe, `nvcc`, `nvidia-smi`, `vswhere` on Windows). Wrap the handler in a **5-second in-process TTL cache** (`Arc<Mutex<Option<(Instant, Capabilities)>>>`) to avoid hammering on rapid modal reopens.

**`GET /api/backends`** — joined view of known backends. **All field names below are wire DTO names (snake_case); see §4.1.**
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
  ],
  "custom": []
}
```

Notes:
- `update` is left unfilled here — the UI calls `/check-updates` explicitly to avoid hitting GitHub on every page load.
- `active_job` is set **iff** there is currently a job with `status == Running`. Finished/failed jobs do not populate it. The UI uses this solely to reattach an SSE stream after a page reload; completed jobs surface via the card's per-state rendering based on the current registry entry.
- `custom` lists any `BackendType::Custom` registry entries as read-only rows.

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

**Server-side source-build forcing** — the handler upgrades `build_from_source` to `true` in these cases, regardless of what the client sent:

1. `backend_type == ik_llama` (no prebuilts exist at all).
2. `backend_type == llama_cpp` **and** `os == linux` **and** `gpu == Cuda` — because `urls.rs` currently maps Linux+CUDA to the plain `llama-*-bin-ubuntu-x64.tar.gz` CPU build (see `urls.rs:33-37`). Serving a silent CPU binary to a user who asked for CUDA is a trap; building from source is the only way to honor the request today.

The response payload for a forced upgrade includes a notice:
```json
{ "job_id": "j_abc123", "kind": "install", "backend_type": "llama_cpp", "notices": ["forced source build: no prebuilt CUDA binary for Linux"] }
```

When source build is selected (or forced) the handler must verify `git_available && cmake_available && compiler_available` from capabilities; if any are missing it returns `400 Bad Request` with a clear error instead of kicking off a doomed job.

Returns `409 Conflict` if another install/update job is already running:
```json
{ "error": "another backend job is already running", "job_id": "j_existing" }
```

**`POST /api/backends/:name/update`** (empty body) — reuses the existing registry's `gpu_type` and `source` (mirroring CLI `cmd_update`). The handler:

1. Calls `check_latest_version` **once** up front and captures the concrete tag (e.g. `b8410` or `main@abcd1234`).
2. Passes that resolved tag as `latest_version` into `update_backend` so the post-install registry row reflects exactly what the user saw in the card's "Update available → bXXXX" line. Avoids the `updater.rs:124` re-resolve path entirely.

Returns the same shape as install. 409 on concurrent job.

**`DELETE /api/backends/:name`** — synchronous. Removes registry entry **and** binary files.

The current CLI helpers `backends_dir()` and the `canonicalize → starts_with` safety check live privately inside `crates/koji-cli/src/commands/backend.rs`. They must be **lifted into `koji-core`** as:

- `koji_core::backends::backends_dir() -> Result<PathBuf>` — returns `<config_dir>/backends`.
- `koji_core::backends::safe_remove_installation(info: &BackendInfo) -> Result<()>` — canonicalizes `info.path.parent()`, asserts it starts with `backends_dir()`, and `remove_dir_all`s it. Preserves the existing Windows `ErrorKind::PermissionDenied` handling from `cmd_remove`.

Both `koji-web::api::backends::remove_backend` and the CLI's `cmd_remove` then call the shared helper. The handler rejects (without touching the filesystem) any registry entry whose `path` does not canonicalize to a location under `backends_dir()` — for example a user-registered `/usr/local/bin/llama-server` — returning:
```json
{ "error": "path is outside the managed backends directory; remove manually" }
```
with status `409 Conflict`.

Returns:
```json
{ "removed": true }
```
or 404 on unknown name. Returns `409 Conflict` if a job for the same backend is currently `Running`.

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
    pub log_head: RwLock<VecDeque<String>>,  // first 100 lines, never evicted once filled
    pub log_tail: RwLock<VecDeque<String>>,  // last 400 lines, FIFO on overflow
    pub log_dropped: AtomicU64,              // count of lines skipped between head and tail
    pub log_tx: broadcast::Sender<JobEvent>, // capacity 1024
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
- **Retention.** Finished jobs are kept so the UI can still fetch their final log. FIFO eviction keeps at most **8 finished jobs**. Eviction is safe for any SSE clients still attached to an evicted job: the SSE handler holds an `Arc<Job>` taken at connect time, so evicting the map entry does not drop the `Job` out from under the stream — it only stops *new* clients from looking it up.
- **Log buffer: head + tail.** A full source build can emit thousands of lines, and the tail-only view loses the all-important "what command was run and where did it fail" context from the start. We keep two bounded `VecDeque`s per job:
  - `log_head`: the first **100 lines**, never evicted once filled.
  - `log_tail`: the last **400 lines**, `pop_front` on overflow.
  On SSE connect, the handler replays `log_head`, then a single synthesized `[... N lines skipped ...]` marker if any, then `log_tail`.
- **Broadcast channel.** Live tailing. Capacity **1024** events (source builds can burst at hundreds of lines/sec). Slow subscribers that get `broadcast::error::RecvError::Lagged(n)` emit a synthesized `[N lines dropped]` event to the client so the gap is visible in the UI, then continue with the next live event.
- **No persistence** across process restarts. See §6.4 for on-disk cleanup.

`JobManager` is added to `AppState` as `pub jobs: Arc<JobManager>`.

### 6.2 Progress propagation from `koji-core`

**A public field on `InstallOptions` is not viable.** `InstallOptions` derives `Debug` (`installer/mod.rs:17`), adding `Arc<dyn ProgressSink>` would break the derive; it has no `Default` impl so callers can't use `..Default::default()`; and there are three existing call sites (`backend.rs:350`, `backend.rs:471`, `source.rs:491`) that would require `progress: None` churn. Instead, we add **wrapper functions** that leave `InstallOptions` entirely untouched.

```rust
// crates/koji-core/src/backends/mod.rs
pub trait ProgressSink: Send + Sync {
    fn log(&self, line: &str);
}

// No-op sink for callers that want the no-println! behavior in tests.
pub struct NullSink;
impl ProgressSink for NullSink { fn log(&self, _line: &str) {} }
```

```rust
// crates/koji-core/src/backends/installer/mod.rs

// Existing API — unchanged, zero call-site churn.
pub async fn install_backend(options: InstallOptions) -> Result<PathBuf> {
    install_backend_with_progress(options, None).await
}

// New API — koji-web calls this.
pub async fn install_backend_with_progress(
    options: InstallOptions,
    progress: Option<Arc<dyn ProgressSink>>,
) -> Result<PathBuf> {
    // ... existing body, routing println! through `emit(&progress, ...)` ...
}
```

Analogous wrapper `update_backend_with_progress` in `updater.rs`. The CLI keeps calling the original `install_backend` / `update_backend` — no code change in `koji-cli/src/commands/backend.rs`.

Internally, milestone `println!`s in the installer are replaced with an `emit` helper:

```rust
fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    match sink {
        Some(s) => s.log(&line),
        None => println!("{line}"),
    }
}
```

**Scope guard:** we do not rewrite every `println!` in one pass. We route:
1. High-level milestones in `install_backend_with_progress` ("Cloning…", "Building…", "Downloading…", "Extracting…", "Installation complete").
2. Child-process output from `git clone`, `cmake`, `make`/`ninja`.
3. Prebuilt download progress (see below).
4. Extract-step output.

Anything missed still goes to stdout/stderr unchanged (captured by the koji log file).

**Child-process capture** replaces the current inherited stdio on spawned commands in `source.rs` with `Stdio::piped()` for both stdout and stderr. Two line-buffered reader tasks (via `tokio::io::AsyncBufReadExt::lines()`) forward each line via `emit(...)`. Exit-code error handling is preserved.

**Prebuilt download progress — `indicatif` handling.** Today `download.rs` writes a TTY `ProgressBar` to stderr unconditionally. We refactor `download_file` to take an optional progress callback:

```rust
pub async fn download_file(
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()>
```

- If `progress` is `Some`, `indicatif` is **skipped entirely** (no TTY bar) and progress is reported via the sink, throttled to at most one emitted line per ~250 ms (format: `"downloaded 12.3 MiB / 45.6 MiB (27%)"`).
- If `progress` is `None`, the existing `indicatif` TTY bar is preserved — CLI UX is unchanged.

CLI `cmd_install` continues to see its progress bar; the web path gets throttled log lines. No CLI call-site changes required because `download_file` is only called from inside `install_prebuilt`, which is called from `install_backend_with_progress`, which threads the sink down.

### 6.3 SSE handler

```rust
async fn job_events_sse(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Sse<...>, StatusCode> {
    let job = state.jobs.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let mut rx = job.log_tx.subscribe();
    let head: Vec<String> = job.log_head.read().await.iter().cloned().collect();
    let dropped = job.log_dropped.load(Ordering::Relaxed);
    let tail: Vec<String> = job.log_tail.read().await.iter().cloned().collect();
    let terminal_at_connect = {
        let s = job.state.read().await;
        matches!(s.status, JobStatus::Succeeded | JobStatus::Failed).then_some(s.status)
    };

    let stream = async_stream::stream! {
        for line in head {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line }))?);
        }
        if dropped > 0 {
            yield Ok(Event::default().event("log")
                .json_data(json!({ "line": format!("[... {dropped} lines skipped ...]") }))?);
        }
        for line in tail {
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
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    // Surface the gap to the client so the UI shows dropped lines.
                    yield Ok(Event::default().event("log")
                        .json_data(json!({ "line": format!("[{n} lines dropped]") }))?);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
```

Uses `axum::response::sse::Sse`. Replay-on-connect lets clients reconnect mid-install without losing context.

Implementation notes for the `async_stream::stream!` block:
- `Event::default().event(..).json_data(..)` returns `Result<Event, axum::Error>`.
- The `Stream` yielded to `Sse::new` must have item type `Result<Event, E>` where `E: Into<BoxError>`. Use `axum::Error` as the error type and propagate via `?` — don't invent a fresh error type.
- `async_stream` is already a transitive dependency (`koji-core` pulls it in; confirmed in `Cargo.lock`). Declare it explicitly in `crates/koji-web/Cargo.toml` under `[dependencies]` gated to the `ssr` feature.

### 6.4 Process shutdown, orphans, and stale build dirs

`axum::serve(...)` is called without `with_graceful_shutdown` today (`server.rs:161`). If the Koji process dies while a source build is running:

- On Unix, the spawned `git`/`cmake`/`make` are reparented to init and continue building, leaving a partial tree under `<config_dir>/backends/<name>/build/`.
- On Windows, they survive (they are not placed into a Job Object).
- The next install attempt sees a dirty `target_dir` and fails because `allow_overwrite=false`.

Two complementary mitigations:

1. **Graceful shutdown with child-process teardown.** The spawned install-job task stores the child `Child` PIDs in the `Job` struct. On SIGINT/SIGTERM the `JobManager` kills any `Running` job (`Child::kill().await`) and the HTTP server exits via `with_graceful_shutdown`. This is best-effort and does not cover `SIGKILL` or panics.

2. **Stale-install recovery on startup.** On `JobManager::new`, scan `backends_dir()` for per-backend subdirectories that contain a `build/` directory **but** have no corresponding `BackendRegistry` entry with a valid binary path. These are leftover partial installs from a previous crash. The manager logs a warning listing them; the UI exposes a "Clean up" button that `remove_dir_all`s the stale dirs (gated on the same canonical-path safety check as `safe_remove_installation`).

   We do **not** auto-delete on startup — a user with a half-built backend from five minutes ago might want to inspect the build log first. The cleanup UI surface is a single button in the Backends section header that only appears when stale dirs are detected.

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
- **CUDA version** dropdown — shown only when CUDA selected. Options come from a shared constant table (see below). Default from `detected_cuda_version` nearest-match. The same constant table is used by `urls.rs` so UI and URL mapping cannot drift.
- **ROCm version** — **no selector.** `urls.rs:31` hardcodes `rocm-7.2` regardless of the version field in `GpuType::RocM`. The modal shows static text "ROCm 7.2 (hardcoded)" and sends `GpuType::RocM { version: "7.2".into() }`. Revisit when `urls.rs` honors the version parameter.
- **Version** — free text, placeholder `latest`. Hidden for `ik_llama`.
- **Build from source** checkbox.
  - Forced on + disabled for `ik_llama` with help text "ik_llama always builds from source".
  - Forced on + disabled when `os == linux && gpu == cuda` with help text "No prebuilt CUDA binary available for Linux; will build from source."
- **Force overwrite** checkbox.

**Constant table** (shared between `urls.rs` and the modal): `pub const SUPPORTED_CUDA_VERSIONS: &[&str] = &["11.1", "12.4", "13.1"];` in `koji_core::backends::installer::urls`. The UI imports this via a new `GET /api/system/capabilities` field `supported_cuda_versions: Vec<String>` populated from the same constant, so the modal has no hardcoded list.

Warning banner shown at the top when `cmake_available` / `git_available` / `compiler_available` is false **and** source build is selected (forced or otherwise):
> ⚠ cmake not found — source build will fail.

When the warning is active, the Install button is **disabled**; the backend handler will also reject such requests with 400 (defense in depth).

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
> This will delete the registry entry and the backend directory at `<parent_of_path>/`.
> `Cancel`  `Remove`

The modal shows the **parent directory** of `info.path` because that is what `safe_remove_installation` actually removes (matching the existing CLI `cmd_remove` behavior). DELETE `/api/backends/:name`. Synchronous. On success, refresh the card.

Interactive CLI confirmation prompts (`inquire::Confirm` in `cmd_remove` and `cmd_update`) stay in the CLI; the web path skips them entirely — the UI's modal confirmation is the equivalent.

### 7.8 Check for updates

Top-right of the Backends section. POSTs `/api/backends/check-updates`, populates each card's `update` field, updates the badge. Manual only.

---

## 7a. Security & threat model

The existing web API is configured with `CorsLayer::permissive()` (`server.rs:141`) and has no authentication. The default proxy host in `crates/koji-core/src/config/types.rs` is `0.0.0.0`.

The new endpoints materially expand the blast radius over "edit a TOML file":
- `POST /api/backends/install` runs `git clone`, `cmake`, `make`, arbitrary network downloads from GitHub, and writes binaries into the user's home directory.
- `DELETE /api/backends/:name` runs `remove_dir_all` on a path under the user's home directory.

This spec does not introduce authentication (that's a bigger design). It does take the following hardening steps, specific to the new endpoints:

1. **CORS hardening for state-changing backend routes.** The new `/api/backends/*` and `/api/system/capabilities` routes are grouped under a router whose `CorsLayer` only allows `GET` from other origins and requires same-origin for `POST`/`DELETE`. Implementation: a dedicated `CorsLayer::new().allow_origin(AllowOrigin::mirror_request()).allow_methods([GET])` plus an `axum::middleware::from_fn` that checks `Origin`/`Host` equality on non-GET methods and rejects with 403.

2. **Origin-header enforcement.** For `POST`/`DELETE` on the new routes, reject any request where `Origin` is present and does not match `Host`. This blocks CSRF from a malicious page when the user has Koji bound to a LAN address.

3. **Documentation warning.** The Backends section UI shows a one-time dismissible banner on first use:
   > ⚠ Koji is bound to `0.0.0.0`. Anyone on your network can install backends here. Bind to `127.0.0.1` in `config.toml` if you don't want that.
   The banner is shown only when the server detects it is bound to a non-loopback address.

4. **No change to existing endpoints.** The hardening applies only to the new routes so we don't regress any current UI flows.

Full authentication (API tokens, session cookies, etc.) is explicitly out of scope — tracked as future work.

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
- Add `pub trait ProgressSink` (with `NullSink` impl).
- Add `pub fn backends_dir() -> Result<PathBuf>` (lifted from `koji-cli`).
- Add `pub fn safe_remove_installation(info: &BackendInfo) -> Result<()>` (lifted from `cmd_remove`, preserves Windows `PermissionDenied` handling).
- Re-export from `koji_core::backends`.

**`crates/koji-core/src/backends/installer/mod.rs`**
- Add `install_backend_with_progress(options, progress)` wrapper.
- Keep `install_backend(options)` as thin wrapper calling `_with_progress(options, None)` — **zero call-site churn, no `InstallOptions` struct changes, no Debug/Default breakage**.
- Internal `fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>)` routes to sink or `println!`.
- Route top-level milestone `println!`s through `emit`.

**`crates/koji-core/src/backends/installer/source.rs`**
- Accept an optional `&Arc<dyn ProgressSink>` parameter (internal API; not breaking).
- Replace `Stdio::inherit()` on `git`, `cmake`, `make`/`ninja` with `Stdio::piped()` for stdout + stderr.
- Spawn two `tokio::io::BufReader::lines()` reader tasks per process; forward each line through the sink.
- Preserve existing exit-code error handling.

**`crates/koji-core/src/backends/installer/prebuilt.rs`** & **`download.rs`**
- `download_file` gains optional `progress: Option<&Arc<dyn ProgressSink>>`.
- When `Some`, skip `indicatif` entirely and emit throttled progress lines (~1/250 ms).
- When `None`, preserve existing `indicatif` TTY bar (CLI UX unchanged).
- Route extract-step output through the sink.

**`crates/koji-core/src/backends/installer/urls.rs`**
- Export `pub const SUPPORTED_CUDA_VERSIONS: &[&str] = &["11.1", "12.4", "13.1"];` — used by both the URL mapping and the capabilities endpoint.

**`crates/koji-core/src/backends/updater.rs`**
- Add `update_backend_with_progress(registry, name, options, latest_version, progress)` wrapper.
- Keep `update_backend` as thin wrapper calling `_with_progress` with `None`.

**`crates/koji-cli/src/commands/backend.rs`**
- Remove local `backends_dir()` helper, import `koji_core::backends::backends_dir` instead.
- Replace the filesystem-removal block in `cmd_remove` with a call to `koji_core::backends::safe_remove_installation`.
- No other behavioral changes; CLI continues calling `install_backend` / `update_backend` (no progress sink).

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
- `async-stream` — already present transitively via `koji-core` (confirmed in `Cargo.lock`). Declare it **explicitly** in `crates/koji-web/Cargo.toml` under `[dependencies]` gated to the `ssr` feature so `koji-web` does not rely on a transitive edge.
- `tokio::sync::broadcast` — already in scope via `tokio`.

---

## 9. Test plan

### 9.1 `koji-core` unit tests

1. `ProgressSink` trait object: a `MockSink` collects lines into a `Vec<String>`. `install_backend_with_progress` emits the expected milestone lines.
2. **Parity**: `install_backend(opts)` and `install_backend_with_progress(opts, None)` produce identical behavior (same final binary path, same stdout, same errors on a mocked installer).
3. **Debug-derive smoke test**: `fn _assert<T: Debug>() {}; _assert::<InstallOptions>();` — guards against future regressions of the "don't add fields with non-Debug types" invariant.
4. `backends_dir()` and `safe_remove_installation()` tests (lifted from existing CLI test coverage): refuses paths outside `backends_dir`, handles canonicalization failures safely.
5. `SUPPORTED_CUDA_VERSIONS` constant matches the match arms in `get_prebuilt_url` (test asserts every listed version produces a successful URL).
6. CLI path still compiles; existing backend command tests pass unchanged.

### 9.2 `koji-web` job manager unit tests

7. `JobManager::submit` creates a job; state transitions `Running → Succeeded` on a fake worker.
8. Concurrent `submit` while a job is running returns `JobError::AlreadyRunning(existing_id)`.
9. FIFO eviction: after finishing 9 jobs, the oldest is evicted; at most 8 finished jobs retained.
10. **Head buffer invariant**: emitting 150 lines fills `log_head` with the first 100 and never evicts them; `log_head.len() == 100` and `log_head.front()` is the 1st line.
11. **Tail buffer invariant**: after 1000 emitted lines, `log_tail.len() == 400` and `log_tail.front()` is the 601st line; `log_dropped == 500`.
12. **Replay order on connect**: a subscriber that attaches after 1000 lines have been emitted receives, in order: `log_head` (100 lines), one synthesized `[... 500 lines skipped ...]` marker, `log_tail` (400 lines), then any live lines.
13. Broadcast channel delivers live events to active subscribers.

### 9.3 `koji-web` API integration tests

14. `GET /api/system/capabilities` returns the expected shape and includes `supported_cuda_versions`.
15. **JSON snapshot test** against a fixture file: `GET /api/backends` with a seeded registry returns exactly the documented wire shape (catches serde drift from §4.1).
16. `GET /api/backends` with empty registry → both known backends with `installed: false`, `custom: []`.
17. `GET /api/backends` with a fake registry entry → `installed: true` + populated `info`.
18. `GET /api/backends` with a `BackendType::Custom` entry → custom row appears in `custom`, not in the two known cards.
19. `POST /api/backends/install` returns 202 + `job_id`; second call while running → 409 with existing `job_id`.
20. `POST /api/backends/install` for `ik_llama` with `build_from_source: false` → server upgrades to source build and includes a notice.
21. `POST /api/backends/install` for `llama_cpp` with `os=linux, gpu=cuda, build_from_source: false` → server upgrades to source build and includes a notice.
22. `POST /api/backends/install` with source build requested and `cmake_available: false` → 400 with a clear error, no job started.
23. `POST /api/backends/check-updates` populates `update` for installed backends only.
24. `DELETE /api/backends/:name` → 404 on unknown; 200 + `{ "removed": true }` on known.
25. **Path-traversal guard**: `DELETE /api/backends/:name` where the registry-stored `path` points outside `backends_dir()` (e.g. `/usr/local/bin/llama-server`) → 409, no filesystem mutation.
26. **DELETE while running**: `DELETE /api/backends/:name` while a `Running` job targets the same backend → 409.
27. **Origin-header enforcement**: `POST /api/backends/install` with `Origin: http://evil.example` and `Host: 127.0.0.1:8080` → 403.
28. `GET /api/backends/jobs/:id/events` SSE — connect after a job has completed → receives buffered head + (optional marker) + tail + final status, then closes. Connect mid-job → receives buffered logs + live logs.
29. **SSE disconnect**: client disconnects mid-stream; the worker task continues to completion and the job ends in `Succeeded` (verified via `GET /api/backends/jobs/:id`).
30. **SSE lagged marker**: force a slow subscriber until the broadcast channel laps; verify a `[N lines dropped]` event is visible to the client.

### 9.4 `koji-web` component tests

31. `BackendCard` renders correctly in each of the 6 states.
32. `InstallModal` builds the correct request body for each GPU type / source selection.
33. `InstallModal` for `ik_llama` disables the source toggle (forced on) and hides the version field.
34. `InstallModal` displays the cmake-missing warning when capabilities report it and source build is selected.

### 9.5 Manual smoke tests

35. Install `llama.cpp` prebuilt CUDA from a fresh registry — observe live logs, completion, card state transition.
36. Install `ik_llama` from source — observe build logs and final success.
37. Trigger update on an installed backend — observe version bump.
38. Trigger uninstall — verify files removed under `backends_dir()` and card returns to "Not installed".
39. Reload the page mid-install — confirm card rehydrates with running job and re-attaches the log stream.
40. Force a failure (disconnect network during prebuilt download) — confirm Failed state + Retry button.

---

## 10. Open questions

None outstanding at spec time.

## 11. Acceptance criteria

- `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all -- --check`, `cargo test --workspace` all pass.
- Both backends can be installed, updated, and uninstalled from the web UI end-to-end on Linux/CUDA.
- Live build/download logs appear in the UI during install/update.
- Page reload mid-install rehydrates the job state and log stream.
- CLI `koji backend install / update / list / remove` still works with no behavior change.
