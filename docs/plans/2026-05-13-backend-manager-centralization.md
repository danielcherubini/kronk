# BackendManager Centralization Plan

**Goal:** Create a `BackendManager` struct in `tama-core` that centralizes all backend data access (config, discovery, installation, resolution), replacing scattered `db::queries::*` calls and absorbing `BackendRegistry`.

**Architecture:** `BackendManager` wraps a `rusqlite::Connection` and exposes methods for all backend DB operations. Callers open a fresh instance per operation (SQLite open is cheap, `Connection` is `Send` but not `Sync`). `BackendRegistry` gets deprecated and removed. `Config::resolve_*` and `build_*` lose their `db_conn: Option<&Connection>` parameters — callers pre-resolve values via `BackendManager` and pass concrete data.

**Tech Stack:** Rust, rusqlite, Axum API, Leptos frontend

---

### Task 1: Create BackendManager struct + config/discovery methods

**Context:**
This is the foundation. `BackendManager` wraps a `Connection` and replaces the pattern where every caller opens `crate::db::open()` and passes `&conn` to `db::queries::*` functions. The struct goes in `crates/tama-core/src/backends/manager.rs` (new file). The `BackendOption` struct currently lives in `tama-web/src/api/models/info.rs` — it moves to `tama-core` so `BackendManager::available_backends()` can return it.

**Files:**
- Create: `crates/tama-core/src/backends/manager.rs`
- Create: `crates/tama-core/src/backends/types.rs` (extract BackendInfo, BackendSource, BackendType from registry_ops.rs)
- Modify: `crates/tama-core/src/backends/mod.rs` (add `pub mod manager;`, `pub mod types;`, re-export)
- Modify: `crates/tama-core/src/backends/registry/registry_ops.rs` (import types from `crate::backends::types` instead of defining locally)
- Modify: `crates/tama-web/src/api/models/info.rs` (import `BackendOption` from `tama_core::backends`)
- Test: `crates/tama-core/src/backends/manager.rs` (inline `#[cfg(test)]` module)

**What to implement:**

1. Create `crates/tama-core/src/backends/manager.rs`:

```rust
use anyhow::{Context, Result};
use rusqlite::Connection;

/// A single backend option for UI dropdowns (e.g. model editor backend selector).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct BackendOption {
    pub name: String,
    #[serde(default)]
    pub variant: Option<String>,
    pub label: String,
}

/// Centralized backend data access. Each caller opens its own instance.
/// `Connection` is `Send` but not `Sync` — do not share across threads.
pub struct BackendManager {
    conn: Connection,
}

impl BackendManager {
    /// Open from config directory. Runs DB migrations on first open.
    /// Also runs legacy backend migration (idempotent, no-op if already done).
    pub fn open(config_dir: &std::path::Path) -> Result<Self> {
        let open_result = crate::db::open(config_dir)?;

        // Run legacy backend migration (idempotent, no-op if already done)
        let backends_dir = config_dir.join("backends");
        crate::backends::migration::migrate_legacy_backends(
            &open_result.conn,
            &backends_dir,
        ).context("Failed to run legacy backend migration")?;

        Ok(Self {
            conn: open_result.conn,
        })
    }

    /// Open an in-memory database for testing.
    pub fn open_in_memory() -> Result<Self> {
        let open_result = crate::db::open_in_memory()?;
        Ok(Self {
            conn: open_result.conn,
        })
    }

    // ── Config (backend_configs table) ──────────────────────────

    /// Get config for a backend name + gpu_variant pair.
    pub fn get_config(
        &self,
        name: &str,
        gpu_variant: &str,
    ) -> Result<Option<crate::db::queries::BackendConfigRecord>> {
        crate::db::queries::get_backend_config(&self.conn, name, gpu_variant)
    }

    /// Insert or update config. Returns the row's integer id.
    pub fn save_config(
        &self,
        name: &str,
        gpu_variant: &str,
        default_args: &[String],
        health_check_url: Option<&str>,
    ) -> Result<i64> {
        crate::db::queries::upsert_backend_config(
            &self.conn,
            name,
            gpu_variant,
            default_args,
            health_check_url,
        )
    }

    /// List all backend config rows.
    pub fn list_configs(&self) -> Result<Vec<crate::db::queries::BackendConfigRecord>> {
        crate::db::queries::list_backend_configs(&self.conn)
    }

    // ── Discovery ───────────────────────────────────────────────

    /// Return backend options for UI dropdowns (name, variant, label).
    /// Discovers from active installations in `backend_installations` table.
    pub fn available_backends(&self) -> Result<Vec<BackendOption>> {
        let active = crate::db::queries::list_active_backends(&self.conn)?;
        let mut seen = std::collections::HashSet::new();
        let mut options = Vec::new();
        for record in &active {
            let key = (record.name.clone(), record.gpu_variant.clone());
            if seen.insert(key.clone()) {
                options.push(BackendOption {
                    name: key.0.clone(),
                    variant: Some(key.1.clone()),
                    label: if key.1 == "cpu" {
                        key.0.clone()
                    } else {
                        format!("{} ({})", key.0, key.1)
                    },
                });
            }
        }
        Ok(options)
    }
}
```

2. Update `crates/tama-core/src/backends/mod.rs`:
```rust
pub mod manager;
pub use manager::{BackendManager, BackendOption};
```

3. In `crates/tama-web/src/api/models/info.rs`:
   - Remove the local `BackendOption` struct definition (lines ~13-18)
   - Remove the local `build_backend_options` function (replaced by `mgr.available_backends()` — but don't switch callers yet, that's Task 6)
   - Add `use tama_core::backends::BackendOption;`
   - Keep the existing `build_backend_options` function as-is for now (it still recieves `BackendOption` from `tama_core`)

   Wait — the current `build_backend_options` function defines `BackendOption` locally. We need to:
   - Import `BackendOption` from `tama_core::backends`
   - Remove the local `struct BackendOption` definition
   - The `build_backend_options` function body stays the same (it constructs `BackendOption` values — those now use the tama-core type)

4. In `crates/tama-web/src/pages/model_editor/types.rs`:
   - Find the existing `BackendOption` struct (likely named the same or similar)
   - Replace the local definition with `pub use tama_core::backends::BackendOption;`

**Steps:**
- [ ] Create `crates/tama-core/src/backends/types.rs` by extracting `BackendInfo`, `BackendSource`, `BackendType` (and `FromStr` impl) from `registry_ops.rs` into a shared module. Update `registry_ops.rs` to `pub use crate::backends::types::*` or re-import.
- [ ] Update `crates/tama-core/src/backends/mod.rs` to re-export `manager` module and `BackendOption`
- [ ] In `crates/tama-web/src/api/models/info.rs`: import `BackendOption`, remove local struct
- [ ] In `crates/tama-web/src/pages/model_editor/types.rs`: replace local `BackendOption` with re-export
- [ ] Write tests in `manager.rs`:
  - `test_open_in_memory_creates_instance`
  - `test_save_and_get_config_roundtrip`
  - `test_save_config_updates_existing`
  - `test_get_config_returns_none_for_missing`
  - `test_list_configs_returns_all`
  - `test_available_backends_returns_options`
  - `test_available_backends_groups_by_variant`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "feat: add BackendManager struct with config and discovery methods"

**Acceptance criteria:**
- [ ] `BackendManager::open(config_dir)` opens DB and returns instance
- [ ] `get_config` / `save_config` / `list_configs` read/write `backend_configs` table
- [ ] `available_backends()` returns per-variant options from `backend_installations`
- [ ] `BackendOption` defined once in `tama-core`, used by both `tama-web` files
- [ ] All existing tests pass

---

### Task 2: Add installation methods to BackendManager

**Context:**
`BackendRegistry` currently wraps `backend_installations` queries. This task adds those methods to `BackendManager` so `BackendRegistry` can be replaced later. All methods delegate to existing `db::queries::*` functions. This task does NOT switch any callers yet — it just adds the methods with tests.

**Files:**
- Modify: `crates/tama-core/src/backends/manager.rs`
- Test: `crates/tama-core/src/backends/manager.rs` (inline)

**What to implement:**

Add these methods to `BackendManager` in `manager.rs`. Each delegates to the existing `db::queries::*` function:

```rust
// ── Installation (backend_installations table) ─────────────

use crate::backends::types::{BackendInfo, BackendSource, BackendType};

/// Add a new backend installation, marking it as the active version.
/// Delegates to `insert_backend_installation` which handles INSERT OR REPLACE
/// and deactivates other versions of the same (name, gpu_variant).
pub fn add_installation(&self, info: &BackendInfo) -> Result<()> {
    let record = Self::info_to_record(info)?;
    crate::db::queries::insert_backend_installation(&self.conn, &record)
        .with_context(|| format!("Failed to insert backend '{}'", info.name))
}

/// Get the active installation for a name + variant.
pub fn get_active(
    &self,
    name: &str,
    gpu_variant: &str,
) -> Result<Option<BackendInfo>> {
    let record = crate::db::queries::get_active_backend(&self.conn, name, gpu_variant)?;
    match record {
        Some(r) => Ok(Some(Self::record_to_info(r)?)),
        None => Ok(None),
    }
}

/// List all active backend installations (one per name+variant).
pub fn list_active(&self) -> Result<Vec<BackendInfo>> {
    let records = crate::db::queries::list_active_backends(&self.conn)?;
    records.into_iter().map(Self::record_to_info).collect()
}

/// List all versions of a backend.
/// If `gpu_variant` is Some, filters to that variant. If None, returns all variants.
/// Returns None if no versions exist for this name.
pub fn list_versions(
    &self,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<Option<Vec<BackendInfo>>> {
    let records = crate::db::queries::list_backend_versions(&self.conn, name, gpu_variant)?;
    if records.is_empty() {
        Ok(None)
    } else {
        Ok(Some(
            records.into_iter().map(Self::record_to_info).collect::<Result<Vec<_>>>()?,
        ))
    }
}

/// Get a specific installation by (name, gpu_variant, version).
pub fn get_by_version(
    &self,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<Option<BackendInfo>> {
    let record =
        crate::db::queries::get_backend_by_version(&self.conn, name, gpu_variant, version)?;
    match record {
        Some(r) => Ok(Some(Self::record_to_info(r)?)),
        None => Ok(None),
    }
}

/// Activate a specific version for a name + variant.
/// Deactivates all other versions of the same (name, gpu_variant).
/// Returns true if the version was found and activated.
pub fn activate(
    &self,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<bool> {
    crate::db::queries::activate_backend_version(&self.conn, name, gpu_variant, version)
}

/// Update an existing backend to a new version (convenience).
/// Reads the current active installation, builds a new BackendInfo with
/// updated version/path/source, and calls add_installation.
pub fn update_version(
    &self,
    name: &str,
    gpu_variant: &str,
    new_version: String,
    new_path: std::path::PathBuf,
    new_source: Option<BackendSource>,
) -> Result<()> {
    let existing = self
        .get_active(name, gpu_variant)?
        .ok_or_else(|| anyhow::anyhow!("Backend '{}' variant '{}' not found", name, gpu_variant))?;
    let updated = BackendInfo {
        name: existing.name,
        backend_type: existing.backend_type,
        version: new_version,
        path: new_path,
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64),
        gpu_type: existing.gpu_type,
        gpu_variant: existing.gpu_variant,
        source: new_source,
    };
    self.add_installation(&updated)
}

/// Delete a specific (name, gpu_variant, version) installation row.
/// If the deleted version was active, re-activates the newest remaining version.
pub fn remove_version(
    &self,
    name: &str,
    gpu_variant: &str,
    version: &str,
) -> Result<()> {
    // Check if the target version exists before deleting
    let existing = crate::db::queries::get_backend_by_version(
        &self.conn, name, gpu_variant, version,
    )?;
    let was_active = existing.as_ref().map(|r| r.is_active).unwrap_or(false);

    crate::db::queries::delete_backend_installation(
        &self.conn, name, gpu_variant, version,
    )?;

    // If we deleted the active version, activate the newest remaining one
    if was_active {
        let remaining = crate::db::queries::list_backend_versions(
            &self.conn, name, Some(gpu_variant),
        )?;
        if let Some(newest) = remaining.first() {
            crate::db::queries::activate_backend_version(
                &self.conn, name, gpu_variant, &newest.version,
            )?;
        }
    }

    Ok(())
}

/// Delete all versions of a backend.
/// If `gpu_variant` is Some, only deletes that variant.
/// If None, deletes all variants.
pub fn delete_all_versions(
    &self,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    crate::db::queries::delete_all_backend_versions(&self.conn, name, gpu_variant)
}
```

Also add the private helper methods (copied from BackendRegistry):

```rust
// ── Private helpers ──────────────────────────────────────

fn info_to_record(info: &BackendInfo) -> Result<crate::db::queries::BackendInstallationRecord> {
    let gpu_type_json = info
        .gpu_type
        .as_ref()
        .map(|g| serde_json::to_string(g))
        .transpose()
        .context("Failed to serialize gpu_type")?;
    let source_json = info
        .source
        .as_ref()
        .map(|s| serde_json::to_string(s))
        .transpose()
        .context("Failed to serialize source")?;
    Ok(crate::db::queries::BackendInstallationRecord {
        id: 0,
        name: info.name.clone(),
        backend_type: info.backend_type.to_string(),
        version: info.version.clone(),
        path: info.path.to_string_lossy().to_string(),
        installed_at: info.installed_at,
        gpu_type: gpu_type_json,
        gpu_variant: info.gpu_variant.clone(),
        source: source_json,
        is_active: true,
    })
}

fn record_to_info(record: crate::db::queries::BackendInstallationRecord) -> Result<BackendInfo> {
    let gpu_type = record
        .gpu_type
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("Failed to deserialize gpu_type")?;
    let source = record
        .source
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .context("Failed to deserialize source")?;
    Ok(BackendInfo {
        name: record.name,
        backend_type: record
            .backend_type
            .parse()
            .unwrap_or(BackendType::LlamaCpp),
        version: record.version,
        path: std::path::PathBuf::from(record.path),
        installed_at: record.installed_at,
        gpu_type,
        gpu_variant: record.gpu_variant,
        source,
    })
}
```

The necessary imports at the top of `manager.rs` already include `use anyhow::{Context, Result};` which covers `with_context`. Add `use anyhow::anyhow;` if needed for `update_version`.

**Steps:**
- [ ] Add the installation methods and helpers to `manager.rs`
- [ ] Add tests:
  - `test_add_and_get_installation`
  - `test_add_installation_replaces_old`
  - `test_list_active_returns_all`
  - `test_list_versions_by_variant`
  - `test_activate_switches_active`
  - `test_remove_version_deletes_row`
  - `test_delete_all_versions_with_variant`
  - `test_delete_all_versions_without_variant_deletes_all`
  - `test_update_version_convenience`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "feat: add installation methods to BackendManager"

**Acceptance criteria:**
- [ ] All 9 installation management methods work against `backend_installations`
- [ ] `delete_all_versions(name, None)` removes all variants
- [ ] `delete_all_versions(name, Some("cpu"))` removes only cpu variant
- [ ] `update_version` convenience works end-to-end
- [ ] All existing tests pass

---

### Task 3: Add resolution methods to BackendManager

**Context:**
Currently `Config::resolve_health_url`, `Config::resolve_backend_url`, and `build_args`/`build_full_args` all accept `db_conn: Option<&Connection>` to look up data from `backend_configs`. This task adds resolution methods to `BackendManager` that perform those lookups. Later tasks will update `Config` methods to accept pre-resolved values instead of `db_conn`.

**Files:**
- Modify: `crates/tama-core/src/backends/manager.rs`
- Test: `crates/tama-core/src/backends/manager.rs` (inline)

**What to implement:**

Add these methods to `BackendManager`:

```rust
// ── Resolution ────────────────────────────────────────────

/// Get default_args for a backend + variant from backend_configs.
/// Returns empty vec if no config exists.
pub fn get_default_args(&self, name: &str, gpu_variant: &str) -> Vec<String> {
    crate::db::queries::get_backend_config(&self.conn, name, gpu_variant)
        .ok()
        .flatten()
        .map(|c| c.default_args)
        .unwrap_or_default()
}

/// Get health_check_url from backend_configs.
pub fn get_health_check_url(&self, name: &str, gpu_variant: &str) -> Option<String> {
    crate::db::queries::get_backend_config(&self.conn, name, gpu_variant)
        .ok()
        .flatten()
        .and_then(|c| c.health_check_url)
}
```

**Steps:**
- [ ] Add the three resolution methods to `manager.rs`
- [ ] Add `use url;` if not already in `manager.rs`
- [ ] Add tests:
  - `test_get_default_args_returns_args`
  - `test_get_default_args_returns_empty_for_missing`
  - `test_get_health_check_url_returns_url`
  - `test_get_health_check_url_returns_none_for_missing`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "feat: add resolution methods to BackendManager"

**Acceptance criteria:**
- [ ] `get_default_args` returns Vec<String> from DB, empty if absent
- [ ] `get_health_check_url` returns Option<String> from DB
- [ ] All existing tests pass

---

### Task 4: Update Config signatures to remove db_conn parameters

**Context:**
After Tasks 1-3, `BackendManager` has resolution methods. This task updates `Config::build_args`, `build_full_args`, `resolve_health_url`, `resolve_backend_url`, `resolve_health_check`, and `resolve_backend_path` to accept pre-resolved values (or `&BackendManager`) instead of raw `Option<&Connection>`. All callers of these methods must be updated simultaneously so the build doesn't break.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs` (method signatures and bodies)
- Modify: `crates/tama-core/src/proxy/lifecycle/mod.rs` (update call sites)
- Modify: `crates/tama-core/src/proxy/state.rs` (update call sites)
- Modify: `crates/tama-core/src/proxy/types.rs` (update call sites if any)
- Modify: `crates/tama-core/src/bench/runner.rs` (update call sites)
- Modify: `crates/tama-cli/src/handlers/run.rs` (update call sites)
- Modify: `crates/tama-cli/src/handlers/service_cmd.rs` (update call sites)
- Modify: `crates/tama-cli/src/handlers/server/ls.rs` (update call sites)
- Modify: `crates/tama-core/src/config/resolve/tests/args_building.rs` (update test calls)
- Modify: `crates/tama-core/src/config/resolve/tests/kv_cache_types.rs` (if any calls)
- Modify: `crates/tama-core/src/config/resolve/tests/server_resolution.rs` (if any calls)
- Modify: `crates/tama-core/src/config/resolve/tests/path_resolution.rs` (if any calls)

**What to implement:**

1. In `crates/tama-core/src/config/resolve/mod.rs`, update method signatures:

```rust
// build_args: db_conn: Option<&Connection> → default_args: &[String]
pub fn build_args(
    &self,
    server: &ModelConfig,
    #[allow(dead_code)] backend: &BackendConfig,
    default_args: &[String],
) -> Vec<String> {
    let mut grouped = crate::config::merge_args(default_args, &server.args);
    // ... rest unchanged
}

// build_full_args: db_conn: Option<&Connection> → default_args: &[String]
pub fn build_full_args(
    &self,
    server: &ModelConfig,
    #[allow(dead_code)] backend: &BackendConfig,
    ctx_override: Option<u32>,
    default_args: &[String],
) -> Result<Vec<String>> {
    let mut grouped = crate::config::merge_args(default_args, &server.args);
    // ... rest unchanged
}

// resolve_health_url: db_conn: Option<&Connection> → health_check_url: Option<&str>
pub fn resolve_health_url(
    &self,
    server: &ModelConfig,
    health_check_url: Option<&str>,
) -> Option<String> {
    let backend = match self.backends.get(&server.backend) {
        Some(b) => b,
        None => {
            tracing::warn!("Backend '{}' not found when resolving health URL", server.backend);
            return None;
        }
    };
    // Use the passed-in health_check_url instead of querying DB
    if let Some(ref backend_url) = health_check_url {
        if let Some(port) = server.port {
            let mut url = url::Url::parse(backend_url).ok()?;
            url.set_port(Some(port)).ok()?;
            return Some(url.to_string());
        }
        return Some(backend_url.to_string());
    }
    if let Some(port) = server.port {
        return Some(format!("http://localhost:{}/health", port));
    }
    None
}

// resolve_backend_url: db_conn → health_check_url: Option<&str>
pub fn resolve_backend_url(
    &self,
    server: &ModelConfig,
    health_check_url: Option<&str>,
) -> Option<String> {
    let backend = match self.backends.get(&server.backend) {
        Some(b) => b,
        None => {
            tracing::warn!("Backend '{}' not found when resolving backend URL", server.backend);
            return None;
        }
    };
    if let Some(ref health_url) = health_check_url {
        let mut url = url::Url::parse(health_url).ok()?;
        if let Some(port) = server.port {
            url.set_port(Some(port)).ok()?;
        }
        url.set_path("");
        url.set_query(None);
        url.set_fragment(None);
        return Some(url.to_string().trim_end_matches('/').to_string());
    }
    if let Some(port) = server.port {
        return Some(format!("http://localhost:{}", port));
    }
    None
}

// resolve_health_check: db_conn → health_check_url: Option<&str>
pub fn resolve_health_check(
    &self,
    server: &ModelConfig,
    health_check_url: Option<&str>,
) -> HealthCheck {
    let server_hc = server.health_check.as_ref();
    HealthCheck {
        url: server_hc
            .and_then(|h| h.url.clone())
            .or_else(|| self.resolve_health_url(server, health_check_url)),
        interval_ms: Some(
            server_hc
                .and_then(|h| h.interval_ms)
                .unwrap_or(self.supervisor.health_check_interval_ms),
        ),
        timeout_ms: Some(server_hc.and_then(|h| h.timeout_ms).unwrap_or(3000)),
    }
}

// resolve_backend_path: conn: &Connection → manager: &BackendManager
pub fn resolve_backend_path(
    &self,
    name: &str,
    model_variant: Option<&str>,
    manager: &crate::backends::BackendManager,
) -> Result<std::path::PathBuf> {
    // Same logic as before but use manager.get_active(), manager.list_versions(),
    // manager.get_by_version() instead of db::queries::*
    let gpu_variant: String = model_variant
        .map(String::from)
        .or_else(|| {
            self.backends.get(name).and_then(|b| b.gpu_variant.as_deref()).map(String::from)
        })
        .unwrap_or_else(|| "cpu".to_string());

    // Check for pinned version
    if let Some(pinned_version) = self.backends.get(name).and_then(|b| b.version.as_deref()) {
        if let Some(info) = manager.get_by_version(name, &gpu_variant, pinned_version)? {
            return Ok(info.path);
        }
        if let Some(versions) = manager.list_versions(name, None)? {
            for v in &versions {
                if v.version == pinned_version {
                    return Ok(v.path.clone());
                }
            }
        }
        anyhow::bail!(
            "Backend '{}' version '{}' not found. Run `tama backend install {}` first.",
            name, pinned_version, name
        );
    }

    // Try active installation for the resolved variant
    if let Some(info) = manager.get_active(name, &gpu_variant)? {
        return Ok(info.path);
    }

    // Try any active variant as fallback
    if let Some(versions) = manager.list_versions(name, None)? {
        for v in &versions {
            if let Some(info) = manager.get_active(name, &v.gpu_variant)? {
                return Ok(info.path);
            }
        }
    }

    // Final fallback to config path
    self.backends.get(name)
        .and_then(|b| b.path.as_deref())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!(
            "Backend '{}' has no installed path. Run `tama backend install {}` first.",
            name, name
        ))
}
```

2. Update call sites. For each caller that previously passed `db_conn: Option<&Connection>`:

**Pattern for `build_args`/`build_full_args`:**
```rust
// Before:
let args = config.build_full_args(server, backend, ctx, db_conn)?;

// After:
let manager = BackendManager::open(&config_dir)?;
let gpu_variant = server.gpu_variant.as_deref().unwrap_or("cpu");
let default_args = manager.get_default_args(&server.backend, gpu_variant);
let args = config.build_full_args(server, backend, ctx, &default_args)?;
```

**Pattern for `resolve_health_url`/`resolve_backend_url`/`resolve_health_check`:**
```rust
// Before:
let url = config.resolve_health_url(server, db_conn);

// After:
let manager = BackendManager::open(&config_dir)?;
let gpu_variant = server.gpu_variant.as_deref().unwrap_or("cpu");
let health_url = manager.get_health_check_url(&server.backend, gpu_variant);
let url = config.resolve_health_url(server, health_url.as_deref());
```

**Pattern for `resolve_backend_path`:**
```rust
// Before:
let path = config.resolve_backend_path(name, variant, &conn)?;

// After:
let manager = BackendManager::open(&config_dir)?;
let path = config.resolve_backend_path(name, variant, &manager)?;
```

**Specific call sites to update:**

- `crates/tama-core/src/proxy/lifecycle/mod.rs` line ~94: `build_full_args(server_config, backend_config, None, db_conn)`
- `crates/tama-core/src/proxy/lifecycle/mod.rs` lines ~70-84: `resolve_backend_path(name, ..., &db_conn)` — change to use `BackendManager`
- `crates/tama-core/src/proxy/state.rs` line ~76: `resolve_backend_url(server, db_conn.as_ref())`
- `crates/tama-cli/src/handlers/run.rs` line ~18: `build_full_args(server, backend, ctx_override, Some(&conn))`
- `crates/tama-cli/src/handlers/run.rs` line ~34: `resolve_health_check(server, Some(&conn))`
- `crates/tama-cli/src/handlers/service_cmd.rs` line ~22: `build_full_args(srv, backend, None, Some(&conn))`
- `crates/tama-cli/src/handlers/server/ls.rs` line ~43: `resolve_health_check(srv, Some(&conn))`
- `crates/tama-core/src/bench/runner.rs` line ~110: `build_full_args(server_config, backend_config, ctx_override, Some(&conn))`
- All test files in `config/resolve/tests/` — change `None` for `db_conn` to relevant test values

**Steps:**
- [ ] Update `build_args` and `build_full_args` signatures + bodies in `resolve/mod.rs`
- [ ] Update `resolve_health_url`, `resolve_backend_url`, `resolve_health_check` signatures + bodies
- [ ] Update `resolve_backend_path` to take `&BackendManager` instead of `&Connection`
- [ ] Update all call sites listed above
- [ ] Update all test fixtures in `config/resolve/tests/`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "refactor: replace db_conn params on Config::resolve_* and build_*"

**Acceptance criteria:**
- [ ] `build_args`/`build_full_args` accept `default_args: &[String]` instead of `db_conn`
- [ ] `resolve_health_url`/`resolve_backend_url`/`resolve_health_check` accept `health_check_url: Option<&str>` instead of `db_conn`
- [ ] `resolve_backend_path` accepts `&BackendManager` instead of `&Connection`
- [ ] All callers updated to open `BackendManager` and pre-resolve values
- [ ] All tests pass

---

### Task 5: Switch tama-web callers from raw db::queries to BackendManager

**Context:**
Now that `BackendManager` has all methods, switch the web API handlers from opening their own DB + calling `db::queries::*` to using `BackendManager`. The `build_backend_options` function in `info.rs` is replaced with `mgr.available_backends()`.

**Files:**
- Modify: `crates/tama-web/src/api/models/info.rs` (use `BackendManager::available_backends()`)
- Modify: `crates/tama-web/src/api/backends/manage.rs` (use `BackendManager::save_config()`)
- Modify: `crates/tama-web/src/api/backends/list.rs` (use `BackendManager::list_configs()`)
- Modify: `crates/tama-web/src/api/backends/install.rs` (use `BackendManager` for update check cleanup)
- Modify: `crates/tama-web/src/api/updates.rs` (use `BackendManager::list_versions()`)

**What to implement:**

1. In `crates/tama-web/src/api/models/info.rs`:
   Replace the `build_backend_options` function body:
   ```rust
   fn build_backend_options(
       _cfg: &tama_core::config::Config,
       config_dir: &std::path::Path,
   ) -> Vec<BackendOption> {
       let mgr = match tama_core::backends::BackendManager::open(config_dir) {
           Ok(m) => m,
           Err(_) => return Vec::new(),
       };
       mgr.available_backends().unwrap_or_default()
   }
   ```

2. In `crates/tama-web/src/api/backends/manage.rs`:
   Replace the `update_backend_default_args` handler body that opens DB and calls `upsert_backend_config`:
   ```rust
   let result: Result<(), anyhow::Error> = tokio::task::spawn_blocking(move || {
       let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
       mgr.save_config(&backend_name, &gpu_variant, &default_args, None)?;
       Ok(())
   })
   ```

3. In `crates/tama-web/src/api/backends/list.rs`:
   Replace both occurrences of `tama_core::db::open(&config_dir)` + `list_backend_configs` with:
   ```rust
   let backend_configs_map = tama_core::backends::BackendManager::open(&config_dir)
       .ok()
       .map(|mgr| mgr.list_configs().ok())
       .flatten()
       .map(|configs| configs.into_iter().map(|c| ((c.name, c.gpu_variant), c.default_args)).collect())
       .unwrap_or_default();
   ```

4. In `crates/tama-web/src/api/backends/install.rs`:
   Replace `tama_core::db::open(&config_dir)` + `delete_update_check` with `BackendManager::open(&config_dir)` — though note that `delete_update_check` is on `db::queries`, not on `BackendManager`. For now, keep using `db::open` directly for update_checks table operations. BackendManager doesn't cover update_checks.

5. In `crates/tama-web/src/api/updates.rs`:
   Replace `tama_core::db::open(&config_dir)` + `list_backend_versions` with:
   ```rust
   let mgr = tama_core::backends::BackendManager::open(&config_dir)?;
   let versions = mgr.list_versions(&name, gpu_variant.as_deref())?;
   ```

**Steps:**
- [ ] Update `build_backend_options` in `info.rs` to use `BackendManager`
- [ ] Update `update_backend_default_args` in `manage.rs` to use `BackendManager`
- [ ] Update `list_backends` and `check_backend_updates` in `list.rs`
- [ ] Update `updates.rs` to use `BackendManager`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-web --features ssr`
- [ ] Run `cargo test --package tama-web --features ssr`
- [ ] Commit with message: "refactor: switch tama-web to BackendManager"

**Acceptance criteria:**
- [ ] `info.rs` uses `mgr.available_backends()` instead of iterating `cfg.backends.keys()`
- [ ] `manage.rs` uses `mgr.save_config()` instead of `db::open()` + `db::queries::upsert_backend_config()`
- [ ] `list.rs` uses `mgr.list_configs()` instead of raw DB opens
- [ ] All 147 web tests pass

---

### Task 6: Switch CLI + proxy callers from BackendRegistry to BackendManager

**Context:**
`BackendRegistry` is now redundant — all its DB methods are on `BackendManager`. Switch CLI and proxy callers that use `BackendRegistry` for DB operations to use `BackendManager`. Install/update functions that need both DB access AND `reqwest::Client` take them as separate parameters.

**Files:**
This is a search-and-replace across many files. Use `grep -rn "BackendRegistry" --include="*.rs"` to find all call sites.

Key callers:
- `crates/tama-cli/src/commands/backend/*` (install, list, remove, switch, update)
- `crates/tama-web/src/api/backends/install.rs` (install endpoint uses registry)
- `crates/tama-web/src/api/backends/manage.rs` (update, remove_version, activate handlers)
- `crates/tama-core/src/proxy/lifecycle/mod.rs` (for TTS backend start)
- `crates/tama-core/src/updates/checker.rs` (update checker iterates backends)
- `crates/tama-core/src/db/backfill.rs` (startup backfill uses registry)

**What to implement:**

Pattern: Replace `BackendRegistry::open(config_dir)` with `BackendManager::open(config_dir)` and adapt method calls:
- `registry.list()` → `mgr.list_active()`
- `registry.get(name, variant)` → `mgr.get_active(name, variant)`
- `registry.add(info)` → `mgr.add_installation(&info)`
- `registry.remove(name, variant)` → `mgr.delete_all_versions(name, variant)`
- `registry.activate(name, variant, version)` → `mgr.activate(name, variant, version)`
- `registry.update_version(name, variant, ver, path, src)` → `mgr.update_version(name, variant, ver, path, src)`
- `registry.list_all_versions(name, variant)` → `mgr.list_versions(name, variant)`

For functions like `install_backend_with_progress` and `update_backend_with_progress` that take `&mut BackendRegistry`:
- Change signature to take `(&BackendManager, &reqwest::Client)` separately
- Update all callers

For `BackendRegistry.client` access:
- Callers that need HTTP client create their own or receive one as parameter
- The `make_client()` helper moves to a free function in `backends/` module or stays where needed

**Steps:**
- [ ] Search for all `BackendRegistry` usage: `grep -rn "BackendRegistry" --include="*.rs" crates/`
- [ ] Switch each caller from `BackendRegistry` to `BackendManager` using the patterns above
- [ ] Update `install_backend_with_progress` and `update_backend_with_progress` signatures
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "refactor: switch CLI and proxy from BackendRegistry to BackendManager"

**Acceptance criteria:**
- [ ] No CLI or web handler uses `BackendRegistry::open()` for DB operations
- [ ] Install/update functions accept `(&BackendManager, &reqwest::Client)`
- [ ] All tests pass

---

### Task 7: Deprecate and remove BackendRegistry

**Context:**
After Tasks 5-6, `BackendRegistry` has zero remaining callers. Mark it deprecated and remove it.

**Files:**
- Modify: `crates/tama-core/src/backends/registry/registry_ops.rs` (add `#[deprecated]`, remove if no callers)
- Modify: `crates/tama-core/src/backends/registry/mod.rs` (remove if no callers)
- Modify: `crates/tama-core/src/backends/mod.rs` (remove registry re-exports)

**What to implement:**

1. Verify zero remaining callers:
   ```bash
   grep -rn "BackendRegistry" --include="*.rs" crates/ | grep -v "registry_ops.rs" | grep -v "test"
   ```
   Should return nothing (or only doc references).

2. Add `#[deprecated(since = "1.55.0", note = "use BackendManager instead")]` to `BackendRegistry` struct.

3. If truly zero callers, delete the entire `crates/tama-core/src/backends/registry/` directory and remove `pub mod registry;` from `backends/mod.rs`.

**Steps:**
- [ ] Verify zero non-test callers of `BackendRegistry`
- [ ] Add `#[deprecated]` attribute (or delete if zero callers)
- [ ] Remove re-exports from `backends/mod.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --workspace`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "refactor: deprecate BackendRegistry in favor of BackendManager"

**Acceptance criteria:**
- [ ] `BackendRegistry` is deprecated or removed
- [ ] No code imports `BackendRegistry` (except possibly deprecated warnings in tests)
- [ ] All tests pass