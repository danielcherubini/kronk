# DB Auto-Backfill on First Open + Process Tracking Plan

**Goal:** (1) Automatically backfill the SQLite database with metadata from all installed models when the DB is first created, and (2) persist running model PIDs/ports in the DB so `kronk status` can report live state and `kronk serve` can clean up orphaned processes on startup.

**Architecture:** The `db::open()` function detects a freshly created DB (migration v1 just ran on an empty DB) and triggers a one-time backfill that scans installed model cards and fetches commit SHAs + LFS hashes from HuggingFace. A new `active_models` table tracks which backend processes are currently running, written by the proxy's `load_model()`/`unload_model()` lifecycle. On `kronk serve` startup, stale entries are detected (PID no longer alive) and cleaned up.

**Key design decisions:**
- Backfill happens inside a new `db::backfill` module called from `db::open()`. Since `db::open()` is sync but backfill needs async network calls, the backfill is a separate async function called from the CLI layer (not from `db::open()` itself). `db::open()` returns a flag indicating whether backfill is needed.
- `active_models` table uses a simple insert-on-load / delete-on-unload pattern. The proxy holds a `Connection` (opened once at startup), passed into `ProxyState`. All DB writes happen synchronously outside `.await` points.
- `kronk status` still queries the `/status` HTTP endpoint when the proxy is running (most accurate), but the DB `active_models` table serves as a fallback and is used for orphan cleanup on startup.

---

## Task 1: Add `active_models` table and backfill detection to DB

**Context:**
The DB currently has three tables (`model_pulls`, `model_files`, `download_log`) from migration v1. We need a fourth table `active_models` to track running backend processes, and a mechanism to detect when the DB was just freshly created so the CLI can trigger a one-time backfill. This task adds migration v2 with the new table, and changes `db::open()` to return whether backfill is needed.

**Files:**
- Modify: `crates/kronk-core/src/db/mod.rs` (change `open()` return type, add `needs_backfill` check)
- Modify: `crates/kronk-core/src/db/migrations.rs` (add v2 migration, update LATEST_VERSION)
- Modify: `crates/kronk-core/src/db/queries.rs` (add active_models query functions)

**What to implement:**

1. **Migration v2** in `migrations.rs`:
   ```sql
   CREATE TABLE IF NOT EXISTS active_models (
       server_name TEXT PRIMARY KEY,   -- config key, e.g. "my-coding-model"
       model_name TEXT NOT NULL,       -- model identifier used for loading
       backend TEXT NOT NULL,          -- backend key, e.g. "llama-server"
       pid INTEGER NOT NULL,           -- backend process PID (i64 in Rust)
       port INTEGER NOT NULL,          -- backend port (i64 in Rust)
       backend_url TEXT NOT NULL,      -- full URL, e.g. "http://127.0.0.1:54321"
       loaded_at TEXT NOT NULL,        -- ISO 8601 timestamp
       last_accessed TEXT NOT NULL     -- ISO 8601 timestamp, updated periodically
   );
   ```
   Update `LATEST_VERSION` to `2`. Add the new migration tuple with explicit version `2`.

2. **`db::open()` return change** in `mod.rs`:
   Change `open()` to return `Result<OpenResult>`:
   ```rust
   pub struct OpenResult {
       pub conn: Connection,
       pub needs_backfill: bool,
   }
   ```
   `needs_backfill` is `true` when the DB was at version 0 before migrations ran (i.e., it was just freshly created). Check `user_version` before running migrations — if it's 0, set a flag, run migrations, then return with `needs_backfill: true`.

   **Update all call sites** that use `db::open()`:
   - `crates/kronk-cli/src/commands/model.rs` — `cmd_pull()`, `cmd_rm()`, `cmd_update()` — these all just need `result.conn`, can ignore `needs_backfill`
   - The backfill itself will be wired in Task 2

3. **Active models query functions** in `queries.rs`:
   ```rust
   /// Insert or replace an active model entry when a backend is loaded.
   pub fn insert_active_model(
       conn: &Connection,
       server_name: &str,
       model_name: &str,
       backend: &str,
       pid: i64,
       port: i64,
       backend_url: &str,
   ) -> Result<()>

   /// Remove an active model entry when a backend is unloaded.
   pub fn remove_active_model(conn: &Connection, server_name: &str) -> Result<()>

   /// Get all active model entries (for status / cleanup).
   pub fn get_active_models(conn: &Connection) -> Result<Vec<ActiveModelRecord>>

   /// Remove all active model entries (for startup cleanup).
   pub fn clear_active_models(conn: &Connection) -> Result<()>

   /// Update last_accessed timestamp for an active model.
   pub fn touch_active_model(conn: &Connection, server_name: &str) -> Result<()>
   ```

   Record struct:
   ```rust
   #[derive(Debug, Clone)]
   pub struct ActiveModelRecord {
       pub server_name: String,
       pub model_name: String,
       pub backend: String,
       pub pid: i64,
       pub port: i64,
       pub backend_url: String,
       pub loaded_at: String,
       pub last_accessed: String,
   }
   ```

**Steps:**
- [ ] Add migration v2 to `migrations.rs`, update `LATEST_VERSION` to `2`
- [ ] Create `OpenResult` struct and update `open()` to return it, checking pre-migration `user_version`
- [ ] Update `open_in_memory()` to also return `OpenResult`
- [ ] Add `ActiveModelRecord` struct and all five query functions to `queries.rs`
- [ ] Update all existing `db::open()` call sites to destructure `OpenResult`
- [ ] Write tests:
  - `test_migration_v2_creates_active_models` — verify `active_models` table exists
  - `test_needs_backfill_true_on_fresh_db` — new in-memory DB returns `needs_backfill: true`
  - `test_needs_backfill_false_on_existing_db` — open twice, second time returns `false`
  - `test_insert_and_get_active_models` — insert, query, verify
  - `test_remove_active_model` — insert, remove, verify gone
  - `test_clear_active_models` — insert several, clear, verify empty
  - `test_touch_active_model` — insert, touch, verify last_accessed changed
- [ ] Run `cargo test --package kronk-core -- db`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat: add active_models table and backfill detection to DB"

**Acceptance criteria:**
- [ ] Migration v2 creates `active_models` table idempotently
- [ ] `db::open()` returns `needs_backfill: true` only on a freshly created DB
- [ ] All CRUD operations for `active_models` work correctly
- [ ] Existing call sites compile and work unchanged (they just ignore `needs_backfill`)
- [ ] All tests pass, clippy clean

---

## Task 2: Auto-backfill DB on first creation

**Context:**
When a user upgrades to the version with the DB and runs any command, `db::open()` creates a fresh DB. We want to detect this and automatically populate `model_pulls` and `model_files` with metadata from all installed models. This requires: (1) scanning installed model cards (sync, local), and (2) fetching commit SHAs and LFS hashes from HuggingFace (async, network). The backfill runs once, on first DB creation, and the user sees progress output so they know what's happening.

Since `db::open()` is synchronous but backfill needs async network calls, the approach is:
- `db::open()` returns `OpenResult { conn, needs_backfill }` (from Task 1)
- A new async function `db::backfill::run_initial_backfill()` is called from the CLI layer when `needs_backfill` is true
- The backfill scans model cards, fetches HF metadata, and writes to DB
- After backfill completes, a marker is set in the DB so it never runs again (we use the presence of any row in `model_pulls` as a signal, but also set a custom pragma or metadata row)

**Files:**
- Create: `crates/kronk-core/src/db/backfill.rs`
- Modify: `crates/kronk-core/src/db/mod.rs` (add `pub mod backfill;`)
- Modify: `crates/kronk-cli/src/commands/model.rs` (trigger backfill when `needs_backfill` is true in `cmd_pull`, `cmd_update`)
- Modify: `crates/kronk-cli/src/handlers/serve.rs` (trigger backfill on `kronk serve` startup)

**What to implement:**

1. **`db/backfill.rs`** — the backfill module:
   ```rust
   use rusqlite::Connection;
   use anyhow::Result;
   use crate::config::Config;
   use crate::models::ModelRegistry;

   /// Run the initial DB backfill for all installed models.
   ///
   /// Scans model cards from the config/models directories, then fetches
   /// commit SHAs and LFS hashes from HuggingFace for each model.
   /// Prints progress to stdout.
   ///
   /// This function is async because it makes network calls to HuggingFace.
   /// The `&Connection` is only used for sync writes after all async work
   /// for each model completes (respecting the !Send constraint).
   pub async fn run_initial_backfill(conn: &Connection, config: &Config) -> Result<()>
   ```

   Implementation:
   1. Scan installed models: `let registry = ModelRegistry::new(...)`, `let models = registry.scan()?;`
   2. If no models, return early (nothing to backfill).
   3. Print `"  Backfilling database for {} installed model(s)..."` with count.
   4. For each model:
      - Print `"  [{}/{}] {}..."` with progress counter and `model.card.model.source`
      - `let repo_id = &model.card.model.source;`
      - ASYNC: Call `pull::list_gguf_files(repo_id).await` — get commit SHA
      - ASYNC: Call `pull::fetch_blob_metadata(repo_id).await` — get LFS hashes
      - SYNC: `upsert_model_pull(conn, repo_id, &listing.commit_sha)?`
      - SYNC: For each quant in `model.card.quants`, `upsert_model_file(conn, ...)` with the LFS hash from blobs map and `size_bytes` from the quant info
      - If network calls fail for a model, log a warning and continue to the next (don't abort the whole backfill). Use `tracing::warn!` and print a user-visible note.
   5. Print `"  Database backfill complete."`

2. **Wire backfill into CLI commands** — anywhere `db::open()` is called and the user has an interactive session:

   In `cmd_pull()` (model.rs), after opening the DB:
   ```rust
   let db_result = kronk_core::db::open(&db_dir)?;
   if db_result.needs_backfill {
       kronk_core::db::backfill::run_initial_backfill(&db_result.conn, config).await?;
   }
   let conn = db_result.conn;
   ```

   Similarly in `cmd_update()`.

   In `start_proxy_server()` (serve.rs):
   ```rust
   let db_dir = kronk_core::config::Config::config_dir()?;
   let db_result = kronk_core::db::open(&db_dir)?;
   if db_result.needs_backfill {
       kronk_core::db::backfill::run_initial_backfill(&db_result.conn, &updated_config).await?;
   }
   ```

   In `cmd_rm()` — this is sync, can't call async backfill. Since `cmd_rm` doesn't need backfill data (it only deletes), just ignore the flag.

**Steps:**
- [ ] Create `crates/kronk-core/src/db/backfill.rs` with `run_initial_backfill()`
- [ ] Add `pub mod backfill;` to `crates/kronk-core/src/db/mod.rs`
- [ ] Wire backfill into `cmd_pull()`, `cmd_update()`, and `start_proxy_server()`
- [ ] Write a unit test `test_backfill_with_no_models` — calls backfill on empty registry, verifies it succeeds without error
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: auto-backfill DB with model metadata on first creation"

**Acceptance criteria:**
- [ ] On first DB creation, all installed models have their commit SHAs and LFS hashes fetched and stored
- [ ] Backfill shows progress to the user (`[1/5] bartowski/OmniCoder-8B-GGUF...`)
- [ ] If a model's HF fetch fails (e.g. private repo, network down), it's skipped with a warning — not fatal
- [ ] Backfill only runs once (second `db::open()` returns `needs_backfill: false`)
- [ ] All tests pass, clippy clean

---

## Task 3: Wire DB into proxy for process tracking

**Context:**
The proxy currently tracks running models in an in-memory `HashMap<String, ModelState>` inside `ProxyState`. When the proxy exits (gracefully or crashes), that state is lost. We want to persist model load/unload events to the `active_models` table so that: (1) `kronk status` can show richer data, (2) on startup, orphaned processes from a previous crash can be detected and killed.

The `ProxyState` needs access to a `Connection`. Since `Connection` is `!Send`, it can't be stored in `ProxyState` (which is `Arc`'d and shared across tokio tasks). Instead, we'll open a fresh connection for each sync DB operation, or store the `db_dir` path in `ProxyState` and open/close on demand. Opening a WAL-mode SQLite DB is very fast (~0.1ms), so this is fine for the load/unload frequency.

**Files:**
- Modify: `crates/kronk-core/src/proxy/types.rs` (add `db_dir: Option<PathBuf>` to `ProxyState`)
- Modify: `crates/kronk-core/src/proxy/lifecycle.rs` (write to DB on `load_model` and `unload_model`)
- Modify: `crates/kronk-core/src/proxy/server/mod.rs` (clean up stale entries on `ProxyServer::new()`)
- Modify: `crates/kronk-cli/src/handlers/serve.rs` (pass `db_dir` when creating `ProxyState`)

**What to implement:**

1. **`ProxyState` changes** in `types.rs`:
   Add a field:
   ```rust
   pub struct ProxyState {
       pub config: crate::config::Config,
       pub models: Arc<tokio::sync::RwLock<HashMap<String, ModelState>>>,
       pub client: reqwest::Client,
       pub metrics: Arc<ProxyMetrics>,
       pub db_dir: Option<std::path::PathBuf>,  // NEW
   }
   ```

   Update `ProxyState::new()` (if it exists in `state.rs`) to accept `db_dir`:
   ```rust
   pub fn new(config: Config, db_dir: Option<PathBuf>) -> Self
   ```

   Add a helper method:
   ```rust
   /// Open a DB connection for a quick sync operation.
   /// Returns None if db_dir is not configured (e.g., in tests).
   fn open_db(&self) -> Option<rusqlite::Connection> {
       self.db_dir.as_ref().and_then(|dir| {
           crate::db::open(dir).ok().map(|r| r.conn)
       })
   }
   ```

2. **`load_model()` changes** in `lifecycle.rs`:
   After the model transitions to `Ready` state (around the block that sets `ModelState::Ready`), add a DB write:
   ```rust
   // Persist to DB (best-effort)
   if let Some(conn) = self.open_db() {
       let _ = crate::db::queries::insert_active_model(
           &conn,
           &server_name,
           model_name,
           &server_config.backend,
           pid as i64,
           port as i64,
           &backend_url,
       );
   }
   ```

3. **`unload_model()` changes** in `lifecycle.rs`:
   After removing the model from the in-memory map, add:
   ```rust
   // Remove from DB (best-effort)
   if let Some(conn) = self.open_db() {
       let _ = crate::db::queries::remove_active_model(&conn, server_name);
   }
   ```

4. **Startup cleanup** in `server/mod.rs`:
   In `ProxyServer::new()`, after creating the idle timeout checker, add:
   ```rust
   // Clean up stale entries from previous runs
   Self::cleanup_stale_processes(&state);
   ```

   Add a new method:
   ```rust
   fn cleanup_stale_processes(state: &ProxyState) {
       if let Some(conn) = state.open_db() {
           if let Ok(active) = crate::db::queries::get_active_models(&conn) {
               for entry in &active {
                   let pid = entry.pid as u32;
                   if !crate::proxy::process::is_process_alive(pid) {
                       tracing::info!("Cleaning up stale process entry: {} (pid {})", entry.server_name, pid);
                       let _ = crate::db::queries::remove_active_model(&conn, &entry.server_name);
                   } else {
                       tracing::warn!(
                           "Orphaned backend process detected: {} (pid {}). Killing.",
                           entry.server_name, pid
                       );
                       // Kill the orphaned process synchronously (block_in_place for the async kill)
                       let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
                       let _ = crate::db::queries::remove_active_model(&conn, &entry.server_name);
                   }
               }
           }
           // Clear any remaining entries to start fresh
           let _ = crate::db::queries::clear_active_models(&conn);
       }
   }
   ```

5. **`serve.rs` changes** — pass `db_dir` to `ProxyState::new()`:
   ```rust
   let db_dir = kronk_core::config::Config::config_dir().ok();
   let state = Arc::new(ProxyState::new(updated_config, db_dir));
   ```

   **Update all other `ProxyState::new()` call sites** (tests, service.rs) to pass `None` for `db_dir` in tests, or `Config::config_dir().ok()` in production.

**Steps:**
- [ ] Add `db_dir: Option<PathBuf>` to `ProxyState`, update constructor and `open_db()` helper
- [ ] Add DB write to `load_model()` after Ready transition
- [ ] Add DB delete to `unload_model()` after map removal
- [ ] Add `cleanup_stale_processes()` to `ProxyServer::new()`
- [ ] Update `serve.rs` to pass `db_dir` when creating `ProxyState`
- [ ] Update all test `ProxyState::new()` calls to pass `None`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: persist running model PIDs in DB for process tracking"

**Acceptance criteria:**
- [ ] When a model is loaded, a row appears in `active_models` with PID, port, URL
- [ ] When a model is unloaded, the row is removed
- [ ] On `kronk serve` startup, stale entries from a crashed previous session are detected and cleaned up
- [ ] Orphaned processes are killed on startup
- [ ] Tests still pass (they use `db_dir: None` so no DB writes)
- [ ] All tests pass, clippy clean

---

## Task 4: Enhance `kronk status` to show PID and loaded state from DB

**Context:**
Currently `kronk status` shows "proxy not running" when the proxy `/status` endpoint is unreachable. With the `active_models` table, we can show richer information even in the fallback path: which models were last loaded, their PIDs, and whether those PIDs are still alive. When the proxy IS running, we can also include PID info from the `/status` endpoint (it already returns `backend_pid`).

**Files:**
- Modify: `crates/kronk-cli/src/handlers/status.rs` (enhance fallback to query DB)

**What to implement:**

Update the fallback path in `cmd_status()` (the `else` branch when proxy is not reachable) to:

1. Open the DB (best-effort):
   ```rust
   let db_active = kronk_core::config::Config::config_dir()
       .ok()
       .and_then(|dir| kronk_core::db::open(&dir).ok())
       .and_then(|r| kronk_core::db::queries::get_active_models(&r.conn).ok())
       .unwrap_or_default();
   ```

2. For each model in config, check if it has an active DB entry:
   ```rust
   let active_entry = db_active.iter().find(|a| a.server_name == *name);
   ```

3. Change the "Loaded" line from `"proxy not running"` to:
   - If there's an active entry and the PID is alive: `"true (pid: 12345, port: 54321) — proxy not responding"`
   - If there's an active entry but PID is dead: `"false (stale — pid 12345 no longer running)"`
   - If no active entry: `"false"`

4. When the proxy IS running (the `if let Some(ref proxy_json)` path), also show the PID in the loaded line:
   - Currently shows: `"true (idle: 5s ago, unloads in 4m55s)"`
   - Change to: `"true (pid: 12345, idle: 5s ago, unloads in 4m55s)"`
   - The `backend_pid` is already in the `/status` JSON response.

**Steps:**
- [ ] Update the proxy-running path to include PID in the loaded line
- [ ] Update the fallback path to query `active_models` from DB
- [ ] Show PID liveness status in the fallback path
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: enhance kronk status with PID and DB-backed loaded state"

**Acceptance criteria:**
- [ ] `kronk status` shows PID when the proxy is running and a model is loaded
- [ ] `kronk status` shows last-known state from DB when the proxy is not responding
- [ ] Stale PIDs (process dead) are correctly identified and shown as stale
- [ ] When no DB or no active entries exist, shows `"false"` (clean fallback)
- [ ] All tests pass, clippy clean
