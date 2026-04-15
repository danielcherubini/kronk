# Updates Center (`/updates`) Plan

**Goal:** Add a centralized `/updates` page to the web UI that shows available updates for backends and models, with a background checker that periodically polls GitHub/HuggingFace and caches results in SQLite.

**Architecture:** New `update_checks` DB table caches check results. A Tokio background task runs checks at a configurable interval (default 12h). The web UI reads cached results via `GET /api/updates` and can trigger manual checks or apply updates. Model updates use the existing GGUF LFS hash comparison; backend updates use GitHub API version comparison. Both apply flows go through `JobManager` for progress tracking.

**Tech Stack:** Rust, SQLite (rusqlite), Axum (API), Leptos 0.7 (frontend), Tokio (background task)

---

### Task 1: Database Migration & Query Functions

**Context:**
All update check results need persistent storage so they survive restarts and are instantly available to the frontend without re-checking. This task creates the `update_checks` table (migration v6) and all CRUD query functions. The existing migration system uses `PRAGMA user_version` and sequential version numbers — currently at v5. All query functions follow the sync `&Connection` pattern used throughout `crates/koji-core/src/db/queries/`.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs`
- Create: `crates/koji-core/src/db/queries/update_check_queries.rs`
- Modify: `crates/koji-core/src/db/queries/types.rs`
- Modify: `crates/koji-core/src/db/queries/mod.rs`
- Modify: `crates/koji-core/src/db/mod.rs` (add test for migration v6)

**What to implement:**

1. In `migrations.rs`: Add migration tuple `(6, ...)` to the `migrations` array. The SQL:
   ```sql
   CREATE TABLE IF NOT EXISTS update_checks (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       item_type TEXT NOT NULL,
       item_id TEXT NOT NULL,
       current_version TEXT,
       latest_version TEXT,
       update_available INTEGER NOT NULL DEFAULT 0,
       status TEXT NOT NULL DEFAULT 'unknown',
       error_message TEXT,
       details_json TEXT,
       checked_at INTEGER NOT NULL,
       UNIQUE(item_type, item_id)
   );
   CREATE INDEX IF NOT EXISTS idx_update_checks_type ON update_checks(item_type);
   ```
   Update `LATEST_VERSION` from `5` to `6`.

2. In `types.rs`: Add `UpdateCheckRecord` struct:
   ```rust
   #[derive(Debug, Clone)]
   pub struct UpdateCheckRecord {
       pub item_type: String,
       pub item_id: String,
       pub current_version: Option<String>,
       pub latest_version: Option<String>,
       pub update_available: bool,
       pub status: String,
       pub error_message: Option<String>,
       pub details_json: Option<String>,
       pub checked_at: i64,
   }
   ```

3. Create `update_check_queries.rs` with these functions (all take `&Connection`):
   - `upsert_update_check(conn, record: &UpdateCheckRecord) -> Result<()>` — INSERT ... ON CONFLICT(item_type, item_id) DO UPDATE. Use all fields from the record. `checked_at` comes from the record (caller provides unix timestamp).
   - `get_all_update_checks(conn) -> Result<Vec<UpdateCheckRecord>>` — SELECT * ORDER BY item_type, item_id.
   - `get_update_check(conn, item_type: &str, item_id: &str) -> Result<Option<UpdateCheckRecord>>` — SELECT WHERE item_type = ?1 AND item_id = ?2.
   - `get_update_checks_by_type(conn, item_type: &str) -> Result<Vec<UpdateCheckRecord>>` — SELECT WHERE item_type = ?1.
   - `delete_update_check(conn, item_type: &str, item_id: &str) -> Result<()>` — DELETE WHERE.
   - `delete_stale_update_checks(conn, item_type: &str, valid_ids: &[&str]) -> Result<u64>` — DELETE WHERE item_type = ?1 AND item_id NOT IN (?2...). Return number of deleted rows. If `valid_ids` is empty, delete all rows for that item_type.
   - `get_last_full_check_time(conn) -> Result<Option<i64>>` — `SELECT MIN(checked_at) FROM update_checks`. Returns None if table is empty.

4. In `queries/mod.rs`: Add `mod update_check_queries;` and `pub use update_check_queries::*;`.

**Steps:**
- [ ] Add `UpdateCheckRecord` to `crates/koji-core/src/db/queries/types.rs`
- [ ] Create `crates/koji-core/src/db/queries/update_check_queries.rs` with all 7 functions
- [ ] Add `mod update_check_queries;` and `pub use update_check_queries::*;` to `crates/koji-core/src/db/queries/mod.rs`
- [ ] Add migration v6 to `crates/koji-core/src/db/migrations.rs` and update `LATEST_VERSION` to `6`
- [ ] Write tests in `crates/koji-core/src/db/queries/update_check_queries.rs` (in a `#[cfg(test)] mod tests` block):
  - `test_upsert_and_get_update_check` — upsert a record, get it back, verify fields
  - `test_upsert_overwrites` — upsert twice with different values, verify second wins
  - `test_get_all_update_checks` — insert 3 records (2 backends, 1 model), verify all returned
  - `test_get_update_checks_by_type` — insert mixed types, filter by 'backend', verify only backends returned
  - `test_delete_update_check` — insert, delete, verify gone
  - `test_delete_stale_update_checks` — insert 3 items, call with valid_ids for 1, verify other 2 deleted
  - `test_get_last_full_check_time` — insert records with different checked_at, verify MIN returned
  - `test_migration_v6_creates_table` — use `open_in_memory()`, verify table exists in sqlite_master
  Each test uses `crate::db::open_in_memory()` to get a connection.
- [ ] Run `cargo test --package koji-core -- db::queries::update_check_queries::tests`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Fix any warnings and re-run.
- [ ] Commit with message: "feat: add update_checks DB table and query functions (migration v6)"

**Acceptance criteria:**
- [ ] Migration v6 creates `update_checks` table with correct schema
- [ ] `LATEST_VERSION` is `6`
- [ ] All 7 query functions work correctly
- [ ] All 8 tests pass
- [ ] `cargo clippy` clean

---

### Task 2: Configuration — `update_check_interval`

**Context:**
The background update checker needs a configurable interval. This goes in the `[general]` section of `config.toml`. Default is 12 hours. Setting to 0 disables automatic checks. There are THREE places where the `General` struct is defined that all need updating: the core config type, the web mirror type (used for JSON serialization), and the config editor page type (used for the config editor UI). Plus the `From` impls that convert between core and web types.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs`
- Modify: `crates/koji-web/src/types/config.rs`
- Modify: `crates/koji-web/src/pages/config_editor.rs`

**What to implement:**

1. In `crates/koji-core/src/config/types.rs`, add to the `General` struct:
   ```rust
   /// How often to check for updates, in hours. 0 = disabled. Default: 12.
   #[serde(default = "default_update_check_interval")]
   pub update_check_interval: u32,
   ```
   Add the default function OUTSIDE the struct impl block, at module level:
   ```rust
   fn default_update_check_interval() -> u32 { 12 }
   ```

2. In `crates/koji-web/src/types/config.rs`, add to the web mirror `General` struct:
   ```rust
   /// How often to check for updates, in hours. 0 = disabled. Default: 12.
   #[serde(default = "default_update_check_interval")]
   pub update_check_interval: u32,
   ```
   Add the default function at module level:
   ```rust
   fn default_update_check_interval() -> u32 { 12 }
   ```
   Update BOTH `From` impls:
   - `From<CoreGeneral> for General`: add `update_check_interval: g.update_check_interval,`
   - `From<General> for CoreGeneral`: add `update_check_interval: g.update_check_interval,`

3. In `crates/koji-web/src/pages/config_editor.rs`, add to the page-local `General` struct:
   ```rust
   #[serde(default = "default_update_check_interval")]
   pub update_check_interval: u32,
   ```
   Add the default function at module level:
   ```rust
   fn default_update_check_interval() -> u32 { 12 }
   ```

**Steps:**
- [ ] Add the field and default function to `crates/koji-core/src/config/types.rs`
- [ ] Add the field, default function, and update both `From` impls in `crates/koji-web/src/types/config.rs`
- [ ] Add the field and default function to `crates/koji-web/src/pages/config_editor.rs`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compile errors and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add update_check_interval config option (default 12h, 0=disabled)"

**Acceptance criteria:**
- [ ] Existing configs without `update_check_interval` deserialize with default 12
- [ ] Setting `update_check_interval = 0` disables checks
- [ ] Web mirror type round-trips correctly through JSON
- [ ] All existing tests still pass

---

### Task 3: Update Checker Core Logic

**Context:**
This task creates the core orchestration logic that checks all backends and models for updates and writes results to the `update_checks` DB table. It reuses the existing `koji_core::backends::updater::check_latest_version()` for backends and `koji_core::models::update::check_for_updates()` for models. The key constraint is that `rusqlite::Connection` is `!Send`, so all DB access must happen outside `.await` points — use the pattern: read config synchronously → do async network calls → write to DB synchronously via `tokio::task::spawn_blocking`.

The checker stores config model IDs (e.g. "my-model") as `item_id` in the DB, NOT repo_ids. The repo_id is stored inside `details_json`. This is because the frontend works with config model IDs.

**Files:**
- Create: `crates/koji-core/src/updates/mod.rs`
- Create: `crates/koji-core/src/updates/checker.rs`
- Modify: `crates/koji-core/src/lib.rs`

**What to implement:**

1. `crates/koji-core/src/updates/mod.rs`:
   ```rust
   pub mod checker;
   ```

2. `crates/koji-core/src/lib.rs`: Add `pub mod updates;` to the module list.

3. `crates/koji-core/src/updates/checker.rs`:

   Import types from `crate::backends::updater`, `crate::models::update`, `crate::config::Config`, `crate::db`.

   **`check_all_updates`:**
   ```rust
   pub async fn check_all_updates(
       config_dir: &std::path::Path,
       config: &Config,
       min_age_secs: i64,  // skip items checked within this many seconds
   ) -> Result<Vec<UpdateCheckRecord>>
   ```
   Logic:
   - Get current unix timestamp
   - Read existing check records from DB (via `spawn_blocking`) to know last check times
   - For each backend in `config.backends`:
     - Skip if last checked within `min_age_secs`
     - Look up installed backend info. To do this, open the DB in a `spawn_blocking` block and call `crate::db::queries::get_active_backend(conn, backend_name)`. If no active backend found, skip (not installed).
     - Call `crate::backends::updater::check_latest_version(&backend_type).await`
     - Compare versions to determine if update is available
     - Build `UpdateCheckRecord` with `item_type: "backend"`, `item_id: backend_name`
   - For each model in `config.models` that has a `model` field (repo_id):
     - Skip if last checked within `min_age_secs`
     - Call `crate::models::update::check_for_updates(conn, repo_id).await` — but note: this function takes `&Connection` and handles the `!Send` constraint internally (reads sync, network async, no writes). However, `check_for_updates` needs a Connection at call-time for initial DB reads. So: open connection in `spawn_blocking`, read pull record + file records, drop connection, then do async network checks, then write results in another `spawn_blocking`.
     - Actually, looking at the existing `check_for_updates` signature: `pub async fn check_for_updates(conn: &Connection, repo_id: &str) -> Result<UpdateCheckResult>` — this already handles the `!Send` pattern internally (reads before await, no writes after). BUT the Future itself is `!Send` because it captures `&Connection`. So we CANNOT call it directly from a `tokio::spawn` context. Instead, we need to restructure: do the DB reads in `spawn_blocking`, then do async network calls separately, then do comparison and DB writes in `spawn_blocking`.
     - **Approach for models:** Instead of calling `check_for_updates()` directly, replicate its logic in a `!Send`-safe way:
       1. `spawn_blocking`: open DB, call `get_model_pull(conn, repo_id)` and `get_model_files(conn, repo_id)`, return records
       2. Async: call `pull::list_gguf_files(repo_id).await` and `pull::fetch_blob_metadata(resolved_repo_id).await`
       3. Pure: call `crate::models::update::compare_files(&file_records, &remote_blobs)` (this is a pure function, no I/O)
       4. Determine status from file updates
     - Build `UpdateCheckRecord` with `item_type: "model"`, `item_id: config_model_id`, `details_json` containing serialized JSON with `repo_id` and list of changed filenames
   - After all checks: clean stale entries via `delete_stale_update_checks` for each type
   - Write all results to DB via `spawn_blocking` using `upsert_update_check`
   - Return collected results

   **`check_single_update`:**
   ```rust
   pub async fn check_single_update(
       config_dir: &std::path::Path,
       config: &Config,
       item_type: &str,
       item_id: &str,
       min_age_secs: i64,
   ) -> Result<Option<UpdateCheckRecord>>
   ```
   Same logic as above but for a single item. Returns `None` if the item was checked too recently (within `min_age_secs`).

   **Helper function:**
   ```rust
   fn unix_now() -> i64 {
       std::time::SystemTime::now()
           .duration_since(std::time::UNIX_EPOCH)
           .map(|d| d.as_secs() as i64)
           .unwrap_or(0)
   }
   ```

   **Details JSON for models:** Serialize as:
   ```json
   {
     "repo_id": "bartowski/Qwen3-8B-GGUF",
     "changed_files": ["Qwen3-8B-Q4_K_M.gguf"],
     "display_name": "Qwen3 8B"
   }
   ```

**Steps:**
- [ ] Create `crates/koji-core/src/updates/mod.rs` with `pub mod checker;`
- [ ] Add `pub mod updates;` to `crates/koji-core/src/lib.rs`
- [ ] Create `crates/koji-core/src/updates/checker.rs` with `check_all_updates` and `check_single_update`
- [ ] Add unit tests in `checker.rs` for:
  - `test_unix_now_returns_reasonable_value` — just verify it's > 1700000000
  - Note: full integration tests for check_all_updates would require mocking network calls, so keep tests focused on the pure logic parts. The main testing will happen via the API integration in later tasks.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add update checker core logic (checks backends + models)"

**Acceptance criteria:**
- [ ] `check_all_updates` compiles and handles both backend and model checks
- [ ] Stale entry cleanup happens after each full check
- [ ] `min_age_secs` skipping works correctly
- [ ] `!Send` constraint is respected (no `&Connection` across `.await` points)
- [ ] All existing tests still pass

---

### Task 4: API Endpoints for Updates

**Context:**
The web UI needs API endpoints to read cached update results, trigger manual checks, and apply updates. This task creates the `/api/updates` routes. The GET endpoint goes on the main router (no CSRF needed). All POST endpoints go on the `backend_routes` sub-router (CSRF-protected via same-origin enforcement middleware). The `AppState` needs two new fields: `update_check_lock` (prevents concurrent checks) and a way to access `config_dir` (for DB access).

**Files:**
- Create: `crates/koji-web/src/api/updates.rs`
- Modify: `crates/koji-web/src/api.rs`
- Modify: `crates/koji-web/src/server.rs`

**What to implement:**

1. In `crates/koji-web/src/api.rs`: Add `pub mod updates;`.

2. In `crates/koji-web/src/server.rs`:
   - Add to `AppState`:
     ```rust
     pub update_check_lock: Arc<tokio::sync::Mutex<()>>,
     pub config_dir: Option<std::path::PathBuf>,
     ```
   - Initialize in `run_with_opts`: `update_check_lock: Arc::new(tokio::sync::Mutex::new(()))` and `config_dir` derived from `config_path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf())`.
   - Add routes to the main router (before `.merge(backend_routes)`):
     ```rust
     .route("/api/updates", get(api::updates::list_updates))
     ```
   - Add routes to `backend_routes`:
     ```rust
     .route("/api/updates/check", post(api::updates::trigger_check_all))
     .route("/api/updates/check/:item_type/:item_id", post(api::updates::trigger_check_one))
     .route("/api/updates/apply/backend/:name", post(api::updates::apply_backend_update))
     .route("/api/updates/apply/model/:id", post(api::updates::apply_model_update))
     ```
   - Update the `run_with_opts` function signature — it already accepts `config_path`, so derive `config_dir` from it.
   - Update both `AppState` constructors (in `run_with_opts` and `run`) to include the new fields.

3. In `crates/koji-web/src/api/updates.rs`:

   **`list_updates`** (GET /api/updates):
   - Open DB via `spawn_blocking` using `state.config_dir`
   - Call `get_all_update_checks(conn)`
   - Read `update_check_interval` from `state.proxy_config`
   - Calculate `last_full_check` from `get_last_full_check_time(conn)`
   - Calculate `next_check` = `last_full_check + interval * 3600`
   - Return JSON:
     ```json
     {
       "updates": [...],
       "last_full_check": <unix_ts or null>,
       "next_check": <unix_ts or null>,
       "check_interval_hours": 12
     }
     ```
   - Each update record serialized with all fields from `UpdateCheckRecord`.

   **`trigger_check_all`** (POST /api/updates/check):
   - Try to acquire `state.update_check_lock` with `try_lock()`
   - If lock is already held, return the cached results with `"checking": true`
   - If lock acquired, read config from `proxy_config`, call `check_all_updates(config_dir, &config, 300)` (300s = 5 min min_age)
   - Return fresh results

   **`trigger_check_one`** (POST /api/updates/check/:item_type/:item_id):
   - Read config from `proxy_config`
   - Call `check_single_update(config_dir, &config, item_type, item_id, 60)` (60s = 1 min min_age)
   - Return the single result or 404 if item not found

   **`apply_backend_update`** (POST /api/updates/apply/backend/:name):
   - This should delegate to the existing backend update flow. Look at how `update_backend` handler works in `api/backends.rs` and replicate — it creates a job via `JobManager`, spawns the update task, returns the job ID.
   - Return `{"job_id": "..."}` so the frontend can track progress via existing SSE.

   **`apply_model_update`** (POST /api/updates/apply/model/:id):
   - For now, return a `501 Not Implemented` with `{"error": "Model update apply not yet implemented"}`. The model re-pull flow is complex and should be a follow-up task. The frontend can use the "Edit" link as the primary action.
   - TODO comment explaining this needs the pull infrastructure wired through JobManager.

**Steps:**
- [ ] Add `config_dir` and `update_check_lock` fields to `AppState` in `crates/koji-web/src/server.rs`
- [ ] Update both `AppState` constructors in `run_with_opts` and `run`
- [ ] Add `pub mod updates;` to `crates/koji-web/src/api.rs`
- [ ] Create `crates/koji-web/src/api/updates.rs` with all 5 handler functions
- [ ] Add routes to `build_router` in `server.rs` — GET on main router, POSTs on backend_routes
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compile errors and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add /api/updates endpoints for update checking and applying"

**Acceptance criteria:**
- [ ] `GET /api/updates` returns cached results from DB
- [ ] `POST /api/updates/check` triggers a full check with concurrent request deduplication
- [ ] `POST /api/updates/check/:type/:id` checks a single item with rate limiting
- [ ] `POST /api/updates/apply/backend/:name` delegates to existing backend update job
- [ ] `POST /api/updates/apply/model/:id` returns 501 (placeholder)
- [ ] All routes have correct CSRF protection (POST on backend_routes)
- [ ] All existing tests still pass

---

### Task 5: Background Update Checker Task

**Context:**
This task spawns the periodic background checker as a Tokio task when the web server starts. It reads the `update_check_interval` from config (hot-reloadable), runs an immediate check if stale on startup, then loops with the configured interval. It uses the `update_check_lock` from `AppState` to avoid colliding with manual "Check Now" requests from the API.

**Files:**
- Modify: `crates/koji-web/src/server.rs`

**What to implement:**

Add a new function:
```rust
fn spawn_update_checker(state: Arc<AppState>) {
    tokio::spawn(async move {
        // ... background loop
    });
}
```

Logic inside the spawned task:
1. Wait 10 seconds after startup (let everything initialize)
2. Loop:
   a. Read `update_check_interval` from `state.proxy_config` (if available). If `None` or `0`, sleep 60 seconds and re-check (don't exit the task — interval might change via hot-reload).
   b. Check if last full check is stale: open DB in `spawn_blocking`, call `get_last_full_check_time`, compare with `interval * 3600`.
   c. If stale (or never checked): acquire `update_check_lock`, read config, call `check_all_updates(config_dir, &config, 0)` (no min_age for background checks — check everything). Release lock.
   d. Sleep for `interval * 3600` seconds (use `tokio::time::sleep`).
   e. Re-read interval at the top of each loop iteration.

Call `spawn_update_checker(state.clone())` in `run_with_opts` after building the router, before `axum::serve`. Only call it if `config_path` is `Some` (standalone mode without config doesn't need checking).

Error handling: Catch all errors inside the loop and log them with `tracing::warn!`. Never let the background task panic or exit.

**Steps:**
- [ ] Add `spawn_update_checker` function to `crates/koji-web/src/server.rs`
- [ ] Call it from `run_with_opts` (gated on `config_path.is_some()`)
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: spawn background update checker task on server startup"

**Acceptance criteria:**
- [ ] Background task starts on server startup (when config_path is present)
- [ ] Respects `update_check_interval` from config (hot-reloaded each iteration)
- [ ] Interval of 0 pauses checking (sleeps and re-checks config)
- [ ] Uses `update_check_lock` to avoid concurrent checks
- [ ] Never panics — all errors caught and logged
- [ ] All existing tests still pass

---

### Task 6: Frontend — `/updates` Page & Sidebar Badge

**Context:**
This task creates the Leptos frontend page at `/updates` and adds a sidebar nav item with an update badge. The page shows two sections (Backends and Models), each listing items with available updates. It follows the existing Leptos 0.7 patterns used in the codebase: `leptos::task::spawn_local` for async data fetching, `RwSignal` for reactive state, `gloo_net` for HTTP requests, and the `A` component from `leptos_router` for navigation.

The sidebar currently has nav items for: Dashboard, Models, Backends, Logs (in the main nav) and Config (in the footer). The Updates nav item should go between Backends and Logs in the main nav, using a 🔄 emoji icon.

For the sidebar badge, fetch `GET /api/updates` on mount and count items where `update_available` is true. Show as "(N)" next to "Updates" text when N > 0.

**Files:**
- Create: `crates/koji-web/src/pages/updates.rs`
- Modify: `crates/koji-web/src/pages/mod.rs`
- Modify: `crates/koji-web/src/lib.rs`
- Modify: `crates/koji-web/src/components/sidebar.rs`

**What to implement:**

1. `crates/koji-web/src/pages/updates.rs` — main page component:

   ```rust
   #[component]
   pub fn Updates() -> impl IntoView { ... }
   ```

   **State signals:**
   - `updates: RwSignal<Vec<UpdateEntry>>` — the list of update check results
   - `loading: RwSignal<bool>` — true while fetching/checking
   - `checking: RwSignal<bool>` — true while "Check Now" is in progress
   - `last_full_check: RwSignal<Option<i64>>` — unix timestamp
   - `next_check: RwSignal<Option<i64>>` — unix timestamp
   - `check_interval_hours: RwSignal<u32>`

   **`UpdateEntry` struct** (local to this file, `#[derive(Deserialize)]`):
   ```rust
   struct UpdateEntry {
       item_type: String,
       item_id: String,
       current_version: Option<String>,
       latest_version: Option<String>,
       update_available: bool,
       status: String,
       error_message: Option<String>,
       details_json: Option<String>,
       checked_at: i64,
   }
   ```

   **`UpdatesResponse` struct:**
   ```rust
   struct UpdatesResponse {
       updates: Vec<UpdateEntry>,
       last_full_check: Option<i64>,
       next_check: Option<i64>,
       check_interval_hours: u32,
   }
   ```

   **On mount:** `spawn_local` to fetch `GET /api/updates`, populate signals.

   **"Check Now" handler:** `spawn_local` to POST `/api/updates/check`, set `checking` to true, on response update all signals, set `checking` to false.

   **Layout (view macro):**
   - Page header with class `"page-header"`:
     - `<h1>"Updates"</h1>`
     - Status text showing "Last checked: X ago" (convert unix timestamp to relative time) and "Next check in Y" if interval > 0
     - "Check Now" button (disabled while `checking` is true, shows spinner text "Checking..." when active)
   - If `loading`: show a loading spinner
   - Else if no updates with `update_available == true`: show "Everything is up to date ✓" message
   - Else: two sections:
     - **Backends section** (h2 "Backends"): filter updates where `item_type == "backend"` and `update_available == true`. Each row is a card/div showing:
       - Backend name (item_id)
       - Current version → Latest version
       - "Update" button (POST to `/api/updates/apply/backend/:name`, on success show job ID / redirect to backends page)
     - **Models section** (h2 "Models"): filter updates where `item_type == "model"` and `update_available == true`. Each row:
       - Model name (item_id) and repo_id (from details_json)
       - Changed files count (parse details_json, count changed_files array)
       - "Edit" link → `A` component to `/models/{item_id}/edit`
       - "Update" button (disabled, shows "Coming soon" tooltip — model apply is 501 for now)
   - Below the "updates available" sections, show an "Up to date" section listing items where `status == "up_to_date"` in a compact format
   - Show "Check failed" items with error messages in a warning style

   **Time formatting helper:** Add a function `fn time_ago(unix_ts: i64) -> String` that returns relative time like "2 hours ago", "5 minutes ago", "just now". Use `js_sys::Date::now()` to get current time in the WASM context (divide by 1000 for unix seconds).

2. `crates/koji-web/src/pages/mod.rs`: Add `pub mod updates;`.

3. `crates/koji-web/src/lib.rs`: Add route:
   ```rust
   <Route path=path!("/updates") view=pages::updates::Updates />
   ```
   Place it after the `/backends` route.

4. `crates/koji-web/src/components/sidebar.rs`:
   - Add a signal for update count: `let update_count = RwSignal::new(0u32);`
   - On mount (in the existing `spawn_local` block or a new one), fetch `GET /api/updates` and count items where `update_available` is true. Set `update_count`.
   - Add the Updates nav item between the Backends and Logs items:
     ```rust
     <A href="/updates" attr:class="sidebar-item" attr:data-tooltip="Updates" on:click=move |_| mobile_open.set(false)>
         <span class="sidebar-item__icon">"🔄"</span>
         <span class="sidebar-item__text">
             {move || {
                 let count = update_count.get();
                 if count > 0 {
                     format!("Updates ({})", count)
                 } else {
                     "Updates".to_string()
                 }
             }}
         </span>
     </A>
     ```

**Steps:**
- [ ] Create `crates/koji-web/src/pages/updates.rs` with the `Updates` component
- [ ] Add `pub mod updates;` to `crates/koji-web/src/pages/mod.rs`
- [ ] Add the `/updates` route to `crates/koji-web/src/lib.rs`
- [ ] Add the Updates nav item and badge to `crates/koji-web/src/components/sidebar.rs`
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix compile errors and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add /updates page and sidebar badge for update notifications"

**Acceptance criteria:**
- [ ] `/updates` route renders the Updates page
- [ ] Page loads cached results from `GET /api/updates`
- [ ] "Check Now" button triggers `POST /api/updates/check` and refreshes the list
- [ ] Backend rows show version comparison and functional "Update" button
- [ ] Model rows show "Edit" link and disabled "Update" button (placeholder)
- [ ] Sidebar shows "Updates (N)" badge when updates are available
- [ ] Empty state shows "Everything is up to date ✓"
- [ ] Error states are displayed for failed checks
- [ ] All existing tests still pass
