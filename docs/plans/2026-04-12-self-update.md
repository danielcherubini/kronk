# Koji Self-Update Plan

**Goal:** Allow users to update the Koji binary from both the Web UI (update button) and CLI (`koji self-update`), downloading new releases from GitHub and auto-restarting.

**Architecture:** A new `self_update` module in `koji-core` uses the `self_update` crate's lower-level API for GitHub release discovery, download, and extraction, with `self_replace` (bundled) for cross-platform binary swap. The Web UI gets a version badge in the sidebar footer with an "Update" button that streams progress via SSE. The CLI gets a `koji self-update` subcommand. The CI pipeline is updated to produce archives named with target triples for `self_update` crate compatibility.

**Tech Stack:** `self_update` 0.44 (with `archive-tar`, `archive-zip`, `compression-flate2`, `compression-zip-deflate`, `rustls` features), `semver` 1, Leptos 0.7 (CSR), Axum 0.7 (SSR), existing `tokio::sync::broadcast` pattern for SSE.

---

## Task 1: Add `self_update` and `semver` Dependencies to koji-core

**Context:**
Before any self-update logic can be written, the workspace needs the `self_update` and `semver` crates. The `self_update` crate provides GitHub release discovery, asset download, archive extraction, and internally uses `self_replace` for cross-platform binary swapping. The `semver` crate handles version comparison. These are added to `koji-core` because the core library owns all backend logic (backends, models, platform) and the self-update module fits that pattern.

**Files:**
- Modify: `crates/koji-core/Cargo.toml`

**What to implement:**
Add two new dependencies to `crates/koji-core/Cargo.toml` under `[dependencies]`:

```toml
self_update = { version = "0.43", default-features = false, features = ["archive-tar", "compression-flate2", "archive-zip", "compression-zip-deflate", "rustls"] }
semver = "1"
```

The `self_update` features are:
- `archive-tar` + `compression-flate2`: for Linux `.tar.gz` archives
- `archive-zip` + `compression-zip-deflate`: for Windows `.zip` archives
- `rustls`: TLS backend (avoids OpenSSL dependency, consistent with koji's existing use of `rustls-tls` for reqwest)

Do NOT enable `self_update`'s `reqwest` feature — it defaults to `ureq` for HTTP, which avoids conflicts with koji's existing reqwest dependency.

**Steps:**
- [ ] Add `self_update` and `semver` to `crates/koji-core/Cargo.toml` under `[dependencies]`, inserting alphabetically
- [ ] Run `cargo check --package koji-core`
  - Did it succeed? If not, resolve dependency conflicts (likely reqwest version) and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat: add self_update and semver dependencies to koji-core`

**Acceptance criteria:**
- [ ] `cargo check --package koji-core` succeeds with no errors
- [ ] `self_update` and `semver` appear in `Cargo.lock`

---

## Task 2: Implement `koji_core::self_update` Module

**Context:**
This is the core self-update logic. It lives in `koji-core` following the pattern of other core modules (backends, models, platform). The module provides three functions: (1) check if an update is available by querying GitHub Releases, (2) perform the update by downloading + extracting + replacing the binary, and (3) restart the process. The `self_update` crate's API is synchronous, so all calls must be wrapped in `tokio::task::spawn_blocking`. The restart function uses koji's existing platform service management (`platform::linux::restart_service` / `platform::windows::restart_service`) when running as a service, or re-execs the binary for CLI mode.

**Files:**
- Create: `crates/koji-core/src/self_update.rs`
- Modify: `crates/koji-core/src/lib.rs` (add `pub mod self_update;`)

**What to implement:**

Create `crates/koji-core/src/self_update.rs` with these types and functions:

1. **Types:**
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub struct UpdateInfo {
       pub current_version: String,
       pub latest_version: String,
       pub release_notes: String,
       pub published_at: String,
       pub update_available: bool,
   }

   #[derive(Debug, Clone)]
   pub struct UpdateResult {
       pub old_version: String,
       pub new_version: String,
   }
   ```

2. **`check_for_update(current_version: &str) -> Result<UpdateInfo>`:**
   - Accepts `current_version` as a parameter (NOT `env!("CARGO_PKG_VERSION")`) so the caller passes the correct binary version. This avoids version mismatch since `env!("CARGO_PKG_VERSION")` resolves to the crate's own version at compile time, and koji-core's version may differ from koji-cli's.
   - Use `self_update::backends::github::ReleaseList::configure()` with `.repo_owner("danielcherubini")` and `.repo_name("koji")` to fetch releases.
   - Wrap the sync `.fetch()` call in `tokio::task::spawn_blocking`.
   - Compare using `semver::Version::parse()` on the `current_version` parameter.
   - Return `UpdateInfo` with `update_available` set based on semver comparison.
   - If no releases found, return `UpdateInfo` with `update_available: false` and `latest_version` set to current version.
   - Handle `GITHUB_TOKEN` env var for authentication (pass to self_update via `.auth_token()` if present), following the pattern in `backends/updater.rs`.

3. **`perform_update(current_version: &str, on_progress: impl Fn(String) + Send + 'static) -> Result<UpdateResult>`:**
   - Accepts `current_version` as a parameter (same rationale as `check_for_update`).
   - Use the `self_update` crate's **lower-level API** for fine-grained progress control (NOT the high-level `Update::configure().update()` which provides no progress callbacks):
     a. Call `on_progress("Checking for latest release...")` 
     b. Use `ReleaseList::configure().repo_owner(...).repo_name(...).build()?.fetch()?` to get releases
     c. Find the latest release, compare versions with semver
     d. Call `on_progress(format!("Downloading v{}...", new_version))`
     e. Use `self_update::Download::from_url(&asset_url)` to download to a temp file
     f. Call `on_progress("Extracting binary...")`
     g. Use `self_update::Extract::from_source(&tmp_path).archive(archive_kind).extract_file(&tmp_dir, &bin_name)?` to extract
     h. Call `on_progress("Replacing binary...")`
     i. Use `self_replace::self_replace(&extracted_binary_path)?` to swap the running binary
     j. Call `on_progress("Update complete!")`
   - The archive kind is platform-specific: `ArchiveKind::Tar(Some(Compression::Gz))` on Linux, `ArchiveKind::Zip` on Windows. Use `#[cfg(target_os = "...")]` to select.
   - The asset name to look for: `koji-x86_64-unknown-linux-gnu.tar.gz` (Linux) or `koji-x86_64-pc-windows-msvc.zip` (Windows). Match the asset by checking if its name contains the target triple (`env!("TARGET")` or hardcode).
   - Wrap all sync calls in `tokio::task::spawn_blocking`.
   - Return `UpdateResult` with old and new versions.
   - On version already up to date, return `anyhow::bail!("Already up to date (v{version})")`.
   - If `GITHUB_TOKEN` env var is set, add auth header to download requests.

4. **`restart_process() -> Result<()>`:**
   - Detect if running as a systemd service on Linux: check if `INVOCATION_ID` env var is set (systemd sets this for both system and user services). As a fallback, also check if parent PID is 1 or if `JOURNAL_STREAM` env var is set.
   - Detect if running as a Windows service: check if we were launched via the `service-run` command by examining `std::env::args()`.
   - If service on Linux: call `crate::platform::linux::restart_service("koji")`. If that fails (e.g., service not installed), log a warning and fall back to CLI re-exec behavior.
   - If service on Windows: call `crate::platform::windows::restart_service("koji")`. If that fails, fall back to CLI re-exec.
   - Otherwise (CLI mode): get `std::env::current_exe()`, collect `std::env::args().skip(1)`, spawn new process via `std::process::Command::new(exe).args(args).spawn()`, then `std::process::exit(0)`.
   - Helper function: `fn is_running_as_service() -> bool` that encapsulates the detection logic for both platforms.

5. **Constants:**
   ```rust
   pub const REPO_OWNER: &str = "danielcherubini";
   pub const REPO_NAME: &str = "koji";
   ```

Add `pub mod self_update;` to `crates/koji-core/src/lib.rs` (insert alphabetically after `pub mod proxy;`).

**Steps:**
- [ ] Create `crates/koji-core/src/self_update.rs` with the types, constants, and three functions described above
- [ ] Add `pub mod self_update;` to `crates/koji-core/src/lib.rs` after the `pub mod proxy;` line
- [ ] Run `cargo check --package koji-core`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-core -- -D warnings`
  - Did it succeed? If not, fix lint warnings and re-run.
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: `feat: implement koji-core self_update module with check, update, and restart`

**Acceptance criteria:**
- [ ] `koji_core::self_update::check_for_update()` compiles and returns `Result<UpdateInfo>`, accepts `current_version: &str` parameter
- [ ] `koji_core::self_update::perform_update()` compiles, accepts `current_version: &str` and progress callback, uses lower-level API for fine-grained progress
- [ ] `koji_core::self_update::restart_process()` compiles with platform-aware restart logic and fallback
- [ ] `is_running_as_service()` helper correctly detects systemd (INVOCATION_ID / JOURNAL_STREAM) and Windows service (service-run arg)
- [ ] All sync `self_update` crate calls are wrapped in `spawn_blocking`
- [ ] `cargo check --package koji-core` passes
- [ ] `cargo clippy --package koji-core -- -D warnings` passes

---

## Task 3: Add Web API Endpoints for Self-Update

**Context:**
The Web UI needs API endpoints: one to check if an update is available, one to trigger the update, and one to stream progress via SSE. These follow the existing pattern in `koji-web/src/api/` — Axum handlers with `State<Arc<AppState>>` and JSON responses. The update uses a two-step flow: a POST to trigger the update (protected by same-origin middleware against CSRF), then a GET SSE stream for progress. This matches the browser's EventSource API (GET-only) and reuses the proven SSE consumption pattern from `job_log_panel.rs` which uses `gloo_net::eventsource::futures::EventSource`.

**Files:**
- Create: `crates/koji-web/src/api/self_update.rs`
- Modify: `crates/koji-web/src/api.rs` (add `pub mod self_update;`)
- Modify: `crates/koji-web/src/server.rs` (add routes, add `binary_version` + `update_tx` to AppState, update `run_with_opts` signature)
- Modify: `crates/koji-cli/src/handlers/serve.rs` (pass `binary_version` to `run_with_opts`)
- Modify: `crates/koji-cli/src/service.rs` (pass `binary_version` to `run_with_opts`)
- Modify: `crates/koji-cli/src/handlers/web.rs` (pass `binary_version` to `run_with_opts`)

**What to implement:**

1. **Create `crates/koji-web/src/api/self_update.rs`:**

   Response types (derive `Serialize`, `Deserialize`):
   ```rust
   #[derive(Serialize, Deserialize)]
   pub struct UpdateCheckResponse {
       pub update_available: bool,
       pub current_version: String,
       pub latest_version: String,
       pub release_notes: String,
       pub published_at: String,
   }

   #[derive(Serialize, Deserialize)]
   pub struct UpdateTriggerResponse {
       pub ok: bool,
       pub message: String,
   }
   ```

   **Shared state for update SSE:** Add fields to `AppState` in `server.rs`:
   ```rust
   // In server.rs AppState, add:
   pub binary_version: String,  // The actual koji binary version, passed from CLI
   pub update_tx: Arc<tokio::sync::Mutex<Option<broadcast::Sender<String>>>>
   ```
   - `binary_version`: The version of the running koji binary, NOT `env!("CARGO_PKG_VERSION")` (which resolves to koji-web's crate version). This must be passed in from the CLI when starting the web server. Use `tokio::sync::Mutex` for consistency with async code (even though the lock is held briefly).
   - `update_tx`: Initialize as `Arc::new(tokio::sync::Mutex::new(None))` in `run_with_opts`.
   - Update `run_with_opts` to accept a `binary_version: String` parameter (add as the last parameter) and pass it through to `AppState`. Also update the convenience `run()` wrapper to pass a default version.
   - **All callers of `run_with_opts` must be updated to pass the binary version:**
     - `crates/koji-cli/src/handlers/serve.rs` (line ~94): pass `env!("CARGO_PKG_VERSION").to_string()` — this is koji-cli's version, the correct one
     - `crates/koji-cli/src/service.rs` (line ~269): pass `env!("CARGO_PKG_VERSION").to_string()`
     - `crates/koji-cli/src/handlers/web.rs` (line ~13): pass `env!("CARGO_PKG_VERSION").to_string()`
     - `server.rs` `run()` wrapper (line ~212): pass `env!("CARGO_PKG_VERSION").to_string()` (koji-web's version as fallback)

   Handlers:

   a. **`check_update`** — `GET /api/self-update/check`:
   - Call `koji_core::self_update::check_for_update(&state.binary_version).await` — uses the actual binary version from AppState, NOT `env!("CARGO_PKG_VERSION")`.
   - Map `UpdateInfo` to `UpdateCheckResponse`
   - On error, return 502 with JSON error body (GitHub API might be unreachable)

   b. **`trigger_update`** — `POST /api/self-update/update` (placed inside `backend_routes` for same-origin CSRF protection):
   - Create a `tokio::sync::broadcast::channel::<String>(64)` for progress messages
   - Store the sender in `state.update_tx`
   - Clone `state.binary_version` for use in the spawned task
   - Spawn a background `tokio::spawn` task that:
     1. Calls `koji_core::self_update::perform_update(&binary_version, progress_callback)` where the callback sends messages to the broadcast channel
     2. On success, sends JSON via channel: `{"type": "status", "status": "succeeded", "old_version": "...", "new_version": "..."}`
     3. Then sends `{"type": "restarting"}`
     4. Waits 500ms, then calls `koji_core::self_update::restart_process()`
     5. On failure, sends `{"type": "status", "status": "failed", "error": "..."}`
   - Returns `Json(UpdateTriggerResponse { ok: true, message: "Update started" })`
   - If an update is already in progress (sender exists and has receivers), return 409 Conflict

   c. **`update_events`** — `GET /api/self-update/events` (SSE stream):
   - Returns `Sse<impl Stream<Item = Result<Event, axum::Error>>>`
   - Gets the broadcast sender from `state.update_tx`, subscribes to it
   - If no update is in progress (sender is None), return an immediate "no update in progress" event and close
   - Uses `async_stream::stream!` to yield SSE events:
     - `Event::default().event("log").json_data(json!({ "line": message }))` for progress (matches existing `job_events_sse` format)
     - `Event::default().event("status").json_data(json!({ "status": "succeeded", "old_version": "...", "new_version": "..." }))` for completion
     - `Event::default().event("status").json_data(json!({ "status": "failed", "error": "..." }))` for failure
     - `Event::default().event("restarting").json_data(json!({}))` before restart
   - Use `Sse::new(stream).keep_alive(KeepAlive::default())`
   - Import pattern: `use async_stream::stream;`, `use axum::response::sse::{Event, KeepAlive};`, `use axum::response::Sse;`, `use futures_util::Stream;`, `use tokio::sync::broadcast;`

2. **Modify `crates/koji-web/src/api.rs`:**
   - Add `pub mod self_update;` after the existing `pub mod middleware;` line

3. **Modify `crates/koji-web/src/server.rs`:**
   - Add `update_tx: Arc::new(Mutex::new(None))` to `AppState` construction
   - Add the POST route **inside the `backend_routes` sub-router** (which has `enforce_same_origin` middleware), around line 141 where the other backend POST routes are:
     ```rust
     .route("/api/self-update/update", post(api::self_update::trigger_update))
     ```
   - Add the GET routes to the **main router** (safe methods don't need CSRF protection):
     ```rust
     .route("/api/self-update/check", get(api::self_update::check_update))
     .route("/api/self-update/events", get(api::self_update::update_events))
     ```

**Steps:**
- [ ] Add `binary_version: String` and `update_tx: Arc<tokio::sync::Mutex<Option<broadcast::Sender<String>>>>` fields to `AppState` in `server.rs`
- [ ] Update `run_with_opts` to accept `binary_version: String` parameter and pass to AppState
- [ ] Initialize `update_tx` as `Arc::new(tokio::sync::Mutex::new(None))`
- [ ] Update all callers of `run_with_opts` to pass `binary_version`:
  - `crates/koji-cli/src/handlers/serve.rs`: pass `env!("CARGO_PKG_VERSION").to_string()`
  - `crates/koji-cli/src/service.rs`: pass `env!("CARGO_PKG_VERSION").to_string()`
  - `crates/koji-cli/src/handlers/web.rs`: pass `env!("CARGO_PKG_VERSION").to_string()`
  - `server.rs` `run()` wrapper: pass `env!("CARGO_PKG_VERSION").to_string()`
- [ ] Create `crates/koji-web/src/api/self_update.rs` with `check_update`, `trigger_update`, and `update_events` handlers
- [ ] Add `pub mod self_update;` to `crates/koji-web/src/api.rs` after the `pub mod middleware;` line
- [ ] Add POST route inside `backend_routes` sub-router in `server.rs`
- [ ] Add GET routes to main router in `server.rs`
- [ ] Run `cargo check --package koji-web --features ssr`
  - Did it succeed? If not, fix compilation errors (likely missing imports) and re-run.
- [ ] Run `cargo check --workspace` (to verify all callers of `run_with_opts` compile)
  - Did it succeed? If not, fix the callers and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web --features ssr -- -D warnings`
  - Did it succeed? If not, fix lint warnings and re-run.
- [ ] Commit with message: `feat: add self-update API endpoints with SSE progress streaming`

**Acceptance criteria:**
- [ ] `GET /api/self-update/check` handler compiles and returns JSON `UpdateCheckResponse`, uses `state.binary_version` (not compile-time macro)
- [ ] `POST /api/self-update/update` handler is inside `backend_routes` (has same-origin middleware protection)
- [ ] `GET /api/self-update/events` handler returns SSE stream using broadcast channel
- [ ] SSE events use `.json_data()` with JSON payloads (matching existing `job_events_sse` format in `api/backends.rs`)
- [ ] Two-step flow: POST triggers update → GET streams progress (compatible with browser EventSource)
- [ ] `AppState` has `binary_version: String` field populated from CLI
- [ ] Routes are registered in `build_router()`
- [ ] `cargo check --package koji-web --features ssr` passes

---

## Task 4: Add Version Badge and Update Button to Sidebar

**Context:**
The sidebar (`crates/koji-web/src/components/sidebar.rs`) currently shows navigation links and a collapse toggle in the footer. We need to add a version badge that shows the current version, checks for updates on mount, and shows an "Update" button when an update is available. The update flow uses a two-step API: POST to trigger the update (via `gloo_net::http::Request`), then GET SSE to stream progress (via `gloo_net::eventsource::futures::EventSource`) — this exactly matches the proven pattern in `job_log_panel.rs`. This is a CSR (client-side rendered) component using Leptos 0.7 signals.

**Files:**
- Modify: `crates/koji-web/src/components/sidebar.rs`
- Modify: `crates/koji-web/style.css`

**What to implement:**

1. **Modify `crates/koji-web/src/components/sidebar.rs`:**

   Add these reactive signals at the top of the `Sidebar` component (after the existing `collapsed` and `mobile_open` signals):
   ```rust
   let current_version = RwSignal::new(String::new());
   let update_available = RwSignal::new(false);
   let latest_version = RwSignal::new(String::new());
   let update_in_progress = RwSignal::new(false);
   let update_status = RwSignal::new(String::new()); // Shows progress messages
   let show_update_confirm = RwSignal::new(false);
   ```

   **Version check on mount:** Use `leptos::task::spawn_local` to make an async call on mount:
   ```rust
   leptos::task::spawn_local(async move {
       if let Ok(resp) = gloo_net::http::Request::get("/api/self-update/check")
           .send()
           .await
       {
           if let Ok(data) = resp.json::<serde_json::Value>().await {
               if let Some(v) = data["current_version"].as_str() {
                   current_version.set(v.to_string());
               }
               if data["update_available"].as_bool() == Some(true) {
                   update_available.set(true);
                   if let Some(v) = data["latest_version"].as_str() {
                       latest_version.set(v.to_string());
                   }
               }
           }
       }
   });
   ```

   **Confirm handler (two-step: POST trigger → GET SSE stream):**
   Create a closure that:
   1. Sets `show_update_confirm` to false, `update_in_progress` to true, `update_status` to "Starting update..."
   2. Fires a POST to trigger the update:
      ```rust
      let trigger_result = gloo_net::http::Request::post("/api/self-update/update")
          .send()
          .await;
      ```
   3. If POST succeeds, open an EventSource to stream progress (matching `job_log_panel.rs` pattern):
      ```rust
      use gloo_net::eventsource::futures::EventSource;
      let mut es = EventSource::new("/api/self-update/events")
          .expect("failed to open EventSource");
      let (mut log_stream, _) = es.subscribe("log").expect("subscribe log");
      let (mut status_stream, _) = es.subscribe("status").expect("subscribe status");
      ```
   4. Use `futures_util::future::select` in a loop to handle both streams (exact same pattern as `job_log_panel.rs`):
      - On "log" event: parse JSON payload (`{"line": "..."}`) and set `update_status` signal with the `line` value
      - On "status" event: parse JSON payload (`{"status": "succeeded", ...}` or `{"status": "failed", "error": "..."}`):
        - If "succeeded": set `update_status` to "Updated! Restarting..."
        - If "failed": set `update_status` to error message, set `update_in_progress` to false
   5. After the "restarting" or "succeeded" event, close the EventSource and start reconnection polling
   6. If POST fails, set `update_status` to error message, set `update_in_progress` to false

   **Reconnection logic**: After the SSE stream indicates success/restart:
   - Poll `GET /api/self-update/check` every 2 seconds using `gloo_timers::future::TimeoutFuture`
   - When it responds and the version has changed, show "Updated successfully to v{new_version}!"
   - After 5 failed attempts (10 seconds), show "Server is restarting. Please refresh manually."

   **View additions** — Add to the sidebar footer, between the Config link and the collapse toggle button:
   ```rust
   // Version badge (always visible)
   <div class="sidebar-version">
       <span class="sidebar-version__text">
           {move || {
               let cv = current_version.get();
               if update_available.get() {
                   format!("v{} → v{}", cv, latest_version.get())
               } else if !cv.is_empty() {
                   format!("v{}", cv)
               } else {
                   String::new()
               }
           }}
       </span>
       // Update button (visible when update available and not in progress)
       {move || update_available.get().then(|| view! {
           <button
               class="sidebar-update-btn"
               disabled=move || update_in_progress.get()
               on:click=move |_| show_update_confirm.set(true)
           >
               {move || if update_in_progress.get() { "Updating..." } else { "Update" }}
           </button>
       })}
   </div>

   // Confirmation dialog (overlay)
   {move || show_update_confirm.get().then(|| view! {
       <div class="update-confirm-overlay">
           <div class="update-confirm-dialog">
               <p>{format!("Update Koji to v{}?", latest_version.get())}</p>
               <p class="update-confirm-note">"Koji will restart after updating."</p>
               <div class="update-confirm-actions">
                   <button class="btn btn--secondary" on:click=move |_| show_update_confirm.set(false)>"Cancel"</button>
                   <button class="btn btn--primary" on:click=confirm_update>"Update"</button>
               </div>
           </div>
       </div>
   })}

   // Progress overlay (shown during update)
   {move || update_in_progress.get().then(|| view! {
       <div class="update-progress-overlay">
           <div class="update-progress-dialog">
               <div class="update-progress-spinner"></div>
               <p>{move || update_status.get()}</p>
           </div>
       </div>
   })}
   ```

   Note: Version info comes from the API response (NOT `env!("CARGO_PKG_VERSION")` which would be koji-web's version). The `current_version` signal is populated from the `/api/self-update/check` response.

2. **Modify `crates/koji-web/style.css`:**

   Add these CSS classes (add them after the existing `.sidebar-toggle` styles, before the Layout / main content section):

   ```css
   /* Sidebar Version Badge */
   .sidebar-version {
       padding: 0.5rem 1rem;
       display: flex;
       align-items: center;
       justify-content: space-between;
       gap: 0.5rem;
       border-top: 1px solid var(--border-color);
       font-size: 0.75rem;
       color: var(--text-secondary);
   }

   .sidebar--collapsed .sidebar-version {
       padding: 0.5rem;
       justify-content: center;
   }

   .sidebar-version__text {
       white-space: nowrap;
       overflow: hidden;
       text-overflow: ellipsis;
   }

   .sidebar--collapsed .sidebar-version__text {
       display: none;
   }

   .sidebar-update-btn {
       padding: 0.2rem 0.5rem;
       font-size: 0.7rem;
       background: var(--color-success);
       color: white;
       border: none;
       border-radius: var(--radius-sm);
       cursor: pointer;
       white-space: nowrap;
       transition: background var(--transition-fast);
   }

   .sidebar-update-btn:hover {
       background: var(--color-success-hover, #2ea043);
   }

   .sidebar-update-btn:disabled {
       opacity: 0.6;
       cursor: not-allowed;
   }

   .sidebar--collapsed .sidebar-update-btn {
       padding: 0.2rem;
       font-size: 0.6rem;
   }

   /* Update confirmation dialog */
   .update-confirm-overlay,
   .update-progress-overlay {
       position: fixed;
       top: 0;
       left: 0;
       right: 0;
       bottom: 0;
       background: rgba(0, 0, 0, 0.6);
       display: flex;
       align-items: center;
       justify-content: center;
       z-index: 1000;
   }

   .update-confirm-dialog,
   .update-progress-dialog {
       background: var(--bg-secondary);
       border: 1px solid var(--border-color);
       border-radius: var(--radius-md);
       padding: 1.5rem;
       max-width: 400px;
       width: 90%;
       text-align: center;
   }

   .update-confirm-note {
       color: var(--text-secondary);
       font-size: 0.85rem;
       margin-top: 0.5rem;
   }

   .update-confirm-actions {
       display: flex;
       gap: 0.75rem;
       justify-content: center;
       margin-top: 1rem;
   }

   .update-progress-spinner {
       width: 32px;
       height: 32px;
       border: 3px solid var(--border-color);
       border-top-color: var(--color-primary);
       border-radius: 50%;
       animation: spin 0.8s linear infinite;
       margin: 0 auto 1rem;
   }

   @keyframes spin {
       to { transform: rotate(360deg); }
   }
   ```

   Check if `--color-success` already exists in the CSS variables. If not, add `--color-success: #238636;` to the `:root` block.

**Steps:**
- [ ] Add version signals and update check logic to `crates/koji-web/src/components/sidebar.rs`
- [ ] Add two-step update flow: POST trigger + EventSource SSE stream (matching `job_log_panel.rs` pattern)
- [ ] Add version badge, update button, confirmation dialog, and progress overlay to the sidebar view
- [ ] Add reconnection logic using polling after update
- [ ] Add CSS styles to `crates/koji-web/style.css`
- [ ] Run `cargo check --package koji-web` (this checks the WASM/CSR target)
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji-web -- -D warnings`
  - Did it succeed? If not, fix lint warnings and re-run.
- [ ] Commit with message: `feat: add version badge and update button to web UI sidebar`

**Acceptance criteria:**
- [ ] Sidebar shows version badge in footer area
- [ ] Version info comes from `/api/self-update/check` API (not compile-time macro)
- [ ] Version badge shows current version when no update available
- [ ] Version badge shows update indicator and "Update" button when update is available
- [ ] Click "Update" shows confirmation dialog
- [ ] Update flow: POST `/api/self-update/update` → EventSource on `/api/self-update/events` (matching `job_log_panel.rs` pattern)
- [ ] During update, a progress overlay with spinner shows real-time log messages
- [ ] After server restarts, the UI polls and reconnects
- [ ] Styles match the existing dark theme
- [ ] `cargo check --package koji-web` passes (CSR)
- [ ] `cargo check --package koji-web --features ssr` passes (SSR)

---

## Task 5: Add `koji self-update` CLI Command

**Context:**
Users should be able to update Koji from the command line as well. This follows the existing CLI pattern: add a variant to the `Commands` enum in `cli.rs`, create a handler in `handlers/self_update.rs`, and wire it up in the match statement in `lib.rs`. The CLI command supports `--check` (only check, don't install) and `--force` (skip version comparison).

**Files:**
- Modify: `crates/koji-cli/src/cli.rs` (add `SelfUpdate` command variant)
- Create: `crates/koji-cli/src/handlers/self_update.rs` (command handler)
- Modify: `crates/koji-cli/src/handlers/mod.rs` (add `pub mod self_update;`)
- Modify: `crates/koji-cli/src/lib.rs` (add match arm for `SelfUpdate`)

**What to implement:**

1. **Modify `crates/koji-cli/src/cli.rs`:**
   Add a new variant to the `Commands` enum, after the `Logs` variant and before the `Web` variant:
   ```rust
   /// Update koji to the latest version from GitHub
   SelfUpdate {
       /// Only check for updates, don't install
       #[arg(long)]
       check: bool,
       /// Skip version comparison, always download latest
       #[arg(long)]
       force: bool,
   },
   ```

2. **Create `crates/koji-cli/src/handlers/self_update.rs`:**
   ```rust
   pub async fn cmd_self_update(check: bool, force: bool) -> Result<()>
   ```
   Implementation:
   - Get the current version: `let current_version = env!("CARGO_PKG_VERSION");` (this is koji-cli's version, which is the binary being updated — correct!)
   - Call `koji_core::self_update::check_for_update(current_version).await?`
   - Print: `"Current version: v{current_version}"`
   - Print: `"Latest version:  v{latest_version}"`
   - If no update available and not `force`: print "Already up to date!" and return Ok
   - If `check`: return Ok (just print info)
   - Print: `"Updating to v{latest_version}..."`
   - Call `koji_core::self_update::perform_update(current_version, |msg| println!("  {}", msg)).await?`
   - Print: `"Successfully updated from v{old} to v{new}!"`
   - Print: `"Please restart koji to use the new version."`
   - Note: Do NOT auto-restart for CLI mode — the user invoked a one-shot command, not a server. Just tell them to restart.
   - Return Ok

3. **Modify `crates/koji-cli/src/handlers/mod.rs`:**
   Add `pub mod self_update;` after `pub mod status;`

4. **Modify `crates/koji-cli/src/lib.rs`:**
   Add the match arm in the `match args.command` block, after the `Commands::Logs` arm and before the `Commands::Web` arm:
   ```rust
   Commands::SelfUpdate { check, force } => {
       handlers::self_update::cmd_self_update(check, force).await
   }
   ```

**Steps:**
- [ ] Add `SelfUpdate` variant to `Commands` enum in `crates/koji-cli/src/cli.rs`
- [ ] Create `crates/koji-cli/src/handlers/self_update.rs` with `cmd_self_update` function
- [ ] Add `pub mod self_update;` to `crates/koji-cli/src/handlers/mod.rs`
- [ ] Add `Commands::SelfUpdate` match arm to `crates/koji-cli/src/lib.rs`
- [ ] Run `cargo check --package koji`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package koji -- -D warnings`
  - Did it succeed? If not, fix lint warnings and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Commit with message: `feat: add koji self-update CLI command`

**Acceptance criteria:**
- [ ] `koji self-update --check` compiles and would print version info
- [ ] `koji self-update` compiles and would download + replace binary
- [ ] `koji self-update --force` compiles and would skip version check
- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

## Task 6: Update CI Release Workflow for Target-Triple Archives

**Context:**
The `self_update` crate expects release assets to contain the target triple in their filename (e.g., `koji-x86_64-unknown-linux-gnu.tar.gz`). The current release workflow uploads flat-named files (`koji`, `koji.exe`). We need to add steps that create properly-named archives. Existing assets are kept for backward compatibility (manual download users, package managers).

**Files:**
- Modify: `.github/workflows/release.yml`

**What to implement:**

1. **In the `build-linux` job**, after the "Build release" step and before "Install packaging tools":
   Add a step to create a tar.gz archive:
   ```yaml
   - name: Create self-update archive (Linux)
     run: |
       cd target/release
       tar czf koji-x86_64-unknown-linux-gnu.tar.gz koji
   ```

2. **In the `build-linux` job**, update the "Upload artifacts" step to include the archive:
   Add `target/release/koji-x86_64-unknown-linux-gnu.tar.gz` to the `path` list.

3. **In the `build-windows` job**, after the "Build release" step and before "Install Inno Setup":
   Add a step to create a zip archive:
   ```yaml
   - name: Create self-update archive (Windows)
     shell: pwsh
     run: |
       Compress-Archive -Path target/release/koji.exe -DestinationPath target/release/koji-x86_64-pc-windows-msvc.zip
   ```

4. **In the `build-windows` job**, update the "Upload artifacts" step to include the archive:
   Add `target/release/koji-x86_64-pc-windows-msvc.zip` to the `path` list.

5. **In the `release` job**, update the `files` list in the "Create Release" step:
   Add these two lines to the `files` section:
   ```yaml
   linux/**/koji-x86_64-unknown-linux-gnu.tar.gz
   windows/**/koji-x86_64-pc-windows-msvc.zip
   ```

All existing file uploads remain unchanged for backward compatibility.

**Steps:**
- [ ] Add archive creation steps to both `build-linux` and `build-windows` jobs in `.github/workflows/release.yml`
- [ ] Add archive files to artifact upload paths
- [ ] Add archive files to the release file list
- [ ] Verify YAML syntax is valid (check indentation carefully)
- [ ] Run `cargo fmt --all` (just to be safe, even though no Rust files changed)
- [ ] Commit with message: `ci: add target-triple archives to release workflow for self-update`

**Acceptance criteria:**
- [ ] Release workflow creates `koji-x86_64-unknown-linux-gnu.tar.gz` containing the `koji` binary
- [ ] Release workflow creates `koji-x86_64-pc-windows-msvc.zip` containing `koji.exe`
- [ ] Both archives are uploaded as release assets alongside existing files
- [ ] Existing release assets (`koji`, `koji.exe`, `.deb`, `.rpm`, installer) are unchanged
- [ ] YAML is syntactically valid

---

## Task Dependency Order

```
Task 1 (deps) → Task 2 (core module) → Task 3 (web API) → Task 4 (sidebar UI)
                                      → Task 5 (CLI, independent of web tasks)
Task 6 (CI) is independent, can be done in parallel with any task
```

Recommended execution order: 1 → 2 → 3 → 4 → 5 → 6 (or 6 can be done anytime)
