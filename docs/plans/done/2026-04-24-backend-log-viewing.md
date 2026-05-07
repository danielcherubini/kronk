# Backend Log Viewing Plan

**Goal:** Allow users to view logs for any individual backend from the dashboard's Active Models section via a "Logs" button that opens a live-updating log viewer in a modal dialog.

**Architecture:** Add a new REST endpoint `GET /tama/v1/logs/:backend` that reads per-backend log files using the existing `tama_core::logging` infrastructure. Create a new Leptos component `BackendLogPanel` that polls this endpoint every 1 second and displays logs with auto-refresh toggle. Wire it into the dashboard's model rows via the existing `<Modal>` component pattern.

**Tech Stack:** Rust, Axum (web server), Leptos (WASM frontend), gloo_net (HTTP client in WASM), `tama_core::logging` (existing log file I/O).

---

### Task 1: Backend Log API Endpoint

**Context:**
The existing `/tama/v1/logs` endpoint only returns logs for the main application (`tama.log`). Backends write their own log files at `logs_dir/{backend_name}.log` using `tama_core::logging::log_path()`. We need a new endpoint to retrieve these per-backend logs. This is a pure backend change — no frontend involved yet.

**Files:**
- Create: `crates/tama-web/src/api/logs.rs`
- Modify: `crates/tama-web/src/api.rs` (add `pub mod logs;`)
- Modify: `crates/tama-web/src/server.rs` (add route)
- Test: `crates/tama-web/src/api/logs.rs` (inline tests for validation)

**What to implement:**

1. **New module `api/logs.rs`:**
   - Define `BackendLogsQuery` struct with `lines: usize` field, default 200
   - Define `MAX_LINES: usize = 10_000` constant
   - Implement `get_backend_logs` async handler:
     - Extract `logs_dir` from `state.logs_dir`, return 404 if None with body `{ "error": "logs_dir not configured" }`
     - Validate backend name using `is_valid_backend_name()`, return 400 if invalid with body `{ "error": "Invalid backend name" }`
     - Build log path as `dir.join(format!("{}.log", backend))`
     - Check file existence, return 404 if not found with body `{ "error": "No logs found for '<backend>'" }`
     - Use `tokio::task::spawn_blocking` to call `tama_core::logging::tail_lines(&path, n.min(MAX_LINES))`
     - On error from `tail_lines`, log `tracing::warn!("Failed to read backend log {}: {}", path.display(), e)` and return empty vec
     - Return 200 with `{ "lines": [...] }`
   - Implement `is_valid_backend_name(name: &str) -> bool`:
     - Returns `true` if name is non-empty, ≤64 chars, and contains only alphanumeric, underscore, or hyphen characters
     - Use: `!name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-')`

2. **Module registration in `api.rs`:**
   - Add `pub mod logs;` after existing `pub mod` declarations (around line 8)

3. **Route registration in `server.rs`:**
   - In `build_router()` function, add route immediately after the existing `/tama/v1/logs` route:
     ```rust
     .route("/tama/v1/logs", get(api::get_logs))
     .route("/tama/v1/logs/:backend", get(api::logs::get_backend_logs))
     ```
   - This must be in the main router chain, before `.merge(csrf_routes)` and before `.route("/tama/v1/*path", any(proxy_tama))`

**Steps:**
- [ ] Create new file `crates/tama-web/src/api/logs.rs` with the full module implementation
- [ ] Run `cargo test --package tama-web api::logs` — verify all tests pass
  - Valid names: `"llama_cpp"`, `"ik_llama"`, `"tts_kokoro"`, `"custom-backend"`, `"abc123"`
  - Invalid names: `""` (empty), `"../etc/passwd"`, `"../../logs"`, `"name with spaces"`, `"name/with/slashes"`, `"name..double"`, 65+ character string, `"name.with.dots"`, `"name\0null"`, `"UPPER CASE"`
- [ ] Run `cargo test --package tama-web api::logs` — verify all tests pass
- [ ] Implement `get_backend_logs` handler in `crates/tama-web/src/api/logs.rs` following the spec above
- [ ] Add `pub mod logs;` to `crates/tama-web/src/api.rs` (after existing module declarations, around line 8)
- [ ] In `build_router()` in `crates/tama-web/src/server.rs`, add the route immediately after the `/tama/v1/logs` route:
  ```rust
  .route("/tama/v1/logs", get(api::get_logs))
  .route("/tama/v1/logs/:backend", get(api::logs::get_backend_logs))
  ```
  This must be in the main router chain, before `.merge(csrf_routes)` and before `.route("/tama/v1/*path", any(proxy_tama))`
- [ ] Run `cargo check --package tama-web` — verify no compilation errors
- [ ] Run `cargo build --package tama-web` — verify clean build
- [ ] Commit with message: `"feat(web): add backend-specific log endpoint GET /tama/v1/logs/:backend"`

**Acceptance criteria:**
- [ ] `is_valid_backend_name()` correctly accepts valid backend names and rejects all invalid/traversal attempts
- [ ] Endpoint returns 200 with `{ "lines": [...] }` for existing log files
- [ ] Endpoint returns 400 with error JSON for invalid backend names
- [ ] Endpoint returns 404 with error JSON when logs_dir is not configured or file doesn't exist
- [ ] `spawn_blocking` properly handles I/O errors (logs warning, returns empty vec)
- [ ] `lines` parameter is clamped to MAX_LINES (10,000)

---

### Task 2: BackendLogPanel Component

**Context:**
The dashboard needs a component to display backend logs with live updating. This follows the existing `JobLogPanel` pattern but uses HTTP polling instead of SSE. The component must work in WASM target (no `Send` futures). It reuses the CSS classes and patterns from the existing `/logs` page (`crates/tama-web/src/pages/logs.rs`).

**Files:**
- Create: `crates/tama-web/src/components/backend_log_panel.rs`
- Modify: `crates/tama-web/src/components/mod.rs` (add module)

**What to implement:**

1. **New component `BackendLogPanel`:**
   - Props: `backend_name: String`, `on_close: Option<Callback<()>>`
   - State signals:
     - `lines: RwSignal<Vec<String>>` — current log lines
     - `loading: RwSignal<bool>` — initial loading state
     - `error: RwSignal<Option<String>>` — error message
     - `auto_refresh: RwSignal<bool>` — whether polling is active (default true)
     - `last_fetch: RwSignal<std::time::Instant>` — last poll time
   - Fetch function (spawned via `wasm_bindgen_futures::spawn_local`):
     - Uses `gloo_net::http::Request::get(&format!("/tama/v1/logs/{}", backend))`
     - Calls `extract_and_store_csrf_token(&resp)` after receiving response
     - On success (200): parse JSON `{ "lines": [...] }`, update `lines`, set `loading=false`, clear `error`
     - On 404: set error message `"No logs found for '<backend>'"`, clear lines
     - On other errors: set error from response text
     - On network error: set error from exception
     - Cap buffer at 1000 lines (FIFO drain when exceeded)
   - Auto-refresh Effect:
     - Runs every frame, checks if `auto_refresh.get()` is true and if `Instant::now() - last_fetch >= 1000ms`
     - If so, calls fetch function and updates `last_fetch`
   - Mount Effect: immediate fetch on component mount
   - UI layout (dark theme matching JobLogPanel):
     - Header bar (background `#1e293b`): shows `"📋 {backend_name} logs"` on left, three buttons on right (`"↻ Refresh"`, `"Pause"`/`"Resume"`, `"×"`)
     - Content area (overflow-y: auto, flex: 1): displays log lines with color coding
   - `log_level_class()` helper function (same as in `pages/logs.rs`):
     - Returns `"log-line--error"` for ERROR/FATAL
     - Returns `"log-line--warn"` for WARN
     - Returns `"log-line--debug"` for DEBUG
     - Returns `"log-line--info"` otherwise

2. **Module registration in `components/mod.rs`:**
   - Add `pub mod backend_log_panel;` to the module list

**Steps:**
- [ ] Create `crates/tama-web/src/components/backend_log_panel.rs` with full component implementation
- [ ] Add `pub mod backend_log_panel;` to `crates/tama-web/src/components/mod.rs`
- [ ] Add imports at top of file: `use gloo_net::http::Request;`, `use serde::Deserialize;`, `use wasm_bindgen_futures::spawn_local;`, `use crate::utils::extract_and_store_csrf_token;`
- [ ] Run `cargo check --package tama-web` — verify no compilation errors
- [ ] Verify the component compiles without errors by running: `cargo build --package tama-web`
- [ ] Commit with message: `"feat(web): add BackendLogPanel component with auto-refresh polling"`

**Acceptance criteria:**
- [ ] Component compiles for WASM target (no `Send` violations)
- [ ] Polls API every 1000ms when auto_refresh is enabled
- [ ] Pause/Resume toggle correctly starts/stops polling
- [ ] Manual Refresh button triggers immediate fetch
- [ ] Lines are capped at 1000 with FIFO drain
- [ ] Log level color coding works (ERROR=red, WARN=yellow, DEBUG=dim)
- [ ] Empty state shows "No logs yet..." message
- [ ] Error state shows error message from API response

---

### Task 3: Dashboard Integration

**Context:**
Wire the new `BackendLogPanel` component into the dashboard so users can view backend logs from any model row. This involves adding a "Logs" button to each model row in the Active Models section, creating modal state signals, and rendering the `BackendLogPanel` inside the existing `<Modal>` component.

**Files:**
- Modify: `crates/tama-web/src/pages/dashboard.rs`

**What to implement:**

1. **New imports at top of `dashboard.rs`:**
   ```rust
   use crate::components::modal::Modal;
   use crate::components::backend_log_panel::BackendLogPanel;
   ```

2. **New state signals** (place after existing `unload_pending` signal):
   ```rust
   let log_panel_open = RwSignal::new(false);
   let log_panel_backend = RwSignal::new(Option::<String>::None());
   ```

3. **New callbacks:**
   ```rust
   let on_log_click = Callback::new(move |backend: String| {
       log_panel_backend.set(Some(backend));
       log_panel_open.set(true);
   });

   let on_log_close = Callback::new(move |_| {
       log_panel_open.set(false);
   });
   ```

4. **New button in model row actions** (inside the `model-row__actions` div, between the Load/Unload button and the Edit `<A>` link):
   ```rust
   <button
       class="btn btn-secondary btn-sm"
       title=format!("View logs for {}", m.backend)
       on:click=move |_| { on_log_click.run(m.backend.clone()); }
   >
       "Logs"
   </button>
   ```

5. **Modal rendering** (at the bottom of the Dashboard view, inside the main `view!` block, before the final closing braces):
   ```rust
   <Modal
       open=log_panel_open
       title=move || format!("{} logs", log_panel_backend.get().unwrap_or_else(|| "Backend".to_string()))
       on_close=on_log_close
   >
       {move || {
           log_panel_backend.get().map(|backend| {
               view! {
                   <BackendLogPanel
                       backend_name=backend.clone()
                       on_close=Some(on_log_close.clone())
                   />
               }.into_any()
           })
       }}
   </Modal>
   ```

**Steps:**
- [ ] Add imports for `Modal` and `BackendLogPanel` to `dashboard.rs`
- [ ] Add `log_panel_open` and `log_panel_backend` signals after `unload_pending`
- [ ] Add `on_log_click` and `on_log_close` callbacks
- [ ] In the model row actions `<div class="model-row__actions">`, add the Logs button **after** the conditional Load/Unload button and **before** the `<A href=...>"Edit"</A>` link:
  ```rust
  <div class="model-row__actions">
      <span class={badge_class}>{badge_label}</span>
      {/* Load/Unload button (conditional) */}
      <button class="btn btn-secondary btn-sm" title=format!("View logs for {}", m.backend) on:click=move |_| { on_log_click.run(m.backend.clone()); }>
          "Logs"
      </button>  {/* NEW */}
      <A href=...>"Edit"</A>
  </div>
  ```
- [ ] Add Modal rendering at bottom of Dashboard view
- [ ] Run `cargo check --package tama-web` — verify no compilation errors
- [ ] Run `cargo build --package tama-web` — verify clean build
- [ ] Commit with message: `"feat(web): add Logs button to dashboard model rows with modal log viewer"`

**Acceptance criteria:**
- [ ] Each model row in Active Models section has a "Logs" button
- [ ] Clicking "Logs" opens a modal titled "{backend_name} logs"
- [ ] Modal displays live-updating backend logs via BackendLogPanel
- [ ] Closing modal (×, Escape, or backdrop click) stops polling and hides the panel
- [ ] No layout regressions in existing model row styling
- [ ] Button only appears for models that have a `backend` field set (all do, but verify)

---

## Task Dependencies

```
Task 1 (API endpoint) ──→ Task 3 (Dashboard integration)
                              ▲
Task 2 (Component) ─────────┘
```

- Task 1 can be done standalone and tested independently
- Task 2 is independent of Task 3 (component works without dashboard wiring)
- Task 3 depends on both Task 1 (API must exist) and Task 2 (component must exist)
- All tasks are independently commitable

## Testing Notes

- API validation tests are pure Rust unit tests (`cargo test`)
- Component testing is limited in WASM — relies on manual verification in browser
- Integration testing: open dashboard, click Logs button on a model row, verify logs display and auto-refresh works
- The existing `tama_core::logging` module already has comprehensive tests for `tail_lines`, `log_path`, and `open_log`

## Edge Cases Handled

- Backend name with path traversal characters → rejected by validation
- Log file doesn't exist → 404 from API, "No logs yet" in component
- `logs_dir` not configured → 404 from API
- I/O error reading log file → warning logged, empty lines returned (doesn't crash)
- Modal closed while polling is active → component cleanup via Leptos reactivity (signals dropped, polling stops)
- Rapid modal open/close → each mount triggers a new fetch; old in-flight requests complete but don't update stale signals
