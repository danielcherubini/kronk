# Backends Install / Update UI — Implementation Plan

**Goal:** Surface the existing `koji-core` backend install/update/remove flow in the web UI with live build logs, so users never have to drop to the terminal.

**Architecture:** A new in-memory `JobManager` in `koji-web::AppState` runs install/update jobs spawned via new `install_backend_with_progress` / `update_backend_with_progress` wrappers in `koji-core`. The UI shows two cards (`llama_cpp`, `ik_llama`) keyed off the existing `BackendRegistry`. Live logs stream over SSE with a head+tail replay buffer. New routes are origin-checked and live behind a non-permissive CORS layer.

**Tech Stack:** Rust, axum 0.7, Leptos, tokio (broadcast + RwLock), async-stream, SQLite via rusqlite (existing `koji-core::db`).

**Spec reference:** `docs/plans/2026-04-08-backends-install-update-ui-spec.md` (commit `0428aab`). Read it before any task — every section reference (§N) below points there.

**Branch:** `feature/backends-install-update-ui` (already checked out).

---

## Cross-cutting conventions

- **Workspace commands:** `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo fmt --all`, `cargo test --workspace`. These are the gates for every task. **Before committing, also run `cargo fmt --all -- --check`** (the spec §11 acceptance criterion) to verify the formatting is stable, not just that your local formatter rewrote files.
- **TDD:** Write the failing test first, run it, see it fail with the expected error, then implement, then re-run.
- **Commit per task.** Do not stack multiple tasks into one commit.
- **Wire DTOs are snake_case** (§4.1). Never expose `koji-core` enum serde shapes directly through the HTTP API.
- **Do not modify `InstallOptions`.** No new fields, no derive changes, no `Default` impl. (§6.2 spells out why.)
- **Do not change CLI behavior.** `koji backend install/update/list/remove` must work identically before and after.

---

### Task 1: Lift shared backend helpers and constants into `koji-core`

**Context:**
The CLI (`crates/koji-cli/src/commands/backend.rs`) has private helpers `backends_dir()` and an inline `canonicalize → starts_with` safety check inside `cmd_remove`. The web API needs the same logic, so they must move into `koji-core`. We also need to introduce the `ProgressSink` trait that subsequent tasks will use, and export the `SUPPORTED_CUDA_VERSIONS` constant from `urls.rs` so the UI/API and the URL mapper share a single source of truth. CLI code is then refactored to use the lifted helpers — this is the only CLI behavior change in the entire feature, and it must be a pure refactor (no semantic difference).

**Files:**
- Modify: `crates/koji-core/src/backends/mod.rs`
- Modify: `crates/koji-core/src/backends/installer/urls.rs`
- Modify: `crates/koji-cli/src/commands/backend.rs`
- Test: `crates/koji-core/src/backends/mod.rs` (unit tests at the bottom of the file under `#[cfg(test)] mod tests`)
- Test: `crates/koji-core/src/backends/installer/urls.rs` (unit test at the bottom)

**What to implement:**

1. In `crates/koji-core/src/backends/mod.rs`:
   - Add `pub trait ProgressSink: Send + Sync { fn log(&self, line: &str); }`.
   - Add `pub struct NullSink;` with `impl ProgressSink for NullSink { fn log(&self, _line: &str) {} }`.
   - Add `pub fn backends_dir() -> anyhow::Result<std::path::PathBuf>` that returns `Config::base_dir()?.join("backends")` — **exactly** the resolution used by the current CLI helper at `crates/koji-cli/src/commands/backend.rs:165-168`. Do **not** use `dirs::config_dir()`. Create the directory if missing (`std::fs::create_dir_all`).
   - Add `pub fn safe_remove_installation(info: &BackendInfo) -> anyhow::Result<()>`. Behavior:
     - Take `info.path.parent()` (binary lives one level inside the per-backend dir).
     - `canonicalize()` it.
     - Canonicalize `backends_dir()`.
     - Assert the parent's canonical path starts with the canonical backends dir; otherwise return `anyhow::bail!("path is outside the managed backends directory")`.
     - `std::fs::remove_dir_all` the parent.
     - **Preserve the existing Windows `ErrorKind::PermissionDenied` handling** from `cmd_remove` (lines 561–584 at the time of writing): on Windows, retry once after a short delay if removal fails with `PermissionDenied`. Match the exact retry shape used in `cmd_remove` so behavior is identical.
   - Re-export both functions and `ProgressSink` from `koji_core::backends`.

2. In `crates/koji-core/src/backends/installer/urls.rs`:
   - Add `pub const SUPPORTED_CUDA_VERSIONS: &[&str] = &["11.1", "12.4", "13.1"];`.
   - Refactor `get_prebuilt_url`'s CUDA match arms (currently around lines 41-43) to iterate `SUPPORTED_CUDA_VERSIONS` so the constant is the single source of truth. If a refactor would meaningfully change behavior, just add the constant alongside the existing match and add a unit test asserting every version in the constant matches one of the arms — the refactor is a nice-to-have, the constant + test is the requirement.

3. In `crates/koji-cli/src/commands/backend.rs`:
   - Delete the local `fn backends_dir()` helper around line 165.
   - `use koji_core::backends::{backends_dir, safe_remove_installation};`.
   - Replace the filesystem removal block in `cmd_remove` with a single call to `safe_remove_installation(&info)`. Keep the surrounding interactive `inquire::Confirm` prompt.
   - All other CLI behavior must be byte-identical.

**Steps:**
- [ ] Write a failing test `test_backends_dir_returns_config_subdir` in `crates/koji-core/src/backends/mod.rs` `#[cfg(test)]` module asserting `backends_dir()` returns a path ending in `/backends` and that the directory exists after the call.
- [ ] Write a failing test `test_safe_remove_installation_rejects_outside_path` that constructs a `BackendInfo` whose `path` points to `/tmp/llama-server` (outside `backends_dir()`) and asserts `safe_remove_installation` returns an error containing `"outside the managed backends directory"`.
- [ ] Write a failing test `test_supported_cuda_versions_all_map_to_urls` in `urls.rs` that iterates `SUPPORTED_CUDA_VERSIONS` and asserts each one produces a successful prebuilt URL via the existing URL helper.
- [ ] Run `cargo test --package koji-core backends::`
  - All three tests should fail to compile (missing items). If they fail with a different error, stop and investigate.
- [ ] Implement `ProgressSink`, `NullSink`, `backends_dir`, `safe_remove_installation` in `crates/koji-core/src/backends/mod.rs` and the `SUPPORTED_CUDA_VERSIONS` constant in `urls.rs`.
- [ ] Run `cargo test --package koji-core backends::`
  - All three new tests should pass. Existing backends tests must still pass.
- [ ] Refactor `crates/koji-cli/src/commands/backend.rs`: delete local `backends_dir`, import the shared helpers, replace the removal block in `cmd_remove`.
- [ ] Run `cargo test --workspace`
  - All workspace tests still pass. If any CLI test breaks, the refactor changed behavior — stop and reconcile.
- [ ] Run `cargo fmt --all`.
- [ ] Run `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(core): lift backends_dir and safe_remove_installation into koji-core`

**Acceptance criteria:**
- [ ] `koji_core::backends::{backends_dir, safe_remove_installation, ProgressSink, NullSink}` are public.
- [ ] `koji_core::backends::installer::urls::SUPPORTED_CUDA_VERSIONS` is public and has 3 entries.
- [ ] CLI `cmd_remove` calls the shared helper; the local helper and inline safety check are gone.
- [ ] The Windows `PermissionDenied` retry path is preserved in the lifted helper.
- [ ] All workspace tests, clippy, and fmt pass.

---

### Task 2: Add `_with_progress` wrappers and pipe child-process output

**Context:**
The web layer needs to capture install/update output as line-oriented events instead of letting it write to stdout. The spec (§6.2) explicitly forbids modifying `InstallOptions` because it derives `Debug` and has three call sites that don't use a `Default` impl. Instead, we add **wrapper functions** alongside the existing entry points. The original `install_backend` / `update_backend` become thin wrappers calling `_with_progress(..., None)`, so existing call sites compile unchanged. Inside the installer, `println!`s for high-level milestones become `emit(progress, ...)` calls. Spawned subprocesses (`git`, `cmake`, `make`/`ninja`) switch from `Stdio::inherit()` to `Stdio::piped()` and their lines are forwarded through the sink. Prebuilt downloads gain an optional progress parameter — when `Some`, `indicatif` is skipped entirely; when `None`, the existing CLI TTY bar is preserved unchanged.

**Files:**
- Modify: `crates/koji-core/src/backends/installer/mod.rs`
- Modify: `crates/koji-core/src/backends/installer/source.rs`
- Modify: `crates/koji-core/src/backends/installer/prebuilt.rs`
- Modify: `crates/koji-core/src/backends/installer/download.rs`
- Modify: `crates/koji-core/src/backends/updater.rs`
- Test: `crates/koji-core/src/backends/installer/mod.rs` (`#[cfg(test)] mod tests`)

**What to implement:**

1. In `installer/mod.rs`:
   - Add `pub async fn install_backend_with_progress(options: InstallOptions, progress: Option<std::sync::Arc<dyn ProgressSink>>) -> anyhow::Result<std::path::PathBuf>` containing the existing body of `install_backend`.
   - Replace `pub async fn install_backend(options: InstallOptions)` with a one-liner: `install_backend_with_progress(options, None).await`.
   - Add a private `fn emit(sink: Option<&std::sync::Arc<dyn ProgressSink>>, line: impl Into<String>)` that calls `sink.log(&line)` if `Some`, else `println!("{line}")`.
   - **The milestone `println!`s are not in `install_backend` itself** — `install_backend` is a 10-line dispatch (`installer/mod.rs:32-42`). The actual `println!`s live in `source.rs` (11 sites: lines 97, 121, 136, 140, 185, 229, 590, 656, 707, etc.) and `prebuilt.rs` (3 sites: lines 52, 63, 66). **Every affected private helper** (`clone_repository`, `try_clone_latest_tag`, `configure_cmake`, `build_cmake`, `install_binary`, `install_from_source`, `install_prebuilt`, and anything they call transitively) must take `progress: Option<&Arc<dyn ProgressSink>>` as an additional parameter and route its `println!`s through `emit(progress, ...)`.
   - Thread `progress: Option<&Arc<dyn ProgressSink>>` down into `source.rs::install_from_source` and `prebuilt.rs::install_prebuilt`, and from each of those down into every private helper that currently emits a `println!` or spawns a subprocess.

2. In `source.rs`:
   - **Current pattern (confirmed via grep):** every subprocess site uses `tokio::process::Command::new(...)....status().await?` with no explicit `Stdio` configuration — so the child inherits the parent's stdio by default. There are ~8 such sites (around lines 111, 124, 164, 194, 240, 441, 471, 624, 643). There is **no `Stdio::inherit()` to "replace"** — the change is a control-flow shift from `.status()` to `.spawn()` with piped stdio when capturing, otherwise leave the `.status()` path as-is.
   - Accept `progress: Option<&Arc<dyn ProgressSink>>` (internal API; not breaking) on every helper that spawns a subprocess.
   - Extract a small helper `async fn run_command(mut cmd: tokio::process::Command, progress: Option<&Arc<dyn ProgressSink>>) -> anyhow::Result<std::process::ExitStatus>` that:
     - If `progress.is_none()`: call `cmd.status().await?` and return it (preserves exact current CLI behavior).
     - If `progress.is_some()`: call `cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?`, take `child.stdout` and `child.stderr`, spawn two `tokio::task` readers that iterate `tokio::io::BufReader::new(stream).lines()` forwarding each line via `emit(progress, line)`, then `child.wait().await`, then join both reader tasks, then return the status.
   - Rewrite each of the 8 spawn sites to use this helper. Preserve the existing non-zero exit-code error messages verbatim.

3. In `download.rs`:
   - Change `download_file` signature to `pub async fn download_file(url: &str, dest: &Path, progress: Option<&std::sync::Arc<dyn ProgressSink>>) -> anyhow::Result<()>`.
   - When `progress.is_some()`: do **not** create the `indicatif::ProgressBar`. Track `downloaded` and `total` size in local variables, and emit a throttled line via `progress.log(...)` at most once per ~250 ms (`tokio::time::Instant::elapsed`). Format: `downloaded {hsz_done} / {hsz_total} ({pct}%)` using `humansize`-style formatting (or hand-rolled if `humansize` isn't already a dep — check `Cargo.toml`; if not present, use a simple `MiB` fixed-point format `format!("{:.1} MiB", bytes as f64 / 1_048_576.0)`).
   - When `progress.is_none()`: keep the current `indicatif` flow unchanged.
   - Update the only caller (`prebuilt.rs::install_prebuilt`) to pass `progress` through.

4. In `prebuilt.rs`:
   - Accept `progress: Option<&Arc<dyn ProgressSink>>`.
   - Pass to `download_file`.
   - Route extract-step output through `emit` (the `tar`/`zip` extraction milestones).

5. In `updater.rs`:
   - **Important:** `update_backend` **already** takes `options: InstallOptions` and `latest_version: String` today (see `updater.rs:104-109`). There is no `UpdateOptions` type. The new wrapper only adds a trailing `progress` parameter; all existing parameters are unchanged.
   - Add `pub async fn update_backend_with_progress(registry: &mut BackendRegistry, backend_name: &str, options: InstallOptions, latest_version: String, progress: Option<Arc<dyn ProgressSink>>) -> anyhow::Result<()>` containing the existing body of `update_backend`, with:
     - The internal call to `install_backend(options).await?` replaced by `install_backend_with_progress(options, progress.clone()).await?`.
     - The existing `"latest"` → concrete-tag resolution (line 124) preserved as-is. The web handler in Task 5 pre-resolves `latest_version` to a concrete tag before calling this wrapper, so the re-resolve branch becomes a no-op for web callers; CLI callers that pass `"latest"` still get the same behavior.
   - Rewrite `pub async fn update_backend(...)` as a one-liner: `update_backend_with_progress(registry, backend_name, options, latest_version, None).await`.

**Steps:**
- [ ] Write a failing test `test_install_backend_parity_with_null_progress` in `installer/mod.rs` tests that uses a `MockSink` (a struct with `Mutex<Vec<String>>`, implementing `ProgressSink`) and asserts `install_backend_with_progress(opts, None)` and `install_backend_with_progress(opts, Some(Arc::new(NullSink)))` both produce identical results on a mocked installer path. If the installer is hard to mock end-to-end, narrow the test to a helper function that exercises the `emit` routing directly.
- [ ] Write a failing test `test_progress_sink_captures_milestone_lines` that calls the wrapper with a `MockSink` and asserts the captured lines include at least one expected milestone (e.g., contains `"Cloning"` or `"Downloading"` for the relevant code path). This test can use a feature-flag-gated helper that emits a fixed milestone if the full installer is too heavy to invoke.
- [ ] Write a failing test `_assert_install_options_debug` in the same test module: `fn _assert<T: std::fmt::Debug>() {}; _assert::<InstallOptions>();`. This is a compile-time guard against accidentally adding non-Debug fields to `InstallOptions`.
- [ ] Run `cargo test --package koji-core installer::mod::tests`
  - Tests should fail to compile (missing wrapper / MockSink). Fix by implementing.
- [ ] Implement `install_backend_with_progress`, the `emit` helper, and rewire `install_backend` as a thin wrapper. Add the `MockSink` helper inside the test module.
- [ ] Implement `update_backend_with_progress` and rewire `update_backend`.
- [ ] Update `source.rs` to accept and thread `progress`, switching to piped stdio when `Some`.
- [ ] Update `download.rs` and `prebuilt.rs` to accept and thread `progress`, skipping `indicatif` when `Some`.
- [ ] Run `cargo test --package koji-core`
  - All tests pass, including the parity test and the Debug-derive smoke test.
- [ ] Run `cargo build --workspace`
  - Should succeed with zero changes to `koji-cli` files. If `koji-cli` fails to compile, you broke the wrapper invariant — fix the wrapper, do not edit CLI call sites.
- [ ] **Manual sanity check (optional but recommended):** `cargo run -p koji-cli -- backend install llama_cpp` and confirm the TTY progress bar still appears. Skip if no GPU/network. The Debug test + parity test cover the regression surface.
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(core): add install/update progress wrappers with piped child stdio`

**Acceptance criteria:**
- [ ] `install_backend_with_progress` and `update_backend_with_progress` are public; `install_backend` / `update_backend` remain in place as thin wrappers.
- [ ] Zero call-site changes in `crates/koji-cli/`.
- [ ] `InstallOptions` is unmodified — same fields, same derives.
- [ ] Source builds with `progress: Some(...)` capture child stdout/stderr line-by-line into the sink.
- [ ] Source builds with `progress: None` still inherit stdio (CLI behavior unchanged).
- [ ] `download_file` with `progress: Some(...)` skips indicatif and emits throttled lines; with `None` keeps the TTY bar.
- [ ] All workspace tests, clippy, and fmt pass.

---

### Task 3: `JobManager` with head/tail buffer and single-in-flight policy

**Context:**
The web layer needs to track install/update jobs in memory: their status, captured logs, and a broadcast channel for live SSE subscribers. Per spec §6.1, only one job runs at a time; finished jobs are retained (max 8, FIFO) so the UI can fetch their final log; logs are stored as `log_head` (first 100 lines, never evicted) + `log_tail` (last 400 lines, FIFO) with a `log_dropped` counter for the gap. SSE subscribers replay head → marker → tail before attaching to the live broadcast channel (which lives in capacity 1024). This task implements the `JobManager` and `Job` types with thorough unit tests; the HTTP handlers and SSE wiring come in later tasks.

**Files:**
- Create: `crates/koji-web/src/jobs.rs`
- Modify: `crates/koji-web/src/lib.rs` (to declare the new module; check if it's `lib.rs` or `main.rs` — `koji-web` is a binary+lib crate, the module declaration goes wherever existing `mod api;` etc. live)
- Test: `crates/koji-web/src/jobs.rs` (`#[cfg(test)] mod tests` at bottom)

**What to implement:**

In `crates/koji-web/src/jobs.rs`:

```rust
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

pub type JobId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus { Running, Succeeded, Failed }
// Note: there is no Queued state. submit() transitions directly to Running because
// there is only ever one in-flight job. If we later add a queue, reintroduce Queued.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind { Install, Update }

#[derive(Debug, Clone)]
pub enum JobEvent { Log(String), Status(JobStatus) }

pub struct JobState {
    pub status: JobStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub backend_type: koji_core::backends::BackendType,
    pub state: RwLock<JobState>,
    pub log_head: RwLock<VecDeque<String>>,   // cap 100
    pub log_tail: RwLock<VecDeque<String>>,   // cap 400
    pub log_dropped: AtomicU64,
    pub log_tx: broadcast::Sender<JobEvent>,  // capacity 1024
}

pub const LOG_HEAD_CAP: usize = 100;
pub const LOG_TAIL_CAP: usize = 400;
pub const LOG_BROADCAST_CAP: usize = 1024;
pub const RETAINED_FINISHED_JOBS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum JobError {
    #[error("another backend job is already running")]
    AlreadyRunning(JobId),
    #[error("job not found")]
    NotFound,
}

pub struct JobManager {
    jobs: Arc<RwLock<HashMap<JobId, Arc<Job>>>>,
    finished_order: Arc<Mutex<VecDeque<JobId>>>,
    active: Arc<Mutex<Option<JobId>>>,
}

impl JobManager {
    pub fn new() -> Self { ... }

    /// Reserve an active slot, return a fresh Job. Returns AlreadyRunning if one is active.
    pub async fn submit(
        &self,
        kind: JobKind,
        backend_type: koji_core::backends::BackendType,
    ) -> Result<Arc<Job>, JobError> { ... }

    pub async fn get(&self, id: &JobId) -> Option<Arc<Job>>;

    pub async fn active(&self) -> Option<Arc<Job>>;

    /// Append a log line to the job: writes to head if not full, else tail (with eviction),
    /// increments log_dropped if a line falls between head and tail, and broadcasts on log_tx.
    pub async fn append_log(&self, job: &Job, line: String);

    /// Mark the job terminal, broadcast the status event, release the active slot,
    /// and FIFO-evict finished jobs beyond RETAINED_FINISHED_JOBS.
    pub async fn finish(&self, job: &Job, status: JobStatus, error: Option<String>);
}
```

Behavior details:
- `submit`: if `*active.lock() == Some(_)`, return `AlreadyRunning(existing_id)`. Otherwise generate `format!("j_{}", uuid::Uuid::new_v4().simple())` (or a `nanoid` — match what's already in the workspace; check `Cargo.toml`. If neither, hand-roll: 12 hex chars from `rand::random::<u64>()`), construct the `Job`, insert into `jobs`, set `active`, return.
- `append_log` policy:
  - Take `log_head.write()`. If `len < LOG_HEAD_CAP`, push. Done. (Drop the write before broadcast to keep the lock window minimal.)
  - Else: take `log_tail.write()`. If `len < LOG_TAIL_CAP`, push. Else `pop_front()` then push, and increment `log_dropped`.
  - After releasing both locks, `let _ = job.log_tx.send(JobEvent::Log(line));` — ignore send errors (no subscribers is fine).
- `finish`:
  - Update `state.status`, `state.finished_at`, `state.error`.
  - Broadcast `JobEvent::Status(status)`.
  - Set `*active.lock() = None`.
  - Push `id` onto `finished_order`. If `finished_order.len() > RETAINED_FINISHED_JOBS`, `pop_front` and remove that id from `jobs`. **Important:** any SSE subscriber that already holds an `Arc<Job>` is unaffected by the map removal.

`thiserror` and `uuid` should already be in the `koji-web` `Cargo.toml`; check before adding. If `uuid` is missing and you don't want to add it, use a simple counter + random suffix.

**Steps:**
- [ ] Read `crates/koji-web/Cargo.toml` to check whether `uuid`, `thiserror`, `tokio` (with `sync` feature), and `serde` are already deps. Add `thiserror` if missing. Use whatever ID scheme is already available.
- [ ] Read `crates/koji-web/src/lib.rs` (or `main.rs`) to identify how existing modules (`api`, `server`, etc.) are declared.
- [ ] Write failing test `test_submit_then_finish_transitions_state` in `jobs.rs` tests: submit a job, assert it's active and Running; call `finish(_, Succeeded, None)`; assert state is Succeeded, `active()` returns `None`.
- [ ] Write failing test `test_concurrent_submit_returns_already_running`: submit one job, then submit another; assert the second returns `JobError::AlreadyRunning(first_id)`.
- [ ] Write failing test `test_fifo_eviction_after_retained_limit`: submit-and-finish 9 jobs sequentially (resetting `active` between each via `finish`); assert `manager.get(&first_id).await.is_none()` and `manager.get(&second_id).await.is_some()`.
- [ ] Write failing test `test_log_head_invariant_first_100_lines_pinned`: append 150 lines; assert `log_head.len() == 100`, `log_head.front() == "line 0"` (or whatever you used), `log_dropped == 0`, `log_tail.len() == 50`.
- [ ] Write failing test `test_log_tail_eviction_after_overflow`: append 1000 lines; assert `log_head.len() == 100`, `log_tail.len() == 400`, `log_dropped == 500`, `log_tail.front() == "line 600"`.
- [ ] Write failing test `test_broadcast_channel_delivers_live_lines`: submit job, subscribe `log_tx.subscribe()`, append 3 lines, assert the receiver gets 3 `JobEvent::Log` events in order.
- [ ] Run `cargo test --package koji-web jobs::tests`
  - All tests should fail to compile.
- [ ] Implement `jobs.rs` end-to-end. Add `pub mod jobs;` declaration in `lib.rs`/`main.rs` next to existing module decls.
- [ ] Run `cargo test --package koji-web jobs::tests`
  - All six tests pass.
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(web): add JobManager with head+tail log buffer and broadcast channel`

**Acceptance criteria:**
- [ ] `crates/koji-web/src/jobs.rs` exists and is declared as a module.
- [ ] `JobManager` enforces single-in-flight via `submit` returning `AlreadyRunning`.
- [ ] FIFO eviction caps retained finished jobs at 8.
- [ ] `log_head` is bounded at 100 and never evicts; `log_tail` is bounded at 400 and FIFO-evicts; `log_dropped` counts the gap.
- [ ] Broadcast channel has capacity 1024 and delivers `JobEvent`s to subscribers.
- [ ] All six unit tests pass; clippy/fmt clean.

---

### Task 4: Read API — capabilities, list backends, wire DTOs, security middleware

**Context:**
Add the read-only HTTP surface: `GET /api/system/capabilities`, `GET /api/backends`, and the dedicated CORS layer + origin-header middleware that protect the new routes (§7a). The capabilities handler must call blocking process probes via `tokio::task::spawn_blocking` and cache results for 5 seconds to avoid hammering. Wire DTOs must be defined in `koji-web` (snake_case, flat tagging) — never expose `koji-core` enum serde shapes directly. JSON snapshot tests catch drift.

**Files:**
- Create: `crates/koji-web/src/api/backends.rs`
- Create: `crates/koji-web/src/api/backends_dto.rs` (or inline in `backends.rs` — pick one and stick with it)
- Create: `crates/koji-web/tests/backends_api.rs` (integration test file using `axum::Router` + `tower::ServiceExt::oneshot`)
- Create: `crates/koji-web/tests/fixtures/backends_list.json` (snapshot fixture)
- Modify: `crates/koji-web/src/api.rs` (add `pub mod backends;`)
- Modify: `crates/koji-web/src/server.rs` (register routes, wire JobManager into AppState, attach origin middleware + CORS layer)
- Modify: `crates/koji-web/Cargo.toml` (add `tower` dev-dep if needed for `oneshot`; `async-stream` will be needed in Task 6 but add it now under ssr feature so it's covered)

**What to implement:**

1. **Wire DTOs** (`api/backends.rs` or sibling):
   ```rust
   #[derive(Debug, serde::Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct BackendListResponse {
       pub active_job: Option<ActiveJobDto>,
       pub backends: Vec<BackendCardDto>,
       pub custom: Vec<BackendCardDto>,
   }

   #[derive(Debug, serde::Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct BackendCardDto {
       pub r#type: String,             // "llama_cpp" | "ik_llama" | "custom"
       pub display_name: String,
       pub installed: bool,
       pub info: Option<BackendInfoDto>,
       pub update: UpdateStatusDto,
       pub release_notes_url: String,
   }

   #[derive(Debug, serde::Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct BackendInfoDto {
       pub name: String,
       pub version: String,
       pub path: String,
       pub installed_at: i64,
       pub gpu_type: GpuTypeDto,
       pub source: BackendSourceDto,
   }

   #[derive(Debug, serde::Serialize)]
   #[serde(tag = "kind", rename_all = "snake_case")]
   pub enum GpuTypeDto {
       Cpu,
       Cuda { version: String },
       Rocm { version: String },
       Vulkan,
       Metal,
   }

   #[derive(Debug, serde::Serialize)]
   #[serde(tag = "kind", rename_all = "snake_case")]
   pub enum BackendSourceDto {
       Prebuilt { version: String },
       Source { commit: Option<String> },
   }

   #[derive(Debug, serde::Serialize)]
   pub struct UpdateStatusDto {
       pub checked: bool,
       pub latest_version: Option<String>,
       pub update_available: Option<bool>,
   }

   #[derive(Debug, serde::Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct ActiveJobDto {
       pub id: String,
       pub kind: crate::jobs::JobKind,
       pub backend_type: String,
   }

   #[derive(Debug, serde::Serialize)]
   pub struct CapabilitiesDto {
       pub os: String,
       pub arch: String,
       pub git_available: bool,
       pub cmake_available: bool,
       pub compiler_available: bool,
       pub detected_cuda_version: Option<String>,
       pub supported_cuda_versions: Vec<String>,
   }
   ```

   Implement `From<koji_core::backends::BackendInfo> for BackendInfoDto`, `From<&koji_core::gpu::GpuType> for GpuTypeDto`, and `From<&koji_core::backends::BackendSource> for BackendSourceDto`. **`GpuType` lives in `koji_core::gpu`, not `koji_core::backends`** (confirmed `gpu.rs:5`). Match against the actual variants of those core enums (read `crates/koji-core/src/backends/mod.rs`, `backends/registry/` for `BackendSource`, and `gpu.rs` to confirm field names).

   **`BackendSourceDto::SourceCode` must match the real variant** — `koji_core::backends::BackendSource::SourceCode { version, git_url, commit }` has three fields, not one. The DTO must preserve all three:
   ```rust
   #[derive(Debug, serde::Serialize)]
   #[serde(tag = "kind", rename_all = "snake_case")]
   pub enum BackendSourceDto {
       Prebuilt { version: String },
       SourceCode { version: String, git_url: String, commit: Option<String> },
   }
   ```
   The `kind` tag serializes as `"prebuilt"` and `"source_code"` (snake_case of the variant name). Update any fixtures accordingly.

2. **Capabilities handler** with 5-second TTL cache:
   ```rust
   pub struct CapabilitiesCache {
       inner: tokio::sync::Mutex<Option<(std::time::Instant, CapabilitiesDto)>>,
   }
   ```
   Add `pub capabilities: Arc<CapabilitiesCache>` to `AppState`. The handler:
   ```rust
   pub async fn system_capabilities(
       State(state): State<Arc<AppState>>,
   ) -> Json<CapabilitiesDto> {
       let mut guard = state.capabilities.inner.lock().await;
       if let Some((t, c)) = &*guard {
           if t.elapsed() < std::time::Duration::from_secs(5) {
               return Json(c.clone());
           }
       }
       // Cold path: shell out to probes inside spawn_blocking.
       let fresh = tokio::task::spawn_blocking(|| {
           let prereqs = koji_core::gpu::detect_build_prerequisites();
           let cuda = koji_core::gpu::detect_cuda_version();
           CapabilitiesDto {
               os: std::env::consts::OS.to_string(),
               arch: std::env::consts::ARCH.to_string(),
               git_available: prereqs.git_available,
               cmake_available: prereqs.cmake_available,
               compiler_available: prereqs.compiler_available,
               detected_cuda_version: cuda,
               supported_cuda_versions: koji_core::backends::installer::urls::SUPPORTED_CUDA_VERSIONS
                   .iter().map(|s| s.to_string()).collect(),
           }
       }).await;
       let fresh = match fresh {
           Ok(c) => c,
           Err(_) => {
               // Degraded response on probe panic: mark everything unavailable so the UI shows
               // warning banners and disables source-build instead of 500'ing the request.
               CapabilitiesDto {
                   os: std::env::consts::OS.to_string(),
                   arch: std::env::consts::ARCH.to_string(),
                   git_available: false,
                   cmake_available: false,
                   compiler_available: false,
                   detected_cuda_version: None,
                   supported_cuda_versions: koji_core::backends::installer::urls::SUPPORTED_CUDA_VERSIONS
                       .iter().map(|s| s.to_string()).collect(),
               }
           }
       };
       *guard = Some((std::time::Instant::now(), fresh.clone()));
       Json(fresh)
   }
   ```
   Verify the actual field names of `BuildPrerequisites` in `crates/koji-core/src/gpu.rs` before writing this — they may be different. Adjust accordingly.

3. **List backends handler:**
   ```rust
   pub async fn list_backends(
       State(state): State<Arc<AppState>>,
   ) -> Result<Json<BackendListResponse>, (StatusCode, Json<serde_json::Value>)> {
       // Open registry, list all backends, partition into known + custom,
       // attach active_job from JobManager (only if status == Running),
       // populate display names + release notes URLs from a static lookup.
   }
   ```
   - Known set: `[("llama_cpp", "llama.cpp", "https://github.com/ggml-org/llama.cpp/releases"), ("ik_llama", "ik_llama.cpp", "https://github.com/ikawrakow/ik_llama.cpp/commits/main")]`.
   - Always emit both known cards even if neither is installed.
   - `update` is always `{ checked: false, latest_version: null, update_available: null }` for this endpoint — populated only by `check-updates` (Task 5).
   - `active_job` is `Some(_)` only if `state.jobs.active().await.is_some() && active_job.state.status == Running`.

4. **Origin-enforcement middleware** (`api/backends.rs` or new `security.rs`):
   ```rust
   pub async fn enforce_same_origin(
       req: axum::extract::Request,
       next: axum::middleware::Next,
   ) -> Result<axum::response::Response, StatusCode> {
       let method = req.method().clone();
       if matches!(method, axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS) {
           return Ok(next.run(req).await);
       }
       let headers = req.headers();
       let host = headers.get("host").and_then(|v| v.to_str().ok());
       let origin = headers.get("origin").and_then(|v| v.to_str().ok());
       if let (Some(host), Some(origin)) = (host, origin) {
           // Parse origin's host:port and compare to host header
           if let Ok(url) = url::Url::parse(origin) {
               let origin_host = url.host_str().unwrap_or("");
               let origin_authority = match url.port() {
                   Some(p) => format!("{}:{}", origin_host, p),
                   None => origin_host.to_string(),
               };
               if origin_authority != host {
                   return Err(StatusCode::FORBIDDEN);
               }
           } else {
               return Err(StatusCode::FORBIDDEN);
           }
       }
       // If Origin header is absent (same-origin fetch from <form>, server-to-server, or curl),
       // allow through. CSRF requires a browser, which always sends Origin on cross-origin POST/DELETE.
       Ok(next.run(req).await)
   }
   ```
   Add `url` to `koji-web` deps if it isn't already.

5. **Router wiring** in `server.rs`:
   - Add `pub jobs: Arc<JobManager>` and `pub capabilities: Arc<CapabilitiesCache>` fields to `AppState`. Initialize in the constructor.
   - Build a sub-router for the new routes:
     ```rust
     let backends_routes = Router::new()
         .route("/api/system/capabilities", get(api::backends::system_capabilities))
         .route("/api/backends", get(api::backends::list_backends))
         .layer(axum::middleware::from_fn(api::backends::enforce_same_origin))
         .layer(
             tower_http::cors::CorsLayer::new()
                 .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
                 .allow_methods([axum::http::Method::GET])
                 .allow_headers(tower_http::cors::Any),
         );
     ```
     Merge `backends_routes` into the main router. **Crucially,** the new `CorsLayer` must NOT be `permissive()` and must NOT include POST/DELETE in `allow_methods` — those methods will be added in Task 5 when their routes go in. For Task 4 only the GETs exist.
   - Existing routes keep their existing `CorsLayer::permissive()` (no regression).

**Steps:**
- [ ] Read `crates/koji-core/src/backends/mod.rs` (specifically `BackendInfo`, `BackendType`, `BackendSource` enums) and `crates/koji-core/src/gpu.rs` (`GpuType`, `BuildPrerequisites`, `detect_build_prerequisites`, `detect_cuda_version`) to confirm exact field/variant names. Adjust the DTO `From` impls accordingly.
- [ ] Read `crates/koji-web/src/server.rs` and `crates/koji-web/src/api.rs` to understand existing module layout, `AppState` struct, and how routes are currently merged.
- [ ] Read `crates/koji-web/Cargo.toml` to check for `tower-http`, `tower`, `url`. Add what's missing under `[dependencies]` (and `tower` under `[dev-dependencies]` if needed for tests).
- [ ] Add `async-stream` to `koji-web/Cargo.toml` under `[dependencies]` gated to `ssr` feature now (will be used in Task 6).
- [ ] Create `crates/koji-web/tests/fixtures/backends_list.json` with the exact expected JSON for an empty registry (use the §5.1 example as a template, but with `installed: false` for both backends and `info: null`).
- [ ] Write failing integration test `test_get_backends_empty_registry_matches_snapshot` in `crates/koji-web/tests/backends_api.rs`. Build a router with an empty `AppState`, call `GET /api/backends` via `tower::ServiceExt::oneshot`, parse the response body as JSON, and assert it matches the fixture (use `serde_json::Value` equality or `pretty_assertions`).
- [ ] Write failing test `test_get_backends_includes_installed_entry`: seed a `BackendRegistry` with one fake `BackendInfo` for `llama_cpp`, assert `installed: true` and `info` is populated with the expected snake_case fields.
- [ ] Write failing test `test_get_backends_custom_entry_appears_in_custom_array`: seed a `BackendType::Custom` entry, assert it appears in `custom`, not in `backends`.
- [ ] Write failing test `test_get_capabilities_returns_supported_cuda_versions`: assert the response includes `supported_cuda_versions: ["11.1", "12.4", "13.1"]` and the `os` matches `std::env::consts::OS`.
- [ ] Write failing test `test_origin_enforcement_blocks_cross_origin_post`: this test will only meaningfully fire in Task 5 when POST routes exist. **Skip for this task; add a TODO comment** and re-enable in Task 5.
- [ ] Run `cargo test --package koji-web --test backends_api`
  - All four enabled tests should fail to compile.
- [ ] Implement DTOs, `From` conversions, `system_capabilities`, `list_backends`, `enforce_same_origin` middleware. Wire `JobManager` and `CapabilitiesCache` into `AppState`. Register the two GET routes with the new sub-router pattern.
- [ ] Run `cargo test --package koji-web --test backends_api`
  - All four tests pass.
- [ ] Run `cargo test --workspace`
  - Nothing else regresses.
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(web): add backends read API with wire DTOs and capability detection`

**Acceptance criteria:**
- [ ] `GET /api/system/capabilities` returns the documented shape with `supported_cuda_versions` populated from the shared constant.
- [ ] `GET /api/backends` returns the documented snake_case shape; both known backends always appear; `BackendType::Custom` entries land in `custom`.
- [ ] The capabilities handler uses `spawn_blocking` and a 5-second TTL cache.
- [ ] Wire DTOs are defined in `koji-web`; core types are not directly serialized over the wire.
- [ ] JSON snapshot test against a fixture file passes.
- [ ] `enforce_same_origin` middleware exists and is wired to the new sub-router (no POST routes exist yet, so the cross-origin test is deferred to Task 5).
- [ ] All workspace tests, clippy, and fmt pass.

---

### Task 5: Mutation API — install, update, uninstall, check-updates

**Context:**
Add `POST /api/backends/install`, `POST /api/backends/:name/update`, `DELETE /api/backends/:name`, and `POST /api/backends/check-updates`. The install handler must (a) force `build_from_source=true` server-side for `ik_llama` (any OS) and `llama_cpp` on Linux+CUDA, (b) validate `cmake/git/compiler` availability when source build is selected (returning 400 with a clear error if missing), and (c) return 409 if a job is already running. Update pre-resolves the latest tag once via `check_latest_version` and passes it explicitly to `update_backend_with_progress`. Delete uses `safe_remove_installation` and rejects paths outside `backends_dir()` with 409. The job tasks are spawned with a `JobAdapter: ProgressSink` that forwards lines into the JobManager. Origin-header enforcement now applies to real POST/DELETE routes — re-enable the deferred test from Task 4.

**Files:**
- Modify: `crates/koji-web/src/api/backends.rs`
- Modify: `crates/koji-web/src/server.rs` (register the four new routes, extend the sub-router's `allow_methods` to include POST/DELETE)
- Modify: `crates/koji-web/tests/backends_api.rs` (add the deferred test + new ones)

**What to implement:**

1. **`JobAdapter`** — a `ProgressSink` impl that forwards into the `JobManager`:
   ```rust
   pub struct JobAdapter {
       jobs: Arc<JobManager>,
       job: Arc<Job>,
   }
   impl ProgressSink for JobAdapter {
       fn log(&self, line: &str) {
           let jobs = self.jobs.clone();
           let job = self.job.clone();
           let line = line.to_string();
           // ProgressSink::log is sync; we need to call async append_log.
           // Use tokio::runtime::Handle::current().spawn — installer runs inside the runtime.
           tokio::runtime::Handle::current().spawn(async move {
               jobs.append_log(&job, line).await;
           });
       }
   }
   ```
   The fire-and-forget spawn is acceptable here because the installer is on the tokio runtime. Document the assumption in a comment.

2. **Install request DTO + handler:**
   ```rust
   #[derive(Debug, serde::Deserialize)]
   #[serde(rename_all = "snake_case")]
   pub struct InstallRequest {
       pub backend_type: String,           // "llama_cpp" | "ik_llama"
       pub version: Option<String>,        // null → "latest"
       pub gpu_type: GpuTypeDto,
       pub build_from_source: bool,
       pub force: bool,
   }

   #[derive(Debug, serde::Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct InstallResponse {
       pub job_id: String,
       pub kind: JobKind,
       pub backend_type: String,
       #[serde(skip_serializing_if = "Vec::is_empty")]
       pub notices: Vec<String>,
   }
   ```

   Handler logic:
   - Parse `backend_type` into the core enum. Reject `Custom`.
   - Compute `effective_build_from_source`:
     - `true` if `backend_type == ik_llama`.
     - `true` if `backend_type == llama_cpp && os == "linux" && matches!(gpu_type, GpuTypeDto::Cuda { .. })`.
     - Else use the requested value.
   - Build a `notices: Vec<String>` describing any forced upgrades.
   - If `effective_build_from_source`, fetch capabilities (via the cache) and check `git_available && cmake_available && compiler_available`. If any false, return `400 Bad Request` with `{ "error": "missing build prerequisite: cmake" }` (or whichever is missing).
   - Convert `GpuTypeDto` → `koji_core::gpu::GpuType`.
   - Call `state.jobs.submit(JobKind::Install, backend_type.clone()).await`. On `AlreadyRunning(id)` return `409 Conflict` with `{ "error": "another backend job is already running", "job_id": id }`.
   - Build `InstallOptions` using the **real** field set (`installer/mod.rs:17-26`): `{ backend_type, source, target_dir, gpu_type, allow_overwrite }`. There is no `name`, `version`, or `build_from_source` field — the web handler must translate its inputs into a `BackendSource`:
     ```rust
     let version = req.version.unwrap_or_else(|| "latest".into());
     let source = if effective_build_from_source {
         BackendSource::SourceCode {
             version,
             git_url: default_git_url_for(backend_type),  // reuse CLI helper or inline per cmd_install
             commit: None,
         }
     } else {
         BackendSource::Prebuilt { version }
     };
     let target_dir = koji_core::backends::backends_dir()?.join(default_dir_name_for(backend_type));
     let options = InstallOptions {
         backend_type: backend_type.clone(),
         source,
         target_dir,
         gpu_type: Some(gpu_type),
         allow_overwrite: req.force,
     };
     ```
     Mirror the `source` and `target_dir` construction from `cmd_install` in `crates/koji-cli/src/commands/backend.rs` rather than reinventing it. Do **not** add fields to `InstallOptions`.
   - `tokio::spawn(async move { let adapter = Arc::new(JobAdapter { jobs, job }); match install_backend_with_progress(opts, Some(adapter)).await { Ok(_) => jobs.finish(&job, JobStatus::Succeeded, None).await, Err(e) => jobs.finish(&job, JobStatus::Failed, Some(e.to_string())).await, } });`
   - Return `202 Accepted` with `InstallResponse`.

3. **Update handler** (`POST /api/backends/:name/update`, empty body):
   - Look up the backend in the registry. 404 if missing.
   - Call `koji_core::backends::check_latest_version(backend_type, &source).await?` to resolve the concrete tag (e.g. `b8410` or `main@abcd1234`). On error, return 502 with the error message.
   - `state.jobs.submit(JobKind::Update, backend_type).await` — 409 on `AlreadyRunning`.
   - Spawn the update task: `update_backend_with_progress(&mut registry, &name, options_built_from_existing_info, latest_tag, Some(adapter)).await`.
   - Return 202 with the same shape as install (no notices).

4. **Delete handler** (`DELETE /api/backends/:name`):
   - Look up the backend. 404 if missing.
   - If `state.jobs.active().await` is `Some(active_job)` whose `backend_type` matches, return 409 with `{ "error": "a job is currently running for this backend" }`.
   - Call `safe_remove_installation(&info)`. If it returns an error containing `"outside the managed backends directory"`, map to 409 with `{ "error": "path is outside the managed backends directory; remove manually" }`. Other errors → 500.
   - Remove the registry row.
   - Return 200 with `{ "removed": true }`.

5. **Check-updates handler** (`POST /api/backends/check-updates`, empty body):
   - For each installed backend in the registry, call `check_updates` (the existing core function).
   - Return the same shape as `list_backends` with `update.checked: true` and populated `latest_version` / `update_available`.

6. **Router updates** in `server.rs`:
   - Add the four new routes to the `backends_routes` sub-router.
   - Extend the `CorsLayer` to include `POST` and `DELETE` in `allow_methods`. **The middleware still enforces same-origin for these methods** — the CORS layer just controls preflight responses; the rejection happens in `enforce_same_origin`.

**Steps:**
- [ ] Read `crates/koji-core/src/backends/installer/mod.rs` for the exact `InstallOptions` field names.
- [ ] Read `crates/koji-core/src/backends/mod.rs` for `check_latest_version` and `check_updates` signatures.
- [ ] Write failing test `test_install_returns_202_and_job_id`: POST a valid install request, assert 202 and a job_id is returned.
- [ ] Write failing test `test_concurrent_install_returns_409`: POST install twice in quick succession (use a long-running mock or just rely on the first job not finishing within the test); assert second response is 409 with `{"error": ..., "job_id": ...}`.
- [ ] Write failing test `test_install_ik_llama_forces_source_build`: POST with `backend_type: "ik_llama"`, `build_from_source: false`; assert response notices array contains the forced-upgrade message. To verify the upgrade actually happened, read the job's first log line or check via a test-only hook on the installer; if too invasive, just assert the notice is present.
- [ ] Write failing test `test_install_linux_cuda_forces_source_build`: **do not cfg-gate on `target_os = "linux"`** — the forcing logic keys off the *capabilities* `os` field, not compile-time OS. Seed the capabilities cache with `os: "linux"` (use the same stub pattern as the cmake-missing test), POST with `backend_type: "llama_cpp"`, `gpu_type: { kind: "cuda", version: "12.4" }`, `build_from_source: false`. Assert the notice is present. This keeps the test meaningful on macOS/Windows CI.
- [ ] Write failing test `test_install_source_build_with_missing_cmake_returns_400`: stub the capabilities cache (insert a fresh entry with `cmake_available: false`) before the request; POST install with source build; assert 400 with an error mentioning `cmake`.
- [ ] Write failing test `test_delete_404_on_unknown`.
- [ ] Write failing test `test_delete_200_on_known`: seed a registry entry whose `path` is inside a temp dir that's stubbed as `backends_dir()` (or use a test-only override of `backends_dir()`; alternatively, skip with a comment if the helper isn't easily overridable and rely on the path-traversal guard test for coverage).
- [ ] Write failing test `test_delete_path_traversal_returns_409`: seed a registry entry with `path: "/usr/local/bin/llama-server"`; DELETE; assert 409 with the documented error.
- [ ] Write failing test `test_delete_while_job_running_returns_409`: submit a job for `llama_cpp`, then DELETE `llama_cpp`; assert 409.
- [ ] Write failing test `test_origin_enforcement_blocks_cross_origin_post`: POST install with `Origin: http://evil.example` and `Host: 127.0.0.1:8080`; assert 403. (Re-enabled from Task 4.)
- [ ] Run `cargo test --package koji-web --test backends_api`
  - All new tests should fail to compile.
- [ ] Implement `JobAdapter`, the four handlers, and update the router.
- [ ] Run `cargo test --package koji-web --test backends_api`
  - All tests pass.
- [ ] Run `cargo test --workspace`.
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(web): add backends mutation API with source-build forcing and CSRF guard`

**Acceptance criteria:**
- [ ] All four new routes work end-to-end against a test registry.
- [ ] Server-side source-build forcing fires for `ik_llama` and Linux+CUDA `llama_cpp`, with notices in the response.
- [ ] Source build with missing prereqs returns 400, no job started.
- [ ] Concurrent submits return 409 with the existing job id.
- [ ] DELETE rejects paths outside `backends_dir()` with 409.
- [ ] DELETE while a same-backend job is running returns 409.
- [ ] Cross-origin POST is rejected with 403.
- [ ] All workspace tests, clippy, and fmt pass.

---

### Task 6: Job endpoints — snapshot + SSE event stream

**Context:**
Add `GET /api/backends/jobs/:id` (snapshot) and `GET /api/backends/jobs/:id/events` (SSE). The SSE handler implements the head + skipped-marker + tail replay protocol from §6.3 and emits a synthesized `[N lines dropped]` event when the broadcast channel laps. If the job is already terminal at connect time, replay the buffered logs + final status, then close. Otherwise close on terminal status from the live stream.

**Files:**
- Modify: `crates/koji-web/src/api/backends.rs` (add `get_job` and `job_events_sse`)
- Modify: `crates/koji-web/src/server.rs` (register the two routes under the new sub-router)
- Modify: `crates/koji-web/tests/backends_api.rs`

**What to implement:**

1. **Snapshot handler:**
   ```rust
   #[derive(Debug, serde::Serialize)]
   #[serde(rename_all = "snake_case")]
   pub struct JobSnapshotDto {
       pub id: String,
       pub kind: JobKind,
       pub status: JobStatus,
       pub backend_type: String,
       pub started_at: i64,
       pub finished_at: Option<i64>,
       pub error: Option<String>,
   }

   pub async fn get_job(
       State(state): State<Arc<AppState>>,
       Path(id): Path<String>,
   ) -> Result<Json<JobSnapshotDto>, StatusCode> {
       let job = state.jobs.get(&id).await.ok_or(StatusCode::NOT_FOUND)?;
       let s = job.state.read().await;
       Ok(Json(JobSnapshotDto { ... }))
   }
   ```

2. **SSE handler** — implement exactly the body shown in §6.3 of the spec (lines 357-407). Key points:
   - Subscribe to `log_tx` **before** snapshotting head/tail. (See Task 6 implementation note below about ordering.)
   - Read `log_head` (cloned out), `log_dropped`, `log_tail` (cloned out), and the terminal-at-connect flag.
   - Use `async_stream::stream! { ... }` to produce a `Stream<Item = Result<Event, axum::Error>>`.
   - Replay: head lines → optional `[... N lines skipped ...]` marker if `dropped > 0` → tail lines.
   - If terminal at connect: emit a `status` event and `return`.
   - Otherwise: loop on `rx.recv().await`, forwarding `Log` and `Status` events. On `Lagged(n)`: yield a `log` event with line `[{n} lines dropped]`, then `continue`. On `Closed`: `return`.
   - Wrap in `Sse::new(stream).keep_alive(KeepAlive::default())`.

   **Seam race — must fix, not document.** Naively, if `append_log` writes to the buffer and broadcasts as two independent steps, and SSE subscribes and snapshots as two independent steps, a line can appear in both the snapshot and the live stream (duplicate) or neither (loss) depending on interleaving. The fix is to **serialize buffer-write + broadcast-send under the same lock, and take that same lock across SSE's snapshot + subscribe**:

   1. **Amend Task 3's `JobManager::append_log`** (retroactive note: the executing agent for this task should go back and adjust `jobs.rs`): wrap `log_head`, `log_tail`, and `log_dropped` inside a single `tokio::sync::Mutex<LogBuffer>` struct. Inside `append_log`, take the mutex, mutate the buffer, call `log_tx.send(JobEvent::Log(line))` **while still holding the mutex**, then drop the mutex.
   2. **In the SSE handler**, take the same mutex to snapshot head + dropped + tail, then call `log_tx.subscribe()`, **then** drop the mutex. Any future `append_log` call blocks on the mutex until the subscribe is complete; any concurrent `append_log` that already held the mutex has its broadcast delivered to existing subscribers only (SSE isn't one yet), and its line is visible in the snapshot once SSE takes the lock.

   This gives at-most-once delivery across the replay/live seam without needing sequence numbers.

   Update the `Job` struct accordingly: instead of three independent `RwLock` fields, use `pub log: tokio::sync::Mutex<LogBuffer>` where `struct LogBuffer { head: VecDeque<String>, tail: VecDeque<String>, dropped: u64 }`. The `log_tx` stays as a sibling field. This is a small retroactive edit to Task 3 — update Task 3's unit tests to call through the new accessor shape.

**Steps:**
- [ ] Verify `async-stream` is in `koji-web/Cargo.toml` (added in Task 4). If not, add it under `[dependencies]` gated to ssr.
- [ ] Write failing test `test_get_job_404_on_unknown`.
- [ ] Write failing test `test_get_job_returns_snapshot_after_finish`: submit a job, finish it as `Succeeded`, GET `/api/backends/jobs/:id`, assert status is `succeeded`.
- [ ] Write failing test `test_sse_replays_buffered_lines_after_terminal`: submit a job, append 5 log lines via the JobManager, finish as `Succeeded`, then connect SSE; collect the events from the response body (use `eventsource-stream` or parse the `text/event-stream` format manually); assert 5 `log` events + 1 terminal `status` event then close.
- [ ] Write failing test `test_sse_emits_skipped_marker_when_dropped_nonzero`: submit a job, append 1000 lines (filling head, overflowing tail, leaving `log_dropped > 0`), finish, then connect SSE; assert one of the events is a log line containing `lines skipped`.
- [ ] Write failing test `test_sse_lagged_marker_emitted_to_slow_subscriber`: submit a job, subscribe SSE but don't read for a while, append >1024 lines fast enough to trigger `Lagged`, then read and assert one of the events is a log line containing `lines dropped`. **This test is racy by nature** — if it's flaky, gate it with `#[ignore]` and document the expected manual verification.
- [ ] Write failing test `test_sse_disconnect_does_not_kill_worker`: spawn a job task that takes ~200ms to complete (use a sleep), connect SSE, drop the connection immediately, wait for the job to finish, then GET the job snapshot and assert `status == Succeeded`.
- [ ] Run `cargo test --package koji-web --test backends_api`
  - All new tests should fail to compile.
- [ ] Implement `get_job` and `job_events_sse`. Wire both into `backends_routes` in `server.rs`.
- [ ] Run `cargo test --package koji-web --test backends_api`.
- [ ] Run `cargo test --workspace`, `cargo fmt --all`, `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(web): add job snapshot and SSE event stream endpoints`

**Acceptance criteria:**
- [ ] `GET /api/backends/jobs/:id` returns the snapshot DTO; 404 on unknown.
- [ ] `GET /api/backends/jobs/:id/events` replays head + skipped marker (if any) + tail before live tailing.
- [ ] Lagged broadcast subscribers see a `[N lines dropped]` log event.
- [ ] SSE client disconnects don't kill the underlying job worker.
- [ ] All tests, clippy, and fmt pass.

---

### Task 7: Graceful shutdown and stale build-dir cleanup

**Context:**
Per spec §6.4, the current `axum::serve(...)` call has no `with_graceful_shutdown`, so spawned subprocesses survive parent death and leave half-built trees that block the next install. Two mitigations: (a) graceful shutdown that kills any `Running` job's child processes, and (b) stale-build-dir detection on startup that surfaces leftovers via a UI cleanup affordance. The cleanup endpoint and UI button are added here too.

**Files:**
- Modify: `crates/koji-web/src/server.rs`
- Modify: `crates/koji-web/src/jobs.rs` (track child PIDs in `Job`, add `kill_active`)
- Modify: `crates/koji-core/src/backends/installer/source.rs` (publish child PIDs back to caller — see implementation note)
- Modify: `crates/koji-web/src/api/backends.rs` (add `GET /api/backends/stale` and `POST /api/backends/stale/cleanup` endpoints)

**What to implement:**

1. **Child PID tracking — `ProgressSink` extension bundled with Task 2 work:**
   - Extend `ProgressSink` (defined in Task 1) with an optional `register_child(pid: u32)` method, default no-op:
     ```rust
     pub trait ProgressSink: Send + Sync {
         fn log(&self, line: &str);
         fn register_child(&self, _pid: u32) {}
     }
     ```
     Because `NullSink` and any other impl use the default, this is non-breaking.
   - Add `pub child_pids: tokio::sync::Mutex<Vec<u32>>` to `Job` (Task 3's struct — retroactive edit).
   - In `source.rs::run_command` (the helper introduced in Task 2): after `cmd.spawn()?`, call `if let Some(pid) = child.id() { if let Some(p) = progress { p.register_child(pid); } }`. This is a **one-line addition** to the helper Task 2 already created — no restructuring of the spawn sites.
   - `JobAdapter::register_child` pushes the PID into `job.child_pids`.
   - Add `JobManager::kill_active(&self) -> impl Future<Output = ()>` that, if there is an active job, reads `child_pids` and sends `SIGTERM` (Unix) / `TerminateProcess` (Windows). On Unix, use `nix::sys::signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM)`. Add `nix` as a Unix-only dep if not present.

2. **Graceful shutdown:**
   - Replace `axum::serve(listener, app).await?` with `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal(state.clone())).await?`.
   - Implement `async fn shutdown_signal(state: Arc<AppState>)`:
     ```rust
     let ctrl_c = async { tokio::signal::ctrl_c().await.expect("ctrl_c handler"); };
     #[cfg(unix)]
     let terminate = async {
         tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
             .expect("sigterm handler").recv().await;
     };
     #[cfg(not(unix))]
     let terminate = std::future::pending::<()>();
     tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
     state.jobs.kill_active().await;
     ```

3. **Stale build dir detection:**
   - In `JobManager::new`, scan `koji_core::backends::backends_dir()`. For each subdirectory `<name>/`:
     - If `<name>/build/` exists AND the registry has no row for `<name>` whose `path` resolves to a real file inside `<name>/`, mark it stale.
   - Store the stale list in `JobManager`: `pub stale_dirs: RwLock<Vec<PathBuf>>`.
   - Log a warning at startup: `tracing::warn!("Found N stale backend build directories: {:?}", paths);`. (If `tracing` isn't used, fall back to `eprintln!`.)

4. **Cleanup endpoints:**
   - `GET /api/backends/stale` → returns `{ "stale_dirs": ["llama_cpp", ...] }` (just the basenames).
   - `POST /api/backends/stale/cleanup` → for each stale dir, run `safe_remove_installation`-style canonical-path check then `remove_dir_all`. Return `{ "cleaned": [...] }`.
   - Both endpoints go behind `enforce_same_origin`.

**Steps:**
- [ ] Read `crates/koji-web/Cargo.toml` for existing tracing/log setup. Confirm whether `nix` (or `rustix`) is available; add if missing.
- [ ] Add `register_child` default method to `ProgressSink` in `koji-core`. **This is a trait extension** — verify no downstream impl breaks (`NullSink` keeps the default).
- [ ] Update `source.rs` to call `progress.register_child(child.id().unwrap_or(0))` after each `Command::spawn`.
- [ ] Write failing test `test_kill_active_no_active_is_noop` in `jobs.rs` tests.
- [ ] Write failing test `test_register_child_appends_pid`: submit a job, get a `JobAdapter` for it, call `register_child(12345)`, assert `job.child_pids` contains 12345.
- [ ] Write failing test `test_stale_dir_detection_finds_orphaned_build_dirs`: in a temp dir stubbed as `backends_dir()`, create `llama_cpp/build/` with no registry entry, instantiate JobManager, assert `stale_dirs` contains `llama_cpp`. **If stubbing `backends_dir()` is too invasive, skip and rely on manual verification.**
- [ ] Write failing test `test_get_stale_endpoint`.
- [ ] Run `cargo test --package koji-web`
  - New tests should fail.
- [ ] Implement child PID tracking, `kill_active`, graceful shutdown, stale-dir scan, and the two cleanup endpoints.
- [ ] Run `cargo test --workspace`.
- [ ] Run `cargo fmt --all`, `cargo clippy --workspace -- -D warnings`.
- [ ] Commit: `feat(web): graceful shutdown and stale build directory cleanup`

**Acceptance criteria:**
- [ ] Server traps SIGINT/SIGTERM and kills active job's child processes before exiting.
- [ ] On startup, `JobManager` detects orphaned `<backend>/build/` directories with no matching registry entry.
- [ ] `GET /api/backends/stale` and `POST /api/backends/stale/cleanup` work and are origin-protected.
- [ ] `ProgressSink::register_child` has a default no-op so existing impls don't break.
- [ ] All workspace tests, clippy, and fmt pass.

---

### Task 8: UI components — `BackendCard`, `InstallModal`, `JobLogPanel`

**Context:**
Build the Leptos components that render each card state, the install modal, and the reusable log panel that consumes an `EventSource`. This task focuses on the components in isolation; the integration into `BackendsForm` and the API client wiring happens in Task 9. Delete the dead `backends_section.rs` file in this task as cleanup.

**Files:**
- Create: `crates/koji-web/src/components/backend_card.rs`
- Create: `crates/koji-web/src/components/install_modal.rs`
- Create: `crates/koji-web/src/components/job_log_panel.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (declare new modules, remove dead one)
- Delete: `crates/koji-web/src/components/backends_section.rs`

**What to implement:**

1. **`BackendCard`** (`backend_card.rs`):
   - Props: `card: BackendCardDto`, `active_job: Option<ActiveJobDto>`, callbacks for `on_install`, `on_update`, `on_uninstall`.
   - Renders the six states from §7.2 of the spec.
   - Header badge text/color follows the table.
   - Body content follows the table.
   - Action buttons follow the table; the `⋮` kebab menu opens an Uninstall confirm modal (inline state, no separate component needed).
   - Advanced disclosure (§7.3) wraps the existing path/args/health/version inputs. **For this task, render placeholder text** ("Advanced settings (TODO: wire to config form)"); the real wiring happens in Task 9.

2. **`InstallModal`** (`install_modal.rs`):
   - Props: `backend_type: String`, `capabilities: Resource<CapabilitiesDto>`, callbacks `on_submit(InstallRequest)`, `on_cancel()`.
   - Fields per §7.4:
     - GPU radio group (CPU / CUDA / ROCm / Vulkan / Metal).
     - CUDA version dropdown — only shown when CUDA selected; options from `capabilities.supported_cuda_versions`; default to nearest match of `capabilities.detected_cuda_version`.
     - ROCm: static text "ROCm 7.2 (hardcoded)".
     - Version text input (placeholder `latest`); hidden for `ik_llama`.
     - Build-from-source checkbox: forced on + disabled for `ik_llama`; forced on + disabled when OS is Linux and selected GPU is CUDA.
     - Force-overwrite checkbox.
   - Warning banner at top when `cmake_available || git_available || compiler_available` is false **and** source build is selected. Disables the Install button.
   - Buttons: Cancel, Install. Submit builds an `InstallRequest` and calls `on_submit`.
   - Reads `std::env::consts::OS` via the capabilities response (`capabilities.os`), not the WASM `cfg!`.

3. **`JobLogPanel`** (`job_log_panel.rs`):
   - Props: `job_id: String`, `compact: bool` (compact = inline 6-line tail; full = expandable).
   - On mount, opens `EventSource("/api/backends/jobs/<id>/events")`.
   - Maintains a `RwSignal<Vec<String>>` of log lines (bounded; cap at ~600 to match server's head + tail + a margin).
   - Listens for `log` events (parse `data` JSON, extract `line`) and `status` events (parse `status`, close source).
   - On terminal status, the panel emits a parent callback `on_terminal: Callback<JobStatus>`.
   - Renders a `<pre>` block scrolled to bottom; in compact mode shows the last 6 lines + an "Expand" button.
   - On unmount, closes the EventSource cleanly.

4. **Module wiring:**
   - In `crates/koji-web/src/components/mod.rs`, declare `pub mod backend_card; pub mod install_modal; pub mod job_log_panel;` and remove the `mod backends_section;` line.
   - Delete `crates/koji-web/src/components/backends_section.rs`.

**Steps:**
- [ ] Read `crates/koji-web/src/components/mod.rs` and a representative existing component (e.g., a current modal in `components/`) to understand Leptos patterns used in this codebase.
- [ ] Read `crates/koji-web/src/components/backends_section.rs` to confirm it's truly unused before deleting (`grep -r backends_section crates/koji-web/src/`).
- [ ] Write component tests using `leptos::testing` if the project already uses it; **otherwise skip per-component tests** and rely on Task 9's integration smoke tests. Component tests in Leptos require non-trivial setup; do not introduce a new test framework just for this task.
- [ ] Implement `BackendCard` rendering all 6 states. Use a simple match-based dispatch on `(card.installed, card.update.update_available, active_job)` to pick the state.
- [ ] Implement `InstallModal` with smart defaults from `CapabilitiesDto`.
- [ ] Implement `JobLogPanel` with `EventSource` lifecycle.
- [ ] Update `components/mod.rs` and delete `backends_section.rs`.
- [ ] Run `cargo build --workspace` (no test step since we're skipping component-level tests).
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`. Components must compile clean under both `csr` and `ssr` features if the project uses both.
- [ ] Commit: `feat(web): add BackendCard, InstallModal, and JobLogPanel components`

**Acceptance criteria:**
- [ ] Three new Leptos components exist and compile under all feature flags the project uses.
- [ ] `backends_section.rs` is deleted; nothing references it.
- [ ] `cargo build --workspace`, clippy, and fmt all pass.

---

### Task 9: Integration — replace `BackendsForm`, wire API, end-to-end smoke

**Context:**
Wire the components from Task 8 into `BackendsForm`, hook them to the HTTP API, and verify the full flow with manual smoke tests. This task replaces the current `BackendsForm` body with a fixed two-card render, adds an API client module, handles SSE rehydration after page reload, and integrates the existing `config.backends` editing into the Advanced disclosure.

**Files:**
- Modify: `crates/koji-web/src/pages/config_editor.rs` (replace `BackendsForm`)
- Create: `crates/koji-web/src/api_client/backends.rs` (or extend existing api_client module if one exists)
- Modify: `crates/koji-web/src/components/backend_card.rs` (replace Advanced disclosure placeholder with real config inputs)

**What to implement:**

1. **API client** (`api_client/backends.rs` or similar — check what the existing pattern is):
   ```rust
   pub async fn get_backends() -> Result<BackendListResponse, ClientError>;
   pub async fn get_capabilities() -> Result<CapabilitiesDto, ClientError>;
   pub async fn check_updates() -> Result<BackendListResponse, ClientError>;
   pub async fn install_backend(req: InstallRequest) -> Result<InstallResponse, ClientError>;
   pub async fn update_backend(name: &str) -> Result<InstallResponse, ClientError>;
   pub async fn remove_backend(name: &str) -> Result<(), ClientError>;
   pub async fn get_job(id: &str) -> Result<JobSnapshotDto, ClientError>;
   pub async fn get_stale() -> Result<StaleResponse, ClientError>;
   pub async fn cleanup_stale() -> Result<CleanupResponse, ClientError>;
   ```
   Use whatever HTTP client convention `koji-web` already uses (likely `reqwasm`, `gloo-net`, or `leptos::server_fn` — read the existing api_client to confirm).

2. **`BackendsForm` rewrite** in `pages/config_editor.rs`:
   - Replace the existing function body. Keep the function signature and component name.
   - Create a `Resource` for `get_backends()`, refetched on mount and after every job-completion callback.
   - Create a `Resource` for `get_stale()`; if non-empty, show a yellow banner with a "Clean up stale build directories" button calling `cleanup_stale()`.
   - Render a top-level "Check for updates" button calling `check_updates()` and updating the resource.
   - Iterate `backends` and render a `<BackendCard>` for each, plus any `custom` entries as read-only rows below.
   - Pass the active install/update modal state via local signals (`RwSignal<Option<String>>` for "modal open for backend X").
   - On `on_install` callback from a card, open the install modal for that backend.
   - On `on_update`, POST to update API and store the returned `job_id` in a per-card signal so `BackendCard` switches to "job running" state and renders `JobLogPanel`.
   - On `on_uninstall`, show confirm modal then DELETE.
   - On page load, if `active_job` is `Some`, immediately put the matching card in "job running" state and mount `JobLogPanel` for the existing `job_id` — this is the rehydration path.

3. **Advanced disclosure wiring** in `BackendCard`:
   - Replace the placeholder with the existing config inputs from the previous `BackendsForm` body (path, default_args, health_check_url, version).
   - These continue to flow through the existing `POST /api/config/structured` save path. **Do not change the save flow.**
   - The path field is read-only when `card.installed` is true (auto-populated from `info.path`); editable otherwise. A "Reset to managed path" button next to it sets the value back to `info.path` if installed.

4. **First-use security banner** (§7a item 3):
   - Extend `CapabilitiesDto` (defined in Task 4) with `pub server_bound_to_loopback: bool` and extend `AppState` with `pub bound_to_loopback: bool`, populated when the listener binds in `server.rs`.
   - **Retroactive Task 4 updates required:** update the capabilities handler to include this field (always read from `AppState::bound_to_loopback`, not from `spawn_blocking`), update `test_get_capabilities_returns_supported_cuda_versions` to assert the field is present, and update `tests/fixtures/backends_list.json` if it references the capabilities shape.
   - When `false`, render a one-time dismissible banner above the cards: "⚠ Koji is bound to a non-loopback address. Anyone on your network can install backends here."
   - "One-time dismissible" can be `localStorage`-backed in the browser; if too much scope, just make it always-show-when-applicable for now.

**Steps:**
- [ ] Read `crates/koji-web/src/pages/config_editor.rs` to understand the existing `BackendsForm`, the structured-config save flow, and the Leptos patterns in use.
- [ ] Read `crates/koji-web/src/api_client/` (or wherever the existing client lives) to mirror its style.
- [ ] Extend `CapabilitiesDto` (in Task 4's file) and `system_capabilities` handler (in Task 4's file) to include `server_bound_to_loopback: bool`. The listener address has to be threaded into `AppState` from `server.rs` — add `pub bound_to_loopback: bool` to `AppState`, populate when the listener binds.
- [ ] Implement the API client module.
- [ ] Rewrite `BackendsForm` per the structure above. Wire the modal state, job state, rehydration on load, and stale-dir banner.
- [ ] Replace the Task 8 placeholder in `BackendCard`'s Advanced disclosure with real config input bindings.
- [ ] Run `cargo build --workspace`.
- [ ] Run `cargo test --workspace`.
- [ ] Run `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`.
- [ ] **Manual smoke tests** (per spec §9.5 — run as many as time allows):
  - [ ] 35: Install `llama.cpp` prebuilt CUDA from a fresh registry; observe live logs, completion, card state transition.
  - [ ] 36: Install `ik_llama` from source; observe build logs and final success.
  - [ ] 37: Trigger update on an installed backend; observe version bump.
  - [ ] 38: Trigger uninstall; verify files removed and card returns to "Not installed".
  - [ ] 39: Reload page mid-install; confirm card rehydrates.
  - [ ] 40: Force a failure (disconnect network during prebuilt download); confirm Failed state and Retry button.
- [ ] Commit: `feat(web): wire BackendsForm to install/update/SSE flow`

**Acceptance criteria:**
- [ ] `BackendsForm` renders two cards backed by `GET /api/backends`.
- [ ] Install / Update / Uninstall flows work end-to-end against a real `koji-core` registry.
- [ ] Live build logs stream into `JobLogPanel` via SSE.
- [ ] Page reload mid-install rehydrates the running job and re-attaches the SSE stream.
- [ ] Existing config.backends path/args/health/version editing still works through the Advanced disclosure.
- [ ] Stale build dir cleanup affordance appears when applicable.
- [ ] Loopback warning banner appears when bound to non-loopback.
- [ ] All workspace tests, clippy, and fmt pass.
- [ ] All six manual smoke tests in §9.5 pass on at least one platform.

---

## Scope concessions vs. spec §9.4

Spec §9.4 lists four Leptos component tests (#31-34) for `BackendCard` / `InstallModal`. Leptos component testing requires non-trivial harness setup that doesn't currently exist in this workspace. **These tests are deferred** — Task 9's manual smoke tests (§9.5 #35-40) cover the same surface area end-to-end. If a Leptos test harness lands in this project later, retroactively add the component tests as a follow-up.

## Failed-state Retry flow

Spec §7.2 mentions a "Retry" button for the Job-failed state. Wiring:
- `BackendCard` emits `on_retry` when the Retry button is clicked (only visible when the last completed job for this backend was `Failed`).
- `BackendsForm` handles `on_retry` by reopening the Install modal pre-populated with the same `InstallRequest` the failed job used. This requires `BackendsForm` to cache the last-submitted `InstallRequest` keyed by backend name (`RwSignal<HashMap<String, InstallRequest>>`). Clear on successful completion.
- "Dismiss" is a separate button that clears the failed-job state locally and transitions the card back to its pre-job state (Installed / Not installed) based on the current registry.

---

## Out-of-band notes for the executing agent

- **If a task's tests are too invasive to write meaningfully** (e.g., the stale-dir scan in Task 7 or the lagged-marker test in Task 6), it is acceptable to mark them `#[ignore]` with a comment explaining why and note them in the commit message. Do not skip silently.
- **If `koji-cli` fails to compile after Task 2,** you broke the wrapper invariant — fix the wrapper, do not edit CLI call sites. The whole point of the wrapper approach is zero CLI churn.
- **If the JSON snapshot test in Task 4 fails after a serde change,** that is the test working as intended. Do not "fix" it by regenerating the snapshot blindly — figure out which DTO field drifted and decide whether the change was intentional.
- **The `BackendType::Custom` path is partially handled.** If the registry contains a custom entry, it appears in `custom: []` in the list response and as a read-only row in the UI, with no install/update/uninstall actions. Do not add wiring beyond that.
