# Model Manager Centralization Plan

**Goal:** Centralize all model-related DB access into a single `ModelManager` struct, replacing 29+ scattered `db::open()` calls and direct `db::queries::` usage across web, CLI, and proxy.

**Architecture:** `ModelManager` struct in `crates/tama-core/src/models/manager.rs` owns a single `rusqlite::Connection` and provides methods for config CRUD, file tracking, pull tracking, active models, download queue, and update checks. Callers in web API, CLI commands, and proxy lifecycle switch to using `ModelManager` instead of opening their own connections.

**Tech Stack:** Rust, rusqlite, existing `db::queries::*` functions

---

### Task 1: Create ModelManager struct + core methods + config CRUD

**Context:** Establish the foundation — the struct, connection management, and config CRUD operations. This mirrors Task 1 of the BackendManager centralization. The `conn()` getter is a temporary escape hatch for callers that need raw access during incremental migration.

**Files:**
- Create: `crates/tama-core/src/models/manager.rs`
- Modify: `crates/tama-core/src/models/mod.rs`

**What to implement:**

Create `ModelManager` struct:
```rust
pub struct ModelManager {
    conn: rusqlite::Connection,
}
```

Core methods:
- `pub fn open(config_dir: &Path) -> Result<Self>` — calls `crate::db::open(config_dir)`, extracts `conn` from `OpenResult`
- `pub fn open_in_memory() -> Result<Self>` — calls `crate::db::open_in_memory()`, extracts `conn`
- `pub fn conn(&self) -> &rusqlite::Connection` — returns reference to connection (permanent — needed for async functions that must not hold `&Connection` across `.await`, and for transactional operations)
- `pub fn transaction<F, T>(&self, f: F) -> Result<T> where F: FnOnce(&rusqlite::Transaction) -> Result<T>` — wraps `self.conn.transaction()` for atomic multi-step operations (used by web API delete)

Config CRUD methods (all delegate to existing `crate::db::queries::*` functions):
- `pub fn get_config(&self, id: i64) -> Result<Option<ModelConfigRecord>>`
- `pub fn get_config_by_repo_id(&self, repo_id: &str) -> Result<Option<ModelConfigRecord>>`
- `pub fn get_all_configs(&self) -> Result<Vec<ModelConfigRecord>>`
- `pub fn upsert_config(&self, record: &ModelConfigRecord) -> Result<()>`
- `pub fn delete_config(&self, id: i64) -> Result<()>`
- `pub fn rename_config(&self, id: i64, new_repo_id: &str) -> Result<()>`
- `pub fn enable_model(&self, id: i64) -> Result<()>`
- `pub fn disable_model(&self, id: i64) -> Result<()>`

Convenience method (mirrors `db::save_model_config`):
- `pub fn save_model_config(&self, config_key: &str, mc: &ModelConfig) -> Result<i64>` — converts config_key to repo_id, converts `ModelConfig` → `ModelConfigRecord`, sets `api_name` default, calls `upsert_config`. This avoids callers duplicating 10+ lines of record construction.

Use `use crate::db::queries::{ModelConfigRecord, ...}` to import types. All methods pass `&self.conn` to the underlying query functions.

Update `models/mod.rs` to expose `ModelManager`. Do NOT re-export `ModelConfigRecord` — callers import it from `crate::db::queries` (consistent with `BackendManager` pattern).

**Steps:**
- [ ] Write unit tests in `manager.rs` `#[cfg(test)]` module:
  - `test_open_in_memory` — verify valid connection, `get_all_configs()` returns empty vec
  - `test_upsert_and_get_config` — create config record, upsert it, verify it appears in `get_config()` and `get_all_configs()`
  - `test_get_config_by_repo_id_missing` — verify `None` returned for non-existent repo_id
  - `test_enable_disable_model` — enable a model, verify it's enabled, disable it, verify it's disabled
  - `test_rename_config` — rename a config, verify old repo_id returns None, new repo_id returns the record
  - `test_save_model_config_convenience` — verify the convenience method converts `ModelConfig` → `ModelConfigRecord` correctly
- [ ] Run `cargo test --package tama-core models::manager::tests -- --nocapture`
- [ ] Implement `ModelManager` struct + core methods + config CRUD in `models/manager.rs`
- [ ] Update `models/mod.rs` to include `mod manager` and re-export `ModelManager`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add ModelManager struct with config CRUD methods"

**Acceptance criteria:**
- [ ] `ModelManager::open_in_memory()` creates valid connection
- [ ] All config CRUD methods compile and delegate to `db::queries::*`
- [ ] `cargo build --package tama-core` succeeds
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: Add files + pull tracking + verification methods

**Context:** Add file tracking, pull history, and verification methods to `ModelManager`. These cover the model lifecycle — what files exist, what was pulled, and verification status.

**Files:**
- Modify: `crates/tama-core/src/models/manager.rs`

**What to implement:**

File methods:
- `pub fn get_files(&self, model_id: i64) -> Result<Vec<ModelFileRecord>>`
- `pub fn get_all_files(&self) -> Result<Vec<ModelFileRecord>>`
- `pub fn upsert_file(&self, record: &ModelFileRecord) -> Result<()>`
- `pub fn delete_file(&self, model_id: i64, filename: &str) -> Result<()>`
- `pub fn update_verification(&self, model_id: i64, filename: &str, verified: bool, hash: &str) -> Result<()>`

Pull tracking methods:
- `pub fn upsert_pull(&self, model_id: i64, repo_id: &str, commit_sha: &str) -> Result<()>`
- `pub fn get_pull(&self, model_id: i64) -> Result<Option<ModelPullRecord>>`
- `pub fn log_download(&self, entry: &DownloadLogEntry) -> Result<()>`

Import types: `ModelFileRecord`, `ModelPullRecord`, `DownloadLogEntry` from `crate::db::queries`.

**Steps:**
- [ ] Write unit test for `ModelManager::upsert_file()` + `get_files()` in `manager.rs` `#[cfg(test)]` module
  - Create file record, upsert it, verify it appears in `get_files()`
- [ ] Run `cargo test --package tama-core models::manager::tests::test_file_operations -- --nocapture`
- [ ] Implement file methods in `models/manager.rs`
- [ ] Implement pull tracking methods in `models/manager.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add file tracking and pull history methods to ModelManager"

**Acceptance criteria:**
- [ ] File CRUD methods compile and delegate to `db::queries::*`
- [ ] Pull tracking methods compile and delegate to `db::queries::*`
- [ ] `cargo build --package tama-core` succeeds
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 3: Add active models + download queue + update checks

**Context:** Complete the `ModelManager` API with active model tracking, download queue operations, and update check caching. These are the runtime-facing methods used by the proxy lifecycle and download queue.

**Files:**
- Modify: `crates/tama-core/src/models/manager.rs`

**What to implement:**

Active model methods:
- `pub fn insert_active(&self, record: &ActiveModelRecord) -> Result<()>`
- `pub fn remove_active(&self, server_name: &str) -> Result<()>`
- `pub fn get_active(&self) -> Result<Vec<ActiveModelRecord>>`
- `pub fn rename_active(&self, old_name: &str, new_name: &str) -> Result<()>`

Download queue methods (1:1 mapping to `download_queue_queries.rs` — do NOT invent new signatures):
- `pub fn queue_insert(&self, job_id: &str, repo_id: &str, filename: &str, ...) -> Result<()>` — mirrors `insert_queue_item(conn, ...)` exactly, just replaces `conn` with `&self`
- `pub fn queue_get_queued(&self) -> Result<Option<QueueItem>>` — mirrors `get_queued_item(conn)`
- `pub fn queue_get_active(&self) -> Result<Vec<QueueItem>>` — mirrors `get_active_items(conn)`
- `pub fn queue_get_history(&self, limit: i64, offset: i64) -> Result<Vec<QueueItem>>` — mirrors `get_history_items(conn, ...)`
- `pub fn queue_update_status(&self, job_id: &str, new_status: &str, bytes_downloaded: i64, total_bytes: i64, error_message: Option<&str>) -> Result<()>` — mirrors `update_queue_status(conn, ...)` exactly
- `pub fn queue_cancel(&self, job_id: &str) -> Result<()>` — mirrors `cancel_queue_item(conn, ...)` (sets status, does NOT delete row)
- `pub fn queue_get_by_job_id(&self, job_id: &str) -> Result<Option<QueueItem>>` — mirrors `get_item_by_job_id(conn, ...)`

Update check methods:
- `pub fn get_update_check(&self, entity_type: &str, entity_id: &str) -> Result<Option<UpdateCheckRecord>>`
- `pub fn upsert_update_check(&self, record: &UpdateCheckRecord) -> Result<()>`
- `pub fn delete_update_check(&self, entity_type: &str, entity_id: &str) -> Result<()>`

Import types: `ActiveModelRecord`, `QueueItem` (or whatever the actual type is in `download_queue_queries.rs`), `UpdateCheckRecord` from `crate::db::queries`.

**Important:** Read `download_queue_queries.rs` first to get exact function signatures. The manager methods must be 1:1 wrappers — same parameter lists, same return types, just replacing `conn: &Connection` with `&self`. Do NOT invent clean signatures that don't match reality.

**Steps:**
- [ ] Write unit test for `ModelManager::insert_active()` + `get_active()` in `manager.rs` `#[cfg(test)]` module
  - Insert active record, verify it appears in `get_active()`, remove it, verify it's gone
- [ ] Run `cargo test --package tama-core models::manager::tests::test_active_model_operations -- --nocapture`
- [ ] Implement active model methods in `models/manager.rs`
- [ ] Implement download queue methods in `models/manager.rs`
- [ ] Implement update check methods in `models/manager.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add active models, download queue, and update check methods to ModelManager"

**Acceptance criteria:**
- [ ] All methods compile and delegate to `db::queries::*`
- [ ] `cargo build --package tama-core` succeeds
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 4: Switch web API callers to ModelManager

**Context:** Migrate all web API endpoints that currently call `db::open()` + `db::queries::*` to use `ModelManager`. This eliminates ~15 `db::open()` calls in the web crate.

**Files:**
- Modify: `crates/tama-web/src/api/models/crud/create.rs`
- Modify: `crates/tama-web/src/api/models/crud/delete.rs`
- Modify: `crates/tama-web/src/api/models/crud/rename.rs`
- Modify: `crates/tama-web/src/api/models/crud/update.rs`
- Modify: `crates/tama-web/src/api/models/info.rs`
- Modify: `crates/tama-web/src/api/models/files.rs`

**What to implement:**

For each file:
1. Replace `tama_core::db::open(&config_dir)` with `ModelManager::open(&config_dir)` (or use `ModelManager::open_from()` if explicit dir needed)
2. Replace `db::queries::*` calls with corresponding `ModelManager` methods
3. Replace `tama_core::db::save_model_config(...)` with `mgr.save_model_config(...)` (the convenience method)
4. For `delete.rs`: the file uses `conn.transaction()?` for atomic multi-delete. Replace with `mgr.transaction(|tx| { ... })` — the transaction closure takes `&rusqlite::Transaction` and runs multiple delete operations atomically
5. Remove `rusqlite::Connection` imports if no longer needed
6. Handle errors via `anyhow::Context` where appropriate

Example transformation:
```rust
// Before
let open = tama_core::db::open(&config_dir).map_err(|e| { /* ... */ })?;
let record = tama_core::db::queries::get_model_config(&open.conn, id)?;

// After
let mgr = tama_core::models::ModelManager::open(&config_dir).map_err(|e| { /* ... */ })?;
let record = mgr.get_config(id)?;
```

For `info.rs`, the function `build_model_info` takes `conn: &rusqlite::Connection` — update it to take `&ModelManager` instead. Update all callers.

For `files.rs`, multiple functions open DB connections — consolidate to use `ModelManager`.

**Steps:**
- [ ] Switch `create.rs` to use `ModelManager` — replace `db::open()` + `get_model_config_by_repo_id`
- [ ] Switch `delete.rs` to use `ModelManager` — replace `db::open()` + `get_model_config` + `delete_*`
- [ ] Switch `rename.rs` to use `ModelManager` — replace `db::open()` + `get_model_config` + `delete_update_check`
- [ ] Switch `update.rs` to use `ModelManager` — replace `db::open()` + `get_model_config`
- [ ] Switch `info.rs` to use `ModelManager` — update `build_model_info` signature to take `&ModelManager`
- [ ] Switch `files.rs` to use `ModelManager` — replace all `db::open()` calls
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-web`
- [ ] Commit with message: "refactor: switch web API model endpoints to ModelManager"

**Acceptance criteria:**
- [ ] No `db::open()` calls remain in `crates/tama-web/src/api/models/`
- [ ] No direct `db::queries::` calls remain in `crates/tama-web/src/api/models/`
- [ ] `cargo build --package tama-web` succeeds
- [ ] `cargo clippy --package tama-web -- -D warnings` passes

---

### Task 5: Switch CLI callers to ModelManager

**Context:** Migrate all CLI model commands that currently call `db::open()` + `db::queries::*` to use `ModelManager`. This eliminates ~12 `db::open()` calls in the CLI crate.

**Files:**
- Modify: `crates/tama-cli/src/commands/model/enable_disable.rs`
- Modify: `crates/tama-cli/src/commands/model/list_rm.rs`
- Modify: `crates/tama-cli/src/commands/model/prune.rs`
- Modify: `crates/tama-cli/src/commands/model/pull.rs`
- Modify: `crates/tama-cli/src/commands/model/update.rs`
- Modify: `crates/tama-cli/src/commands/model/verify.rs`
- Modify: `crates/tama-cli/src/commands/model/create.rs`
- Modify: `crates/tama-cli/src/commands/model/migrate.rs`

**What to implement:**

For each file:
1. Replace `tama_core::db::open(&db_dir)` with `ModelManager::open(&db_dir)`
2. Replace `db::queries::*` calls with corresponding `ModelManager` methods
3. Remove `OpenResult` destructuring — use `ModelManager` directly
4. For `verify.rs`, the function `verify_model` takes `conn: &Connection` — update to take `&ModelManager`

Example transformation:
```rust
// Before
let OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;
let config = tama_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)?;

// After
let mgr = tama_core::models::ModelManager::open(&db_dir)?;
let config = mgr.get_config_by_repo_id(repo_id)?;
```

For `pull.rs`, this is the most complex file — it has inline `ModelConfigRecord` construction, `DownloadLogEntry` creation, and multiple query calls. Replace all with `ModelManager` methods.

**Steps:**
- [ ] Switch `enable_disable.rs` to use `ModelManager`
- [ ] Switch `list_rm.rs` to use `ModelManager`
- [ ] Switch `prune.rs` to use `ModelManager`
- [ ] Switch `create.rs` to use `ModelManager`
- [ ] Switch `update.rs` to use `ModelManager`
- [ ] Switch `verify.rs` to use `ModelManager` — update `verify_model` signature
- [ ] Switch `pull.rs` to use `ModelManager` — most complex, handle inline record construction
- [ ] Switch `migrate.rs` to use `ModelManager`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-cli`
- [ ] Commit with message: "refactor: switch CLI model commands to ModelManager"

**Acceptance criteria:**
- [ ] No `db::open()` calls remain in `crates/tama-cli/src/commands/model/`
- [ ] No direct `db::queries::` calls remain in `crates/tama-cli/src/commands/model/`
- [ ] `cargo build --package tama-cli` succeeds
- [ ] `cargo clippy --package tama-cli -- -D warnings` passes

---

### Task 6: Switch proxy callers to ModelManager

**Context:** Migrate proxy lifecycle, server, and download queue code to use `ModelManager`. This is the trickiest part because `ProxyState` currently holds its own DB connection and spawns child processes.

**Files:**
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs`
- Modify: `crates/tama-core/src/proxy/server/mod.rs`
- Modify: `crates/tama-core/src/proxy/download_queue.rs`
- Modify: `crates/tama-core/src/proxy/forward.rs`
- Modify: `crates/tama-core/src/proxy/rename.rs`
- Modify: `crates/tama-core/src/proxy/tama_handlers/pull/download.rs`

**What to implement:**

1. **`ProxyState`**: Add `model_mgr: Option<ModelManager>` field alongside existing `backend_mgr`. Initialize it in `ProxyState::new()` using `ModelManager::open()`.

2. **`lifecycle/mod.rs`**: Replace `db::queries::insert_active_model` / `remove_active_model` with `model_mgr.insert_active()` / `model_mgr.remove_active()`.

3. **`server/mod.rs`**: Replace `db::queries::get_active_models` / `remove_active_model` with `model_mgr.get_active()` / `model_mgr.remove_active()`.

4. **`download_queue.rs`**: `DownloadQueueService` is a separate struct from `ProxyState` — it does NOT have access to `ProxyState.model_mgr`. Instead, have `DownloadQueueService` hold its own `ModelManager` (since `ModelManager::open()` creates a new connection each time, like `DownloadQueueService::open_conn()` currently does). Replace `self.open_conn()` + `db::queries::*` queue calls with `self.model_mgr.queue_*()` methods.

5. **`forward.rs`**: Replace `db::queries::remove_active_model` with `model_mgr.remove_active()`.

6. **`rename.rs`**: Replace `db::queries::get_model_config_by_repo_id` / `delete_model_config` with `model_mgr.get_config_by_repo_id()` / `model_mgr.delete_config()`.

7. **`tama_handlers/pull/download.rs`**: Replace `db::queries::upsert_model_file` / `update_verification` with `model_mgr.upsert_file()` / `model_mgr.update_verification()`.

**Pattern:** Same as BackendManager migration — `ProxyState` owns the `ModelManager`, callers access via `state.model_mgr.as_ref().unwrap()`. `DownloadQueueService` holds its own `ModelManager` (not shared with `ProxyState`).

**Steps:**
- [ ] Add `model_mgr: Option<ModelManager>` to `ProxyState` struct
- [ ] Initialize `model_mgr` in `ProxyState::new()` using `ModelManager::open()`
- [ ] Switch `lifecycle/mod.rs` to use `model_mgr` for active model operations
- [ ] Switch `server/mod.rs` to use `model_mgr` for active model + metrics operations
- [ ] Switch `download_queue.rs` to use `model_mgr` for queue operations
- [ ] Switch `forward.rs` to use `model_mgr` for active model operations
- [ ] Switch `rename.rs` to use `model_mgr` for config operations
- [ ] Switch `tama_handlers/pull/download.rs` to use `model_mgr` for file operations
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "refactor: switch proxy callers to ModelManager"

**Acceptance criteria:**
- [ ] `ProxyState` holds `ModelManager` alongside `BackendManager`
- [ ] No `db::queries::` calls remain in proxy code for model operations
- [ ] `cargo build --package tama-core` succeeds
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 7: Cleanup — remove conn() getter, delete old patterns, verify

**Context:** Final cleanup — remove the temporary `conn()` escape hatch from `ModelManager`, delete any remaining `db::open()` calls that should have been migrated, and run full verification.

**Files:**
- Modify: `crates/tama-core/src/models/manager.rs`
- Modify: Any files still using `ModelManager::conn()` for direct DB access

**What to implement:**

1. **Keep `conn()` as a permanent method** — do NOT remove it. It's needed for:
   - Async functions in `models/update.rs` (`check_for_updates`, `refresh_metadata`) that must not hold `&Connection` across `.await`
   - Transactional operations in web API (`delete.rs` uses `mgr.transaction()`) that need raw `&rusqlite::Transaction`
   - Any future callers that need direct DB access

2. **Add async wrapper methods to `ModelManager`** for the update functions:
   - `pub async fn check_for_updates(&self, repo_id: &str) -> Result<UpdateCheckResult>` — calls `check_for_updates(&self.conn, repo_id)` internally
   - `pub async fn refresh_metadata(&self, models_dir: &Path, repo_id: &str) -> Result<()>` — calls `refresh_metadata(&self.conn, models_dir, repo_id)` internally
   These methods are `!Send` because `Connection: !Send` — this is already documented in `update.rs`.

3. **Search for any remaining `db::open()` calls** in `crates/tama-web/src/api/models/` and `crates/tama-cli/src/commands/model/` — migrate any stragglers

4. **Check `models/verify.rs`** — the `verify_model` function takes `&Connection`. Update it to take `&ModelManager` and use `mgr.get_files()` / `mgr.update_verification()` internally.

5. **Run full test suite** including workspace-level integration tests to verify migrated callers work correctly

**Steps:**
- [ ] Add async wrapper methods `check_for_updates()` and `refresh_metadata()` to `ModelManager`
- [ ] Grep for `db::open()` in web/models and cli/model — migrate any stragglers
- [ ] Update `models/verify.rs` to take `&ModelManager` instead of `&Connection`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Smoke test: verify CLI `tama model list` and `tama model verify` commands work with `ModelManager`
- [ ] Commit with message: "refactor: finalize ModelManager — add async wrappers, cleanup stragglers"

**Acceptance criteria:**
- [ ] `ModelManager::conn()` method kept (permanent, not removed)
- [ ] Async wrapper methods `check_for_updates()` and `refresh_metadata()` added to `ModelManager`
- [ ] No `db::open()` calls in web/models or cli/model directories
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes (zero warnings)
- [ ] `cargo test --workspace` passes (all 942+ tests)
- [ ] CLI smoke tests pass (`tama model list`, `tama model verify`)

---

## Summary

| Task | Scope | Files |
|------|-------|-------|
| 1 | Create struct + config CRUD + transaction support | `models/manager.rs`, `models/mod.rs` |
| 2 | Files + pull tracking | `models/manager.rs` |
| 3 | Active models + queue + updates (1:1 queue signatures) | `models/manager.rs` |
| 4 | Switch web API callers (with transaction handling) | 6 files in `tama-web/src/api/models/` |
| 5 | Switch CLI callers | 8 files in `tama-cli/src/commands/model/` |
| 6 | Switch proxy callers (DownloadQueueService gets own ModelManager) | 7 files in `tama-core/src/proxy/` |
| 7 | Add async wrappers, cleanup stragglers, verify | `models/manager.rs` + stragglers |

**Total:** ~7 tasks, 20+ files modified, 29+ `db::open()` calls eliminated.
