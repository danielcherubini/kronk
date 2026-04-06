# Backend Registry: Move from TOML File to SQLite Database

**Goal:** Migrate the backend registry from a `backend_registry.toml` file on disk to the SQLite database (`koji.db`), while keeping `config.toml` as the sole place where named backends are declared (with exactly `llama_cpp` and `ik_llama` as the two well-known names).

**Architecture:**
The current system has two parallel backend representations: `[backends]` in `config.toml` (path + default_args, used at runtime) and `backend_registry.toml` (install metadata like version, gpu_type, source). We will eliminate `backend_registry.toml` entirely. A new `backend_installations` table in SQLite will store all install metadata including version history and an `is_active` flag to mark the "current active version" for each named backend. The `BackendRegistry` struct will be rewritten to use a `rusqlite::Connection` instead of reading/writing a TOML file.

**Tech Stack:** Rust, SQLite via `rusqlite`, existing `crate::db` migration system, existing `koji-core` and `koji-cli` crates.

---

### Task 1: Add DB migration for `backend_installations` table

**Context:**
The existing migration system in `crates/koji-core/src/db/migrations.rs` uses a versioned array of `(i32, &'static str)` tuples. Each migration runs in its own transaction. The current `LATEST_VERSION` is `2`. We need to add migration `3` that creates the `backend_installations` table in `koji.db`. This is a pure data layer change — no business logic yet.

The new table must capture everything currently stored in `backend_registry.toml`:
- `id`: AUTOINCREMENT primary key
- `name`: the backend key (e.g. `"llama_cpp"`, `"ik_llama"`) — this is the name used in `config.toml [backends]`
- `backend_type`: serialized enum string (`"llama_cpp"`, `"ik_llama"`, `"custom"`)
- `version`: version string (e.g. `"b8407"`, `"main@abc12345"`)
- `path`: absolute path to the installed binary
- `installed_at`: unix timestamp (i64)
- `gpu_type`: JSON string (nullable, serialized `GpuType`)
- `source`: JSON string (nullable, serialized `BackendSource`)
- `is_active`: INTEGER NOT NULL DEFAULT 0 — marks which installation is the "current active version" for a given backend name

The `is_active` flag replaces the concept of "the single entry per backend name" that exists today. When a backend is updated, a new row is inserted and `is_active` is set to 1 for the new row, 0 for all previous rows with the same name. This preserves history.

There must be a UNIQUE constraint on `(name, version)` so the same version is not inserted twice.

There must NOT be a unique constraint on `name` alone — multiple versions of the same backend name can coexist, but only one should have `is_active = 1`.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs`

**What to implement:**
1. Add migration `(3, ...)` to the `migrations` array.
2. Update `LATEST_VERSION` from `2` to `3`.
3. The SQL for migration 3:
```sql
CREATE TABLE IF NOT EXISTS backend_installations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    backend_type TEXT NOT NULL,
    version TEXT NOT NULL,
    path TEXT NOT NULL,
    installed_at INTEGER NOT NULL,
    gpu_type TEXT,
    source TEXT,
    is_active INTEGER NOT NULL DEFAULT 0,
    UNIQUE(name, version)
);
CREATE INDEX IF NOT EXISTS idx_backend_installations_name ON backend_installations(name);
```

**Steps:**
- [ ] Write a failing test in `crates/koji-core/src/db/mod.rs` `#[cfg(test)]` block: call `open_in_memory()`, then assert that `backend_installations` table exists in `sqlite_master`. Name the test `test_migration_v3_creates_backend_installations`.
- [ ] Run `cargo test --package koji-core test_migration_v3_creates_backend_installations -- --nocapture`
  - Did it fail with "assertion failed" or "no such table"? Good, proceed.
- [ ] Add migration 3 to `migrations.rs`, update `LATEST_VERSION = 3`.
- [ ] Run `cargo test --package koji-core test_migration_v3_creates_backend_installations -- --nocapture`
  - Did it pass? If not, fix the SQL syntax and re-run.
- [ ] Also verify existing migration tests still pass: `cargo test --package koji-core -- db`
- [ ] Run `cargo fmt --all && cargo build --workspace`
  - Did both succeed? If not, fix and re-run.
- [ ] Commit with message: `"feat: add db migration v3 for backend_installations table"`

**Acceptance criteria:**
- [ ] `backend_installations` table is created in any fresh or upgraded DB
- [ ] `LATEST_VERSION` is updated to 3
- [ ] All existing db tests still pass
- [ ] `cargo build --workspace` succeeds

---

### Task 2: Add DB query functions for `backend_installations`

**Context:**
Following the existing pattern in `crates/koji-core/src/db/queries.rs`, we need typed query functions for the new `backend_installations` table. These are pure data access functions — no business logic. They will be called by the refactored `BackendRegistry` in a later task.

The functions needed:
- `insert_backend_installation(conn, record: &BackendInstallationRecord) -> Result<()>` — inserts a new row and marks it active; sets `is_active = 1` for the new row and `is_active = 0` for all other rows with the same `name`, all in a single transaction. If the `(name, version)` pair already exists (UNIQUE constraint violation), the function should return an error — do NOT use upsert semantics here; a duplicate version is a programming error.
- `get_active_backend(conn, name: &str) -> Result<Option<BackendInstallationRecord>>` — returns the row with `is_active = 1` for the given name
- `list_active_backends(conn) -> Result<Vec<BackendInstallationRecord>>` — returns all rows with `is_active = 1`
- `list_backend_versions(conn, name: &str) -> Result<Vec<BackendInstallationRecord>>` — returns all rows for a given name, ordered by `installed_at DESC`
- `delete_backend_installation(conn, name: &str, version: &str) -> Result<()>` — deletes a specific (name, version) row
- `delete_all_backend_versions(conn, name: &str) -> Result<()>` — deletes all rows for a backend name (for `backend remove`)

The `BackendInstallationRecord` struct fields:
```rust
pub struct BackendInstallationRecord {
    /// Set to 0 when constructing a record for INSERT (DB assigns the real id via AUTOINCREMENT).
    pub id: i64,
    pub name: String,
    pub backend_type: String,     // e.g. "llama_cpp"
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    pub gpu_type: Option<String>, // JSON string
    pub source: Option<String>,   // JSON string
    pub is_active: bool,
}
```

Note: `id` should be set to `0` when creating a record for insertion; the DB will assign the real `AUTOINCREMENT` value. When reading records back from the DB, `id` will be the actual row id.

**Files:**
- Modify: `crates/koji-core/src/db/queries.rs`

**What to implement:**
Add `BackendInstallationRecord` struct and the six functions listed above. For `insert_backend_installation`, use a single transaction that:
1. `INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_type, source, is_active) VALUES (...)` with `is_active = 1`
2. `UPDATE backend_installations SET is_active = 0 WHERE name = ?1 AND version != ?2`

Both DML statements must run inside a single `tx = conn.unchecked_transaction()` block with `tx.commit()` at the end.

**Steps:**
- [ ] Write failing tests in `crates/koji-core/src/db/queries.rs` `#[cfg(test)]` block:
  - `test_insert_and_get_active_backend`: insert a backend, then insert a second version of same name; assert only the second is active (call `get_active_backend` and assert it returns the second version).
  - `test_list_active_backends`: insert two backends with different names; assert `list_active_backends` returns 2.
  - `test_list_backend_versions`: insert two versions of same name; assert `list_backend_versions` returns 2 rows ordered newest first.
  - `test_delete_single_backend_version`: insert two versions of same name; delete one; assert `list_backend_versions` returns 1.
  - `test_delete_all_backend_versions`: insert 2 versions of same name, delete all, assert 0 remain.
  - `test_insert_duplicate_version_fails`: insert same (name, version) twice; assert the second insert returns an `Err`.
- [ ] Run `cargo test --package koji-core -- queries::tests::test_insert_and_get_active_backend`
  - Did it fail (compile error is fine)? Good, proceed.
- [ ] Implement `BackendInstallationRecord` and the six query functions in `queries.rs`.
- [ ] Run `cargo test --package koji-core -- queries`
  - Did all pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all && cargo build --workspace`
- [ ] Commit with message: `"feat: add db query functions for backend_installations"`

**Acceptance criteria:**
- [ ] `insert_backend_installation` correctly sets `is_active = 1` for the new row and `is_active = 0` for all older rows with the same name
- [ ] Inserting a duplicate `(name, version)` returns an `Err`
- [ ] All six query functions pass their tests
- [ ] `cargo build --workspace` succeeds

---

### Task 3: Rewrite `BackendRegistry` + update CLI to use SQLite (combined)

**Context:**
This task combines the registry rewrite and CLI update into a single commit to ensure the workspace always builds cleanly.

`BackendRegistry` currently reads/writes `backend_registry.toml` via the file system. The struct holds `path: PathBuf`, `base_dir: PathBuf`, `data: RegistryData`, and `read_only: bool`. We will replace the internal implementation to use a `rusqlite::Connection`, while updating all call sites in the CLI simultaneously.

**Affected files for `BackendRegistry` rewrite:**
- `crates/koji-core/src/backends/registry/registry_ops.rs` — full rewrite of `BackendRegistry`
- `crates/koji-core/src/backends/registry/mod.rs` — update re-exports
- `crates/koji-core/src/backends/registry/backup.rs` — remove module entirely
- `crates/koji-core/src/backends/mod.rs` — remove `RegistryData` from re-exports if present
- `crates/koji-core/src/backends/updater.rs` — adjust for `get()` return type change
- `crates/koji-core/Cargo.toml` — add `serde_json` if not present

**Affected files for CLI update:**
- `crates/koji-cli/src/commands/backend.rs` — update all `BackendRegistry` usage

#### Part A: New `BackendRegistry` design

The new `BackendRegistry` struct:
```rust
pub struct BackendRegistry {
    conn: Connection,
}
```

Keep `BackendInfo`, `BackendSource`, `BackendType` types completely unchanged — these are the canonical Rust types used throughout the codebase.

Remove entirely: `RegistryData`, and all file-based methods (`save()`, `load()`, `load_with_base_dir()`, `from_backends()`, `load_unchecked()`, `save_unchecked()`, `load_unchecked_with_base_dir()`, `add_unchecked()`, `remove_unchecked()`, `is_read_only()`, `set_read_only()`, `path()`, `base_dir()`, `data()`, `data_mut()`).

New constructor signatures:
- `BackendRegistry::open(config_dir: &Path) -> Result<Self>` — takes the **config directory** (NOT the .db file path), calls `crate::db::open(config_dir)` which internally opens `<config_dir>/koji.db`. This matches the convention of `crate::db::open`.
- `BackendRegistry::open_in_memory() -> Result<Self>` — calls `crate::db::open_in_memory()`. For testing.

Public methods to preserve (with updated signatures):
- `fn add(&mut self, backend: BackendInfo) -> Result<()>` — inserts a new installation row and marks it active
- `fn remove(&mut self, name: &str) -> Result<()>` — deletes ALL versions for a backend name
- `fn get(&self, name: &str) -> Result<Option<BackendInfo>>` — **IMPORTANT CHANGE:** returns `Result<Option<BackendInfo>>` (owned value, fallible) instead of `Option<&BackendInfo>`. The DB lookup can fail, so this must return `Result`.
- `fn list(&self) -> Result<Vec<BackendInfo>>` — **IMPORTANT CHANGE:** returns `Result<Vec<BackendInfo>>` (owned, fallible) instead of `Vec<&BackendInfo>`.
- `fn update_version(&mut self, name: &str, new_version: String, new_binary_path: PathBuf, new_source: Option<BackendSource>) -> Result<()>` — constructs a new `BackendInfo` with updated fields and calls `add()` (which marks the new row active and deactivates the old one).

Helper functions (private, not pub):
- `fn record_to_backend_info(record: BackendInstallationRecord) -> Result<BackendInfo>` — deserializes JSON fields (`gpu_type`, `source`) using `serde_json::from_str`
- `fn backend_info_to_record(backend: &BackendInfo) -> Result<BackendInstallationRecord>` — serializes JSON fields using `serde_json::to_string`. Set `id: 0` (placeholder for AUTOINCREMENT).

**Serialization for `gpu_type` and `source`:** Use `serde_json::to_string()` and `serde_json::from_str()`. Both `GpuType` and `BackendSource` already derive `Serialize`/`Deserialize`.

#### Part B: CLI updates in `backend.rs`

Replace:
```rust
fn registry_path() -> Result<std::path::PathBuf> {
    let base_dir = Config::base_dir()?;
    Ok(base_dir.join("backend_registry.toml"))
}
```
With:
```rust
fn registry_config_dir() -> Result<std::path::PathBuf> {
    Config::base_dir()
}
```

Replace all `BackendRegistry::load(&registry_path()?)` calls with `BackendRegistry::open(&registry_config_dir()?)`.

Update `get()` call sites: the new signature is `Result<Option<BackendInfo>>` not `Option<&BackendInfo>`:
```rust
// OLD:
let backend_info = registry.get(name).ok_or_else(|| anyhow!("..."))?  .clone();
// NEW:
let backend_info = registry.get(name)?.ok_or_else(|| anyhow!("..."))?;
```

Update `list()` call sites: `let backends = registry.list()?;` (add `?`).

In `cmd_check_updates`, the loop changes from:
```rust
for backend in backends {   // backend: &BackendInfo
    check_updates(backend)  // already a reference
```
To:
```rust
for backend in backends {   // backend: BackendInfo (owned)
    check_updates(&backend) // must add &
```

#### Part C: `updater.rs` updates

In `crates/koji-core/src/backends/updater.rs`, the `update_backend` function calls `registry.get(backend_name)`. Change it from:
```rust
registry
    .get(backend_name)
    .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;
```
To:
```rust
registry
    .get(backend_name)?
    .ok_or_else(|| anyhow!("Backend '{}' not found", backend_name))?;
```

**Files:**
- Rewrite: `crates/koji-core/src/backends/registry/registry_ops.rs`
- Modify: `crates/koji-core/src/backends/registry/mod.rs`
- Delete contents of: `crates/koji-core/src/backends/registry/backup.rs` (replace with empty file is fine, or remove `pub mod backup;` from `mod.rs`)
- Modify: `crates/koji-core/src/backends/mod.rs`
- Modify: `crates/koji-core/src/backends/updater.rs`
- Modify: `crates/koji-core/Cargo.toml`
- Modify: `crates/koji-cli/src/commands/backend.rs`

**Steps:**
- [ ] Check `crates/koji-core/Cargo.toml` for `serde_json`; add it under `[dependencies]` if missing (e.g., `serde_json = { version = "1", features = ["std"] }`).
- [ ] Write failing tests in `registry_ops.rs` `#[cfg(test)]`:
  - `test_registry_add_and_list`: use `BackendRegistry::open_in_memory()`, add a backend with name `"llama_cpp"`, call `list()`, assert 1 entry with correct name.
  - `test_registry_remove`: add then remove a backend, assert `list()` returns empty vec.
  - `test_registry_update_version`: add backend v1, call `update_version` to set v2, assert `get("llama_cpp")` returns a backend with version v2.
  - `test_registry_get_returns_none_for_unknown`: call `get("nonexistent")`, assert `Ok(None)`.
- [ ] Run `cargo test --package koji-core -- backends::registry::tests`
  - These will fail (compile error because the new API doesn't exist yet). That's expected.
- [ ] Rewrite `registry_ops.rs` with the new `BackendRegistry` implementation.
- [ ] Update `registry/mod.rs`: remove `RegistryData` from `pub use`, remove `pub mod backup;`.
- [ ] Update `crates/koji-core/src/backends/mod.rs`: remove `RegistryData` re-export if present.
- [ ] Update `updater.rs` as described in Part C.
- [ ] Run `cargo build --package koji-core`
  - Did it succeed? If not, fix `koji-core` compilation errors before proceeding.
- [ ] Run `cargo test --package koji-core -- backends::registry`
  - Did all pass?
- [ ] Now update `crates/koji-cli/src/commands/backend.rs` as described in Part B.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix remaining errors and re-run.
- [ ] Run `cargo test --workspace`
  - Did all pass?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did both succeed? If not, fix and re-run.
- [ ] Commit with message: `"refactor: replace file-based BackendRegistry with SQLite-backed implementation"`

**Acceptance criteria:**
- [ ] `BackendRegistry` no longer reads or writes any `.toml` file
- [ ] `BackendInfo`, `BackendSource`, `BackendType` types are unchanged
- [ ] All `koji-core` tests pass
- [ ] All `koji-cli` tests pass
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] No reference to `backend_registry.toml` or `registry_path()` remains in CLI code

---

### Task 4: Clean up `config.toml` backend declarations

**Context:**
After the registry is fully DB-backed, the `[backends]` section in `config.toml` is still used at **runtime** (by `config.resolve_server()` and `config.build_args()`) — it provides the path to the binary and `default_args`. The user wants only the two well-known named backends in `config.toml`: `llama_cpp` and `ik_llama`. 

Currently, `Config::default()` in `crates/koji-core/src/config/loader.rs` hardcodes only `llama_cpp`. We should also add `ik_llama` with sensible defaults.

Additionally, the `BackendConfig.path` field should be auto-resolved from the DB (the active installation's `path`) if no path is set in config. We will make `path` an `Option<String>` and add a `Config::resolve_backend_path()` helper that accepts a `&Connection`.

**Important — understand the blast radius before editing:**
Before implementing, search the entire codebase for direct uses of `backend.path` or `BackendConfig.path` to find ALL call sites:
```
cargo grep -r "backend\.path\|BackendConfig" --include="*.rs"
```
Known affected call sites (as of current code):
- `crates/koji-cli/src/handlers/run.rs` — reads `backend.path` to get the binary path for spawning the process
- `crates/koji-cli/src/handlers/service_cmd.rs` — reads `backend.path`
- `crates/koji-cli/src/handlers/serve.rs` OR `service.rs` — may read `backend.path`

ALL of these must be updated in this task to use `resolve_backend_path()` instead of `backend.path` directly. If any are missed, the code will fail to compile.

**New `resolve_backend_path` design:**
```rust
// In crates/koji-core/src/config/resolve.rs
use rusqlite::Connection;
use crate::db::queries::get_active_backend;

impl Config {
    /// Resolve the filesystem path for a named backend binary.
    ///
    /// Priority:
    /// 1. Active installation in the DB (via `get_active_backend`)
    /// 2. `path` field in `config.toml` [backends] section (for custom/manual installs)
    ///
    /// Returns an error if neither source has a path.
    pub fn resolve_backend_path(&self, name: &str, conn: &Connection) -> Result<PathBuf> {
        if let Some(record) = get_active_backend(conn, name)? {
            return Ok(PathBuf::from(record.path));
        }
        self.backends
            .get(name)
            .and_then(|b| b.path.as_deref())
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!(
                "Backend '{}' has no installed path. Run `koji backend install {}` first.",
                name, name
            ))
    }
}
```

The `Connection` must be threaded through to call sites. In the CLI handlers, the `Connection` comes from `crate::db::open(&Config::base_dir()?)`. Check how the existing CLI handlers receive/create connections and follow the same pattern.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs` — make `BackendConfig.path` an `Option<String>`
- Modify: `crates/koji-core/src/config/loader.rs` — update `Config::default()` to include both `llama_cpp` and `ik_llama` (both with `path: None`); update existing `llama_cpp` entry from `path: String` to `path: None`
- Modify: `crates/koji-core/src/config/resolve.rs` — add `resolve_backend_path()` method; add `use rusqlite::Connection;`
- Modify: `crates/koji-cli/src/handlers/run.rs` — replace `backend.path` with `config.resolve_backend_path(&backend_name, &conn)?`
- Modify: `crates/koji-cli/src/handlers/service_cmd.rs` — same
- Modify any other CLI handler files that use `backend.path` directly (verify with search)

**What to implement in `types.rs`:**
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,   // Was: pub path: String
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
}
```

**What to implement in `loader.rs` `Config::default()`:**
```rust
backends.insert("llama_cpp".to_string(), BackendConfig {
    path: None,
    default_args: vec![],
    health_check_url: Some("http://localhost:8080/health".to_string()),
});
backends.insert("ik_llama".to_string(), BackendConfig {
    path: None,
    default_args: vec![],
    health_check_url: Some("http://localhost:8080/health".to_string()),
});
```

**Steps:**
- [ ] Search for all uses of `backend.path` (as `BackendConfig`) in the codebase: `rg "backend\.path" crates/koji-cli/src/handlers/`. Note every file and line.
- [ ] Write failing tests in `crates/koji-core/src/config/resolve.rs` `#[cfg(test)]` block:
  - `test_resolve_backend_path_from_db`: create an in-memory DB (`crate::db::open_in_memory()`), insert a backend installation with `name = "llama_cpp"` and `path = "/usr/local/bin/llama-server"`, call `config.resolve_backend_path("llama_cpp", &conn)`, assert it returns `PathBuf::from("/usr/local/bin/llama-server")`.
  - `test_resolve_backend_path_fallback`: empty DB, but `config.backends["llama_cpp"].path = Some("/fallback/llama-server")`, assert fallback is returned.
  - `test_resolve_backend_path_error`: empty DB and `path = None`, assert `Err`.
- [ ] Run `cargo test --package koji-core -- config::resolve`
  - Did they fail? Good.
- [ ] Implement `BackendConfig.path: Option<String>` in `types.rs`.
- [ ] Update `Config::default()` in `loader.rs`.
- [ ] Implement `resolve_backend_path()` in `resolve.rs`.
- [ ] Run `cargo build --workspace` to find all compilation errors from the `path` type change.
  - Note every error — these are the call sites that need updating.
- [ ] Fix all compilation errors by updating call sites to use `resolve_backend_path()`.
  - For each handler file: open a DB connection with `crate::db::open(&Config::base_dir()?)` (or pass through an existing connection if one already exists in scope), then call `config.resolve_backend_path(&name, &conn)?`.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix remaining errors.
- [ ] Run `cargo test --workspace`
  - Did all pass?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `"feat: make BackendConfig.path optional with DB fallback; add ik_llama default backend"`

**Acceptance criteria:**
- [ ] `BackendConfig.path` is `Option<String>`
- [ ] `Config::default()` declares both `llama_cpp` and `ik_llama` backends (with `path: None`)
- [ ] `resolve_backend_path()` returns the active DB-installed path, or falls back to the config path, or errors clearly
- [ ] No direct reads of `BackendConfig.path` remain in CLI handlers (all go through `resolve_backend_path`)
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 5: Add DB migration backfill from existing `backend_registry.toml`

**Context:**
Users who already have a `backend_registry.toml` on disk need their existing backend entries migrated into the new `backend_installations` table in `koji.db`. This backfill should happen once at startup, detected by checking whether the `backend_registry.toml` file exists. After migrating, the file is renamed to `backend_registry.toml.migrated` so it is not re-imported.

First, read `crates/koji-core/src/db/backfill.rs` to understand the existing backfill pattern, and search for where `backfill` is called in the CLI (`rg "backfill" crates/koji-cli/`).

The existing `db::backfill` module should be extended with a new function.

Note: `RegistryData` was removed from the public API in Task 3. For backfill purposes only, define a minimal private deserialization struct in `backfill.rs` that matches the old TOML format:
```rust
#[derive(serde::Deserialize)]
struct LegacyRegistryData {
    #[serde(default)]
    backends: std::collections::HashMap<String, LegacyBackendInfo>,
}

#[derive(serde::Deserialize)]
struct LegacyBackendInfo {
    // Note: name is also stored as the HashMap key; use the HashMap key as the canonical name.
    backend_type: crate::backends::BackendType,
    version: String,
    path: std::path::PathBuf,
    installed_at: i64,
    gpu_type: Option<crate::gpu::GpuType>,
    source: Option<crate::backends::BackendSource>,
}
```

When iterating `for (name, info) in registry_data.backends`, use `name` (the HashMap key) as the canonical backend name, not any `name` field inside the struct.

**Files:**
- Modify: `crates/koji-core/src/db/backfill.rs`
- Modify: the CLI file(s) that call `backfill::run()` — add a call to the new migration function after the existing backfill logic

**What to implement:**
Add a new function `pub fn migrate_backend_registry_toml(conn: &Connection, config_dir: &Path) -> Result<()>` that:
1. Constructs `registry_path = config_dir.join("backend_registry.toml")`
2. If the file does not exist, return `Ok(())` immediately
3. Read and parse the file as `LegacyRegistryData` using `toml::from_str`
4. For each `(name, info)` in `registry_data.backends`:
   - Convert to a `BackendInstallationRecord` (serialize `gpu_type` and `source` to JSON using `serde_json::to_string`)
   - Call `crate::db::queries::insert_backend_installation(conn, &record)`
   - If the insertion fails due to a duplicate (UNIQUE constraint error), log a warning and continue — don't abort the whole migration
5. Rename the file to `backend_registry.toml.migrated`
6. Log `tracing::info!("Migrated {} backends from backend_registry.toml", count)`

Wire it into the CLI startup: call `migrate_backend_registry_toml(&conn, &config_dir)` in the same place existing backfill is called. This function should run unconditionally (not only when `needs_backfill` is true) because the DB might already exist but the `.toml` file could still be present from a partial migration.

**Steps:**
- [ ] Read `crates/koji-core/src/db/backfill.rs` to understand the existing pattern.
- [ ] Run `rg "backfill" crates/koji-cli/src/` to find the call site(s).
- [ ] Write a failing test in `backfill.rs` `#[cfg(test)]`: `test_migrate_backend_registry_toml`:
  - Create a `TempDir`
  - Write a minimal `backend_registry.toml` to it with one backend entry (use the old TOML format with `[backends.llama_cpp]` etc.)
  - Create an in-memory DB and call `migrate_backend_registry_toml(&conn, temp_dir.path())`
  - Assert that `get_active_backend(&conn, "llama_cpp")` returns `Some(...)` with the correct version
  - Assert that `temp_dir.path().join("backend_registry.toml.migrated")` exists
  - Assert that `temp_dir.path().join("backend_registry.toml")` no longer exists
- [ ] Run `cargo test --package koji-core -- db::backfill`
  - Did it fail? Good.
- [ ] Implement `migrate_backend_registry_toml()` in `backfill.rs`.
- [ ] Wire it into the CLI startup.
- [ ] Run `cargo test --package koji-core -- db::backfill`
  - Did all pass?
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: `"feat: add one-time migration from backend_registry.toml to SQLite DB"`

**Acceptance criteria:**
- [ ] If `backend_registry.toml` exists, its entries are imported into `backend_installations` on first run
- [ ] After migration, `backend_registry.toml` is renamed to `backend_registry.toml.migrated`
- [ ] If no `backend_registry.toml` exists, the function is a no-op
- [ ] Duplicate entries (already in DB) are skipped with a warning, not fatal
- [ ] `cargo test --workspace` passes
