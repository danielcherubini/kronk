# Backend Config to Database Plan

**Goal:** Move backend configuration (`default_args`, `health_check_url`) from `config.toml` into SQLite, keyed by `(name, gpu_variant)` with a unique DB id. Eliminate the TOML `[backends]` section entirely.

**Architecture:** Create a `backend_configs` table with `(name, gpu_variant)` as the unique key. Each row stores `default_args` (JSON text) and `health_check_url`. Migrate existing TOML data on first run. The API reads/writes the DB directly. The frontend already keys edits by `"name:variant"`, so no frontend structural changes needed beyond pointing at the new API.

**Tech Stack:** Rust, SQLite (rusqlite), Axum API, Leptos frontend

---

### Task 1: Create backend_configs table + migration

**Context:**
Backend installation records already live in SQLite (`backend_installations` table). Config settings (`default_args`, `health_check_url`) are the odd ones out still stored in `config.toml` under `[backends.<name>]`. This task creates a dedicated config table and migrates existing TOML data.

**Files:**
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/db/queries/backend_queries.rs`
- Modify: `crates/tama-core/src/db/backfill.rs`
- Test: `crates/tama-core/src/db/migrations.rs` (existing migration tests)

**What to implement:**

1. Add migration v22 in `migrations.rs`:
   ```sql
   CREATE TABLE backend_configs (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       name TEXT NOT NULL,
       gpu_variant TEXT NOT NULL DEFAULT 'cpu',
       default_args TEXT,              -- JSON array, e.g. '["--threads","8"]'
       health_check_url TEXT,
       UNIQUE(name, gpu_variant)
   );
   CREATE INDEX idx_backend_configs_name_variant ON backend_configs(name, gpu_variant);
   ```

2. Add queries in `backend_queries.rs`:
   ```rust
   pub struct BackendConfigRecord {
       pub id: i64,
       pub name: String,
       pub gpu_variant: String,
       pub default_args: Vec<String>,  // parsed from JSON
       pub health_check_url: Option<String>,
   }

   pub fn get_backend_config(conn: &Connection, name: &str, gpu_variant: &str) -> Result<Option<BackendConfigRecord>>
   pub fn upsert_backend_config(conn: &Connection, name: &str, gpu_variant: &str, default_args: &[String], health_check_url: Option<&str>) -> Result<i64>
   pub fn list_backend_configs(conn: &Connection) -> Result<Vec<BackendConfigRecord>>
   ```
   - `get_backend_config`: `SELECT * FROM backend_configs WHERE name=? AND gpu_variant=?`
   - `upsert_backend_config`: `INSERT OR REPLACE INTO backend_configs (name, gpu_variant, default_args, health_check_url) VALUES (?, ?, ?, ?)` — returns the row's id
   - `list_backend_configs`: `SELECT * FROM backend_configs`

3. Add migration logic in `backfill.rs`:
   ```rust
   pub fn migrate_backend_config_from_toml(conn: &Connection, config_dir: &Path) -> Result<usize>
   ```
   - Load `config.toml`, iterate `config.backends`
   - For each entry, determine gpu_variant: if `backend_config.gpu_variant` is Some, use it; otherwise insert with 'cpu'
   - Call `upsert_backend_config` for each
   - After successful migration, remove the `[backends]` section from `config.toml` (or comment it out with a migration note)

**Steps:**
- [ ] Add migration v22 in `crates/tama-core/src/db/migrations.rs` — create `backend_configs` table with schema above
- [ ] Add `BackendConfigRecord` struct and three query functions in `crates/tama-core/src/db/queries/backend_queries.rs`
- [ ] Implement `get_backend_config` using `conn.query_row` with JSON deserialization of `default_args`
- [ ] Implement `upsert_backend_config` using `INSERT OR REPLACE`, serialize `default_args` to JSON, return `last_insert_rowid()`
- [ ] Implement `list_backend_configs` using `conn.query` with `query_map`
- [ ] Add `migrate_backend_config_from_toml` in `crates/tama-core/src/db/backfill.rs` — reads TOML backends section, inserts into DB
- [ ] Call `migrate_backend_config_from_toml` from the startup backfill code (same pattern as `migrate_backend_registry_toml`)
- [ ] Write a migration test: insert rows into backend_configs, verify queries return correct data
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: add backend_configs table with migration v22"

**Acceptance criteria:**
- [ ] Migration v22 creates `backend_configs` table with correct schema
- [ ] `get_backend_config` returns parsed `BackendConfigRecord` for existing rows
- [ ] `upsert_backend_config` creates or updates rows, returns the row id
- [ ] `migrate_backend_config_from_toml` transfers TOML data to DB
- [ ] All existing tests pass

---

### Task 2: Update API — read/write default_args from DB

**Context:**
The API endpoints for listing backends and updating default_args currently read/write `config.toml`. They need to switch to the `backend_configs` DB table.

**Files:**
- Modify: `crates/tama-web/src/api/backends/manage.rs`
- Modify: `crates/tama-web/src/api/backends/list.rs`

**What to implement:**

1. In `update_backend_default_args` (manage.rs):
   - Replace the TOML load/save logic with a DB call
   - Parse `gpu_variant` from query param (required now, not optional)
   - Call `upsert_backend_config(conn, &backend_name, &gpu_variant, &default_args, None)`
   - No more touching `config.toml`

   ```rust
   // New handler logic:
   let gpu_variant = query.gpu_variant.ok_or_else(|| {
       (StatusCode::BAD_REQUEST, Json(json!({"error": "gpu_variant is required"})))
   })?;

   let config_dir = /* ... */;
   let open_result = tama_core::db::open(&config_dir)?;
   tama_core::db::queries::upsert_backend_config(
       &open_result.conn,
       &backend_name,
       &gpu_variant,
       &req.default_args,
       None,
   )?;
   ```

2. In `list_backends` (list.rs):
   - Replace the `default_args_map` built from TOML with a DB query
   - Build a map keyed by `"(name, gpu_variant)"` from `list_backend_configs`
   - When building each card, look up `(type_, variant)` in the DB map

   ```rust
   // Replace TOML-based default_args_map with DB query:
   let config_dir = /* ... */;
   let backend_configs_map: std::collections::HashMap<(String, String), Vec<String>> =
       tama_core::db::open(&config_dir)
           .ok()
           .map(|open| {
               tama_core::db::queries::list_backend_configs(&open.conn).ok()
                   .map(|configs| {
                       configs.into_iter().map(|c| {
                           ((c.name, c.gpu_variant), c.default_args)
                       }).collect()
                   })
                   .unwrap_or_default()
           })
           .unwrap_or_default();

   // When building each card:
   let default_args = backend_configs_map
       .get(&(type_.to_string(), variant.clone()))
       .cloned()
       .unwrap_or_default();
   ```

3. Also update `check_backend_updates` (list.rs, ~line 332) — same pattern as `list_backends`

4. **Remove** the TOML-based `default_args_map` construction from both functions

**Steps:**
- [ ] In `update_backend_default_args` (manage.rs), replace TOML load/save with `upsert_backend_config` DB call
- [ ] Make `gpu_variant` query param required (return 400 if missing)
- [ ] Remove the explicit `BackendConfig` struct construction (no longer needed)
- [ ] In `list_backends` (list.rs), replace TOML `default_args_map` with `list_backend_configs` DB query
- [ ] Build `backend_configs_map` keyed by `(name, gpu_variant)` tuple
- [ ] Look up default_args per card using `(type_, variant)` key
- [ ] Apply the same changes to `check_backend_updates` function
- [ ] Remove the `update_backend_default_args` config sync logic (the `proxy_config` write)
- [ ] Run `cargo check --package tama-web --features ssr`
- [ ] Run `cargo test --package tama-web --features ssr`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web --features ssr -- -D warnings`
- [ ] Commit with message: "feat: read/write backend default_args from DB"

**Acceptance criteria:**
- [ ] POST `/tama/v1/backends/:name/default-args?gpu_variant=vulkan` saves to `backend_configs` DB table
- [ ] POST without `gpu_variant` returns 400
- [ ] GET `/tama/v1/backends` returns per-variant default_args from DB
- [ ] `check_backend_updates` also returns per-variant default_args from DB
- [ ] No more TOML reads/writes for backend default_args in these handlers

---

### Task 3: Update config resolution — read default_args from DB

**Context:**
When the proxy starts a backend, it builds the full args by merging `backend.default_args` with server args. Currently this reads from the TOML `BackendConfig`. After this task, it reads from the `backend_configs` DB table.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`
- Modify: `crates/tama-core/src/config/mod.rs` (if config loading needs changes)

**What to implement:**

In `build_args` and `build_full_args` (resolve/mod.rs), replace:
```rust
let mut grouped = crate::config::merge_args(&backend.default_args, &server.args);
```

With DB lookup:
```rust
// Look up default_args from backend_configs table
let db_args = db_conn.as_ref().and_then(|conn| {
    crate::db::queries::get_backend_config(conn, &backend_name, &gpu_variant)
        .ok()
        .flatten()
        .map(|c| c.default_args)
}).unwrap_or_default();
let mut grouped = crate::config::merge_args(&db_args, &server.args);
```

The `build_args` function signature needs a `db_conn` parameter or the caller needs to pass it. Look at how `resolve_backend_path` already accepts a `&Connection` — follow the same pattern.

**Steps:**
- [ ] Update `build_args` signature to accept `db_conn: Option<&Connection>` parameter (same pattern as `resolve_backend_path`)
- [ ] Update `build_full_args` similarly
- [ ] Replace `&backend.default_args` with DB lookup: `get_backend_config(conn, name, variant).default_args`
- [ ] Fall back to empty vec if no DB config exists (backward compat for backends not yet migrated)
- [ ] Update all call sites of `build_args` and `build_full_args` to pass the DB connection
- [ ] Run `cargo check --package tama-core`
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: resolve backend default_args from DB"

**Acceptance criteria:**
- [ ] `build_args` and `build_full_args` read default_args from `backend_configs` table
- [ ] Falls back to empty vec if no DB config exists
- [ ] All call sites pass the DB connection
- [ ] All existing tests pass

---

### Task 4: Remove BackendConfig from TOML + cleanup

**Context:**
After Tasks 1-3, the TOML `[backends]` section is no longer needed for runtime config. The `BackendConfig` struct can be simplified or removed. The `default_args` and `health_check_url` fields are now in the DB.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`
- Modify: Any files that reference `BackendConfig.default_args` or `BackendConfig.health_check_url`

**What to implement:**

1. In `BackendConfig` (types.rs), keep only fields that make sense in TOML:
   - `path: Option<String>` — manual override of backend binary path (still useful in TOML)
   - `version: Option<String>` — version pin (still useful)
   - `gpu_variant: Option<String>` — variant pin (still useful)
   - **Remove**: `default_args`, `health_check_url` (now in DB)

2. Clean up any code that reads `backend.default_args` or `backend.health_check_url` from TOML (should be none after Tasks 2-3)

3. Update config serialization — the `[backends]` section in `config.toml` should no longer include `default_args` or `health_check_url`

**Steps:**
- [ ] Remove `default_args` and `health_check_url` fields from `BackendConfig` in `crates/tama-core/src/config/types.rs`
- [ ] Search for all references to `backend.default_args` and `backend.health_check_url` — verify none remain outside tests
- [ ] Update any test fixtures that construct `BackendConfig` with these fields
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: remove default_args and health_check_url from BackendConfig"

**Acceptance criteria:**
- [ ] `BackendConfig` no longer has `default_args` or `health_check_url` fields
- [ ] No code references these fields on TOML `BackendConfig`
- [ ] All tests pass
- [ ] Config TOML no longer serializes these fields

---

### Task 5: Update frontend + integration test

**Context:**
The frontend save function already keys edits by `"name:variant"`. It needs to pass `gpu_variant` as a required query param. Add an integration test for the full flow.

**Files:**
- Modify: `crates/tama-web/src/pages/backends.rs`
- Modify: `crates/tama-web/tests/server_test.rs`

**What to implement:**

1. In the save function (backends.rs), always include `gpu_variant` in the URL:
   ```rust
   for (key, args_str) in &*args_edits {
       let parts: Vec<&str> = key.splitn(2, ':').collect();
       let bt = parts[0];
       let gv = parts.get(1).copied().unwrap_or("cpu");
       let url = format!("/tama/v1/backends/{}/default-args?gpu_variant={}", bt, gv);
       // ... send request
   }
   ```

2. Integration test:
   - Create temp dir, initialize DB with migration
   - Seed `backend_configs` table with test data
   - Start test server with `config_path` pointing at temp dir
   - POST default_args with gpu_variant → verify DB row is created/updated
   - GET backends → verify per-variant args are returned
   - Verify different variants have independent args

**Steps:**
- [ ] Update the save function in `backends.rs` to always pass `gpu_variant` query param (default to "cpu" if not in key)
- [ ] Add integration test in `crates/tama-web/tests/server_test.rs`
- [ ] Test setup: temp dir with DB, seed backend_configs, start test server
- [ ] Test: POST with gpu_variant creates DB row, GET returns per-variant args
- [ ] Test: Different variants have independent args
- [ ] Run `cargo test --package tama-web --features ssr`
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: wire up DB-backed default_args in frontend + integration test"

**Acceptance criteria:**
- [ ] Frontend save always sends `gpu_variant` query param
- [ ] Integration test verifies full flow: save → DB → load → per-variant args
- [ ] All workspace tests pass
- [ ] No clippy warnings
