# Updates Center (/updates) Plan

**Goal:** Add a unified Updates Center page that shows update availability for both backends and models, allows triggering checks, and initiates re-pulls/updates.

**Architecture:** Centralized update checking with database-backed state. A background checker runs on configurable intervals and on-demand. All update state is stored in a new `update_checks` table. The frontend shows a unified view of backends and models with their current/latest versions.

**Tech Stack:** Rust (koji-core + koji-web), SQLite, Leptos frontend, Axum API

---

## Task 1: Database Migration (v6) — Create `update_checks` Table

**Context:**
We need a new table to persist update check results for both backends and models. This allows the frontend to display cached results without waiting for network requests on every page load. The table tracks version info, update availability status, and any error details.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs`

**What to implement:**
Add migration v6 that creates the `update_checks` table and updates `LATEST_VERSION` to 6.

```rust
(
    6,
    r#"
        CREATE TABLE IF NOT EXISTS update_checks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            item_type TEXT NOT NULL,           -- 'backend' or 'model'
            item_id TEXT NOT NULL,             -- backend name or model config key
            current_version TEXT,              -- installed version/commit SHA
            latest_version TEXT,               -- remote version/commit SHA
            update_available INTEGER NOT NULL DEFAULT 0,
            status TEXT NOT NULL DEFAULT 'unknown',
            error_message TEXT,
            details_json TEXT,                 -- JSON blob (per-file changes for models)
            checked_at INTEGER NOT NULL,        -- unix timestamp
            UNIQUE(item_type, item_id)
        );
        CREATE INDEX IF NOT EXISTS idx_update_checks_type ON update_checks(item_type);
    "#,
),
```

Then update the `LATEST_VERSION` constant to 6.

**Steps:**
- [ ] Add migration v6 to `migrations.rs` with the SQL above
- [ ] Update `LATEST_VERSION` from 5 to 6
- [ ] Run migration test: `cargo test --package koji-core -- migrations`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-core`
- [ ] Commit with message: `feat(db): add migration v6 for update_checks table`

**Acceptance criteria:**
- [ ] `LATEST_VERSION` is 6
- [ ] Migration SQL creates table with all specified columns
- [ ] Index on `item_type` is created
- [ ] Existing migrations (1-5) still apply correctly

---

## Task 2: DB Query Functions — `update_check_queries.rs`

**Context:**
We need CRUD operations for the `update_checks` table. These functions are synchronous (blocking SQLite calls) and will be called from the background checker. We follow the existing patterns in `backend_queries.rs` and `model_queries.rs`.

**Files:**
- Create: `crates/koji-core/src/db/queries/update_check_queries.rs`
- Modify: `crates/koji-core/src/db/queries/types.rs` (add `UpdateCheckRecord`)
- Modify: `crates/koji-core/src/db/queries/mod.rs` (re-export new types)

**What to implement:**

### In `types.rs`, add:
```rust
/// A stored update check record for a backend or model.
#[derive(Debug, Clone)]
pub struct UpdateCheckRecord {
    pub item_type: String,         // "backend" or "model"
    pub item_id: String,           // backend name or model config key
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,            // "unknown", "up_to_date", "update_available", "error"
    pub error_message: Option<String>,
    pub details_json: Option<String>,  // JSON blob for model file changes
    pub checked_at: i64,           // unix timestamp
}
```

### In `update_check_queries.rs`, implement:
- `upsert_update_check(conn, item_type, item_id, current_version, latest_version, update_available, status, error_message, details_json)` — Insert or replace
- `get_all_update_checks(conn) -> Vec<UpdateCheckRecord>`
- `get_update_check(conn, item_type, item_id) -> Option<UpdateCheckRecord>`
- `delete_update_check(conn, item_type, item_id)`
- `get_oldest_check_time(conn) -> Option<i64>` — For scheduling next check

```rust
use anyhow::Result;
use rusqlite::Connection;

use super::types::UpdateCheckRecord;

pub fn upsert_update_check(
    conn: &Connection,
    item_type: &str,
    item_id: &str,
    current_version: Option<&str>,
    latest_version: Option<&str>,
    update_available: bool,
    status: &str,
    error_message: Option<&str>,
    details_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO update_checks (item_type, item_id, current_version, latest_version, update_available, status, error_message, details_json, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(item_type, item_id) DO UPDATE SET
             current_version = excluded.current_version,
             latest_version = excluded.latest_version,
             update_available = excluded.update_available,
             status = excluded.status,
             error_message = excluded.error_message,
             details_json = excluded.details_json,
             checked_at = excluded.checked_at",
        (
            item_type,
            item_id,
            current_version,
            latest_version,
            update_available as i32,
            status,
            error_message,
            details_json,
            chrono::Utc::now().timestamp(),
        ),
    )?;
    Ok(())
}

pub fn get_all_update_checks(conn: &Connection) -> Result<Vec<UpdateCheckRecord>> {
    let mut stmt = conn.prepare(
        "SELECT item_type, item_id, current_version, latest_version, update_available, status, error_message, details_json, checked_at
         FROM update_checks ORDER BY item_type, item_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(UpdateCheckRecord {
            item_type: row.get(0)?,
            item_id: row.get(1)?,
            current_version: row.get(2)?,
            latest_version: row.get(3)?,
            update_available: row.get::<_, i32>(4)? != 0,
            status: row.get(5)?,
            error_message: row.get(6)?,
            details_json: row.get(7)?,
            checked_at: row.get(8)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

pub fn get_update_check(conn: &Connection, item_type: &str, item_id: &str) -> Result<Option<UpdateCheckRecord>> {
    let mut stmt = conn.prepare(
        "SELECT item_type, item_id, current_version, latest_version, update_available, status, error_message, details_json, checked_at
         FROM update_checks WHERE item_type = ?1 AND item_id = ?2",
    )?;
    let mut rows = stmt.query_map((item_type, item_id), |row| {
        Ok(UpdateCheckRecord {
            item_type: row.get(0)?,
            item_id: row.get(1)?,
            current_version: row.get(2)?,
            latest_version: row.get(3)?,
            update_available: row.get::<_, i32>(4)? != 0,
            status: row.get(5)?,
            error_message: row.get(6)?,
            details_json: row.get(7)?,
            checked_at: row.get(8)?,
        })
    })?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

pub fn delete_update_check(conn: &Connection, item_type: &str, item_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM update_checks WHERE item_type = ?1 AND item_id = ?2",
        (item_type, item_id),
    )?;
    Ok(())
}

pub fn get_oldest_check_time(conn: &Connection) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT MIN(checked_at) FROM update_checks",
    )?;
    let mut rows = stmt.query_map([], |row| row.get::<_, Option<i64>>(0))?;
    match rows.next() {
        Some(row) => Ok(row?),
        None => Ok(None),
    }
}
```

**Steps:**
- [ ] Add `UpdateCheckRecord` to `types.rs`
- [ ] Create `update_check_queries.rs` with all functions
- [ ] Add re-export in `mod.rs`
- [ ] Add tests in `crates/koji-core/src/db/queries/tests.rs`
- [ ] Run `cargo test --package koji-core -- queries`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-core`
- [ ] Commit with message: `feat(db): add update_check_queries for CRUD operations`

**Acceptance criteria:**
- [ ] All 5 functions compile and work correctly
- [ ] `upsert_update_check` uses ON CONFLICT for upsert behavior
- [ ] Tests verify basic CRUD operations
- [ ] Re-exported from `queries/mod.rs`

---


## Task 3: Configuration — Add `update_check_interval` to General

**Context:**
We need a configurable interval for background update checks. Users should be able to set how often (in hours) koji automatically checks for updates. The default is 12 hours.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs` (add field to `General`)
- Modify: `crates/koji-core/src/config/defaults.rs` (add default)
- Modify: `crates/koji-web/src/types/config.rs` (mirror type)

**What to implement:**

### In `crates/koji-core/src/config/types.rs`, add to `General`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "crate::config::defaults::default_update_check_interval")]
    pub update_check_interval: u32,
}
```

### In `crates/koji-core/src/config/defaults.rs`, add:
```rust
pub fn default_update_check_interval() -> u32 {
    12
}
```

### Update `General::default()` to include the new field:
```rust
impl Default for General {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            models_dir: None,
            logs_dir: None,
            hf_token: None,
            update_check_interval: default_update_check_interval(),
        }
    }
}
```

### In `crates/koji-web/src/types/config.rs`, add to `General`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "default_update_check_interval")]
    pub update_check_interval: u32,
}
```

Also add a `default_update_check_interval()` function in that file and update the conversion between core and mirror types.

**Steps:**
- [ ] Add `update_check_interval` field to `General` in koji-core
- [ ] Add `default_update_check_interval()` function in defaults.rs
- [ ] Update `General::default()` implementation
- [ ] Add field to mirror `General` in koji-web
- [ ] Add conversion between core and mirror types
- [ ] Add test for deserialization
- [ ] Run `cargo test --package koji-core -- config`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: `feat(config): add update_check_interval to General config`

**Acceptance criteria:**
- [ ] Field exists with default value of 12
- [ ] Mirror type in koji-web has the same field
- [ ] Config round-trip serialization works
- [ ] Default value applied when field is missing from config file

---

## Task 4: Background Update Checker Module

**Context:**
We need a background task that checks for updates for all backends and models. The checker runs on a configurable interval and also on-demand. It uses a phased approach to remain `Send`-safe: sync DB reads → async network calls → sync DB writes.

**Files:**
- Create: `crates/koji-core/src/updates/mod.rs` (module entry)
- Create: `crates/koji-core/src/updates/checker.rs` (main checker logic)
- Modify: `crates/koji-core/src/lib.rs` (export new module)

**What to implement:**

### Module structure:
```rust
// crates/koji-core/src/updates/mod.rs
pub mod checker;

pub use checker::UpdateChecker;
```

### In `checker.rs`:

```rust
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::config::Config;
use crate::db;
use crate::db::queries::{
    get_all_update_checks, upsert_update_check, delete_update_check,
    get_oldest_check_time, get_active_backend, get_model_pull,
};
use crate::backends::{BackendRegistry, BackendType, check_latest_version};
use crate::models::pull::list_gguf_files;

/// Shared state for the update checker. Uses Arc<Mutex<()>> to prevent concurrent check runs.
#[derive(Clone)]
pub struct UpdateChecker {
    /// Mutex to prevent concurrent check runs. Locking the guard serializes checks.
    lock: Arc<Mutex<()>>,
}

impl UpdateChecker {
    pub fn new() -> Self {
        Self {
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Run a full update check for all backends and models.
    /// Returns immediately if another check is already in progress.
    pub async fn run_check(&self, config_dir: &std::path::Path) -> anyhow::Result<()> {
        // Try to acquire the lock
        let _guard = match self.lock.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::info!("Update check already in progress, skipping");
                return Ok(());
            }
        };

        tracing::info!("Starting update check for all items");

        // Phase 1: Sync DB - fetch all items to check
        let (backends, models) = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<(Vec<(String, BackendType)>, Vec<(String, Option<String>)>)> {
                // Get backend info from registry
                let registry = BackendRegistry::open(&config_dir)?;
                let backends: Vec<(String, BackendType)> = registry
                    .list()
                    .unwrap_or_default()
                    .iter()
                    .map(|b| (b.name.clone(), b.backend_type.clone()))
                    .collect();

                // Get model keys and repo_ids from config
                let config = Config::load_from(&config_dir)?;
                let models: Vec<(String, Option<String>)> = config
                    .models
                    .iter()
                    .map(|(k, v)| (k.clone(), v.model.clone()))
                    .collect();

                Ok((backends, models))
            }
        })
        .await??;

        // Phase 2: Async network - check each backend
        for (backend_name, backend_type) in &backends {
            if let Err(e) = self.check_backend(config_dir, backend_name, backend_type).await {
                tracing::warn!("Failed to check backend {}: {}", backend_name, e);
            }
        }

        // Phase 2: Async network - check each model
        for (model_id, repo_id) in &models {
            if let Err(e) = self.check_model(config_dir, model_id, repo_id.as_deref()).await {
                tracing::warn!("Failed to check model {}: {}", model_id, e);
            }
        }

        tracing::info!("Update check complete");
        Ok(())
    }

    /// Check a single backend for updates.
    async fn check_backend(
        &self,
        config_dir: &std::path::Path,
        backend_name: &str,
        backend_type: &BackendType,
    ) -> anyhow::Result<()> {
        // Sync: Get current version from DB
        let current_version = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let backend_name = backend_name.to_string();
            move || -> anyhow::Result<Option<String>> {
                let open = db::open(&config_dir)?;
                let record = get_active_backend(&open.conn, &backend_name)?;
                Ok(record.map(|r| r.version))
            }
        })
        .await??;

        // Async: Check latest version from network
        let latest_version = match backend_type {
            BackendType::LlamaCpp | BackendType::IkLlama => {
                match check_latest_version(backend_type).await {
                    Ok(v) => Some(v),
                    Err(e) => {
                        self.save_check_result(
                            config_dir,
                            "backend",
                            backend_name,
                            current_version.as_deref(),
                            None,
                            false,
                            "error",
                            Some(&e.to_string()),
                            None,
                        ).await?;
                        return Ok(());
                    }
                }
            }
            BackendType::Custom => None,
        };

        let update_available = latest_version.as_ref()
            .map(|v| current_version.as_ref().map(|c| v != c).unwrap_or(true))
            .unwrap_or(false);

        let status = if latest_version.is_none() && current_version.is_none() {
            "unknown"
        } else if update_available {
            "update_available"
        } else {
            "up_to_date"
        };

        self.save_check_result(
            config_dir,
            "backend",
            backend_name,
            current_version.as_deref(),
            latest_version.as_deref(),
            update_available,
            status,
            None,
            None,
        )
        .await
    }

    /// Check a single model for updates.
    /// Uses 3-phase approach: sync DB read, async HF network, sync DB write.
    async fn check_model(
        &self,
        config_dir: &std::path::Path,
        model_id: &str,
        repo_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let repo_id = match repo_id.filter(|s| !s.is_empty()) {
            Some(id) => id,
            None => {
                self.save_check_result(
                    config_dir,
                    "model",
                    model_id,
                    None,
                    None,
                    false,
                    "unknown",
                    Some("Model has no source repo configured"),
                    None,
                ).await?;
                return Ok(());
            }
        };

        // Sync: Get current commit from DB
        let current_version = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let repo_id = repo_id.to_string();
            move || -> anyhow::Result<Option<String>> {
                let open = db::open(&config_dir)?;
                let pull = get_model_pull(&open.conn, &repo_id)?;
                Ok(pull.map(|p| p.commit_sha))
            }
        })
        .await??;

        // Async: List remote files
        let latest_listing = match list_gguf_files(repo_id).await {
            Ok(l) => l,
            Err(e) => {
                self.save_check_result(
                    config_dir,
                    "model",
                    model_id,
                    current_version.as_deref(),
                    None,
                    false,
                    "error",
                    Some(&e.to_string()),
                    None,
                ).await?;
                return Ok(());
            }
        };

        // Pure logic - determine update availability
        let update_available = current_version
            .map(|c| c != latest_listing.commit_sha)
            .unwrap_or(true);

        let details_json = serde_json::json!({
            "repo_id": latest_listing.repo_id,
            "commit_sha": latest_listing.commit_sha,
            "file_count": latest_listing.files.len(),
            "files": latest_listing.files.iter().map(|f| f.filename.clone()).collect::<Vec<_>>(),
        }).to_string();

        let status = if update_available { "update_available" } else { "up_to_date" };

        self.save_check_result(
            config_dir,
            "model",
            model_id,
            current_version.as_deref(),
            Some(&latest_listing.commit_sha),
            update_available,
            status,
            None,
            Some(&details_json),
        )
        .await
    }

    /// Save check result to DB.
    async fn save_check_result(
        &self,
        config_dir: &std::path::Path,
        item_type: &str,
        item_id: &str,
        current_version: Option<&str>,
        latest_version: Option<&str>,
        update_available: bool,
        status: &str,
        error_message: Option<&str>,
        details_json: Option<&str>,
    ) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            let item_type = item_type.to_string();
            let item_id = item_id.to_string();
            let current_version = current_version.map(String::from);
            let latest_version = latest_version.map(String::from);
            let error_message = error_message.map(String::from);
            let details_json = details_json.map(String::from);
            move || -> anyhow::Result<()> {
                let open = db::open(&config_dir)?;
                upsert_update_check(
                    &open.conn,
                    &item_type,
                    &item_id,
                    current_version.as_deref(),
                    latest_version.as_deref(),
                    update_available,
                    status,
                    error_message.as_deref(),
                    details_json.as_deref(),
                    now,
                )?;
                Ok(())
            }
        })
        .await??;
        Ok(())
    }

    /// Get cached update check results.
    pub async fn get_results(&self, config_dir: &std::path::Path) -> anyhow::Result<Vec<crate::db::queries::UpdateCheckRecord>> {
        tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<Vec<crate::db::queries::UpdateCheckRecord>> {
                let open = db::open(&config_dir)?;
                get_all_update_checks(&open.conn)
            }
        })
        .await??
    }

    /// Check if enough time has passed since last check (based on interval).
    pub async fn should_check(&self, config_dir: &std::path::Path) -> anyhow::Result<bool> {
        let config = Config::load_from(config_dir)?;
        let interval_hours = config.general.update_check_interval as i64;
        let interval_secs = interval_hours * 3600;

        let oldest = tokio::task::spawn_blocking({
            let config_dir = config_dir.to_path_buf();
            move || -> anyhow::Result<Option<i64>> {
                let open = db::open(&config_dir)?;
                get_oldest_check_time(&open.conn)
            }
        })
        .await??;

        let now = chrono::Utc::now().timestamp();
        match oldest {
            Some(ts) => Ok(now - ts >= interval_secs),
            None => Ok(true),
        }
    }
}

impl Default for UpdateChecker {
    fn default() -> Self {
        Self::new()
    }
}
```

Also update `upsert_update_check` in Task 2 to accept `checked_at: i64` as a parameter instead of hardcoding it.

**Steps:**
- [ ] Create `crates/koji-core/src/updates/mod.rs`
- [ ] Create `crates/koji-core/src/updates/checker.rs` with full implementation
- [ ] Export from `crates/koji-core/src/lib.rs`
- [ ] Update `upsert_update_check` to accept `checked_at` parameter
- [ ] Add tests in `crates/koji-core/src/updates/tests.rs`
- [ ] Run `cargo test --package koji-core -- updates`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package koji-core`
- [ ] Commit with message: `feat(updates): add background update checker module`

**Acceptance criteria:**
- [ ] UpdateChecker runs without blocking the async runtime
- [ ] Concurrent check runs are prevented via mutex
- [ ] All backends and models are checked on `run_check()`
- [ ] Results are persisted to DB
- [ ] Tests verify concurrent run prevention
## Task 5: API Endpoints for Updates

**Context:**
We need REST API endpoints to:
1. Get cached update check results
2. Trigger a full re-check
3. Check a single item
4. Trigger backend update
5. Trigger model re-pull (resolve model ID to repo_id, then trigger re-pull)

These endpoints follow the same patterns as existing API handlers in `koji-web/src/api.rs`.

**Files:**
- Create: `crates/koji-web/src/api/updates.rs` (new endpoints)
- Modify: `crates/koji-web/src/server.rs` (add routes)
- Modify: `crates/koji-web/src/lib.rs` (export)

**What to implement:**

### DTOs in `updates.rs`:

```rust
use axum::{extract::Path, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::server::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckDto {
    pub item_type: String,           // "backend" or "model"
    pub item_id: String,             // backend name or model config key
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub status: String,
    pub error_message: Option<String>,
    pub details_json: Option<serde_json::Value>,
    pub checked_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdatesListResponse {
    pub backends: Vec<UpdateCheckDto>,
    pub models: Vec<UpdateCheckDto>,
}

#[derive(Debug, Serialize)]
pub struct CheckResponse {
    pub triggered: bool,
    pub message: String,
}
```

### Handlers:

1. `GET /api/updates` - Returns cached results from DB
```rust
pub async fn get_updates(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "config_path not configured" }))).into_response(),
    };

    let checker = koji_core::updates::UpdateChecker::new();
    match checker.get_results(&config_dir).await {
        Ok(records) => {
            let mut backends = Vec::new();
            let mut models = Vec::new();
            for r in records {
                let dto = UpdateCheckDto {
                    item_type: r.item_type,
                    item_id: r.item_id,
                    current_version: r.current_version,
                    latest_version: r.latest_version,
                    update_available: r.update_available,
                    status: r.status,
                    error_message: r.error_message,
                    details_json: r.details_json.and_then(|j| serde_json::from_str(&j).ok()),
                    checked_at: r.checked_at,
                };
                if dto.item_type == "backend" {
                    backends.push(dto);
                } else {
                    models.push(dto);
                }
            }
            Json(UpdatesListResponse { backends, models }).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}
```

2. `POST /api/updates/check` - Trigger full re-check
```rust
pub async fn trigger_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "config_path not configured" }))).into_response(),
    };

    let checker = koji_core::updates::UpdateChecker::new();
    // Run in background, return immediately
    tokio::spawn(async move {
        if let Err(e) = checker.run_check(&config_dir).await {
            tracing::error!("Background update check failed: {}", e);
        }
    });

    Json(CheckResponse { triggered: true, message: "Update check started".to_string() }).into_response()
}
```

3. `POST /api/updates/check/:item_type/:item_id` - Check single item
```rust
pub async fn check_single(
    State(state): State<Arc<AppState>>,
    Path((item_type, item_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "config_path not configured" }))).into_response(),
    };

    let checker = koji_core::updates::UpdateChecker::new();
    let result = match item_type.as_str() {
        "backend" => checker.check_backend(&config_dir, &item_id).await,
        "model" => checker.check_model(&config_dir, &item_id).await,
        _ => return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "Invalid item_type" }))).into_response(),
    };

    match result {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))).into_response(),
    }
}
```

**IMPORTANT: Routes must be placed in `backend_routes` sub-router for CSRF protection (same as other POST endpoints).**

4. `POST /api/updates/apply/backend/:name` - Trigger backend update
```rust
/// Proxy to the existing backend update endpoint.
pub async fn apply_backend_update(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "config_path not configured" }))).into_response(),
    };

    // Load backend info from DB
    let (backend_type, current_version) = tokio::task::spawn_blocking({
        let config_dir = config_dir.clone();
        move || -> anyhow::Result<(Option<BackendType>, Option<String>)> {
            let open = db::open(&config_dir)?;
            let record = get_active_backend(&open.conn, &name)?;
            Ok(record.map(|r| {
                let bt = match r.backend_type.as_str() {
                    "llama_cpp" => BackendType::LlamaCpp,
                    "ik_llama" => BackendType::IkLlama,
                    _ => BackendType::Custom,
                };
                (Some(bt), Some(r.version))
            }).unwrap_or((None, None)))
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))))??;

    let (Some(backend_type), Some(_version)) = (backend_type, current_version) else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "Backend not found" }))).into_response();
    };

    let jobs = match &state.jobs {
        Some(j) => j.clone(),
        None => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "job manager not configured" }))).into_response(),
    };

    let job = match jobs.submit(crate::jobs::JobKind::Update, Some(backend_type.clone())).await {
        Ok(j) => j,
        Err(crate::jobs::JobError::AlreadyRunning(existing_id)) => {
            return (StatusCode::CONFLICT, Json(serde_json::json!({ "error": "another backend job is already running", "job_id": existing_id }))).into_response();
        }
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "failed to create job" }))).into_response(),
    };

    let latest_version = match check_latest_version(&backend_type).await {
        Ok(v) => v,
        Err(e) => return (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": format!("Failed to check latest version: {}", e) }))).into_response(),
    };

    let jobs_clone = jobs.clone();
    let job_clone = job.clone();
    let name_clone = name.clone();
    tokio::spawn(async move {
        let config_dir = koji_core::config::Config::base_dir().unwrap();
        let mut registry = BackendRegistry::open(&config_dir).unwrap();
        let backend_info = registry.get(&name_clone).unwrap().unwrap();
        
        let options = InstallOptions {
            backend_type: backend_type.clone(),
            source: backend_info.source.clone().unwrap_or_else(|| BackendSource::SourceCode {
                version: "main".to_string(),
                git_url: "https://github.com/ggml-org/llama.cpp.git".to_string(),
                commit: None,
            }),
            target_dir: backend_info.path.parent().unwrap().to_path_buf(),
            gpu_type: backend_info.gpu_type,
            allow_overwrite: true,
        };

        match update_backend_with_progress(&mut registry, &name_clone, options, latest_version, None).await {
            Ok(_) => { let _ = jobs_clone.finish(&job_clone, crate::jobs::JobStatus::Succeeded, None).await; }
            Err(e) => { let _ = jobs_clone.finish(&job_clone, crate::jobs::JobStatus::Failed, Some(e.to_string())).await; }
        }
    });

    Json(serde_json::json!({ "job_id": job.id.to_string(), "kind": "update" })).into_response()
}
```

5. `POST /api/updates/apply/model/:id` - Trigger model re-pull
```rust
pub async fn apply_model_update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config_dir = match state.config_path.as_ref().and_then(|p| p.parent()) {
        Some(d) => d.to_path_buf(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "config_path not configured" }))).into_response(),
    };

    let (repo_id, _models_dir) = tokio::task::spawn_blocking({
        let config_dir = config_dir.clone();
        move || -> anyhow::Result<(String, std::path::PathBuf)> {
            let config = Config::load_from(&config_dir)?;
            let model = config.models.get(&id).ok_or_else(|| anyhow::anyhow!("Model not found"))?;
            let repo_id = model.model.clone().ok_or_else(|| anyhow::anyhow!("Model has no source"))?;
            let models_dir = config.models_dir()?;
            Ok((repo_id, models_dir))
        }
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))))??;

    match koji_core::models::pull::list_gguf_files(&repo_id).await {
        Ok(listing) => {
            tokio::task::spawn_blocking({
                let config_dir = config_dir.clone();
                let repo_id = repo_id.clone();
                let commit_sha = listing.commit_sha.clone();
                move || -> anyhow::Result<()> {
                    let open = db::open(&config_dir)?;
                    upsert_model_pull(&open.conn, &repo_id, &commit_sha)?;
                    Ok(())
                }
            })
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() }))))??;
            
            Json(serde_json::json!({ "ok": true, "repo_id": repo_id, "commit_sha": listing.commit_sha })).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, Json(serde_json::json!({ "error": format!("Failed to fetch updates: {}", e) }))).into_response(),
    }
}
```

### Route registration in `server.rs`:

**IMPORTANT:** All POST routes must be added to `backend_routes` sub-router for CSRF protection:
```rust
// In backend_routes (has enforce_same_origin middleware):
.route("/api/updates/check", post(api::updates::trigger_check))
.route("/api/updates/check/:item_type/:item_id", post(api::updates::check_single))
.route("/api/updates/apply/backend/:name", post(api::updates::apply_backend_update))
.route("/api/updates/apply/model/:id", post(api::updates::apply_model_update))

// GET can stay on main router:
.route("/api/updates", get(api::updates::get_updates))
```

**Steps:**
- [ ] Create `crates/koji-web/src/api/updates.rs` with all endpoints
- [ ] Add DTOs for request/response
- [ ] Implement `get_updates` (GET /api/updates)
- [ ] Implement `trigger_check` (POST /api/updates/check)
- [ ] Implement `check_single` (POST /api/updates/check/:item_type/:item_id)
- [ ] Implement `apply_backend_update` (POST /api/updates/apply/backend/:name)
- [ ] Implement `apply_model_update` (POST /api/updates/apply/model/:id)
- [ ] Add routes to `build_router()` in server.rs
- [ ] Run `cargo build --package koji-web`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat(api): add updates endpoints for check and apply`

**Acceptance criteria:**
- [ ] All 5 endpoints compile and return correct JSON
- [ ] GET returns cached results grouped by type
- [ ] POST check triggers background check
- [ ] POST single item checks one item
- [ ] Apply endpoints redirect to appropriate handlers

---

## Task 6: Frontend Updates Page (Leptos)

**Context:**
The frontend needs a new `/updates` page showing:
1. A "Check Now" button to trigger manual re-check
2. Backend section with version info and update buttons
3. Model section with version info and re-pull buttons
4. Status badges showing update availability

The page follows the same patterns as other pages like `backends.rs`.

**Files:**
- Create: `crates/koji-web/src/pages/updates.rs`
- Modify: `crates/koji-web/src/pages/mod.rs` (export new page)
- Modify: `crates/koji-web/src/components/sidebar.rs` (add Updates link with badge)

**What to implement:**

### Page structure:

```rust
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
struct UpdateCheckDto {
    item_type: String,
    item_id: String,
    current_version: Option<String>,
    latest_version: Option<String>,
    update_available: bool,
    status: String,
    error_message: Option<String>,
    checked_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct UpdatesListResponse {
    backends: Vec<UpdateCheckDto>,
    models: Vec<UpdateCheckDto>,
}

#[component]
pub fn Updates() -> impl IntoView {
    let updates = RwSignal::new(UpdatesListResponse {
        backends: vec![],
        models: vec![],
    });
    let checking = RwSignal::new(false);
    let last_checked = RwSignal::new(Option::<i64>::None);
    let error = RwSignal::new(Option::<String>::None);

    // Fetch on mount
    Effect::new(move |_| {
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::get("/api/updates")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    if let Ok(data) = resp.json::<UpdatesListResponse>().await {
                        updates.set(data.clone());
                        // Get last checked time from any record
                        let all_items: Vec<_> = data.backends.iter()
                            .chain(data.models.iter())
                            .collect();
                        last_checked.set(
                            all_items.iter()
                                .map(|r| r.checked_at)
                                .max()
                        );
                    }
                }
                _ => error.set(Some("Failed to load updates".to_string())),
            }
        });
    });

    let on_check_now = move |_| {
        checking.set(true);
        error.set(None);
        wasm_bindgen_futures::spawn_local(async move {
            match gloo_net::http::Request::post("/api/updates/check")
                .send()
                .await
            {
                Ok(resp) if resp.ok() => {
                    // Refresh list after a delay
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    if let Ok(resp2) = gloo_net::http::Request::get("/api/updates")
                        .send()
                        .await
                    {
                        if let Ok(data) = resp2.json::<UpdatesListResponse>().await {
                            updates.set(data);
                        }
                    }
                }
                _ => error.set(Some("Failed to trigger check".to_string())),
            }
            checking.set(false);
        });
    };

    let on_update_backend = move |name: String| {
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/backends/{}/update", name);
            let _ = gloo_net::http::Request::post(&url).send().await;
        });
    };

    let on_refresh_model = move |id: String| {
        wasm_bindgen_futures::spawn_local(async move {
            let url = format!("/api/models/{}/refresh", id);
            let _ = gloo_net::http::Request::post(&url).send().await;
        });
    };

    view! {
        <div class="page updates-page">
            <h1>"Updates Center"</h1>

            <div class="updates-header">
                <button
                    class="btn btn-primary"
                    disabled=move || checking.get()
                    on:click=on_check_now
                >
                    {move || if checking.get() { "Checking..." } else { "Check Now" }}
                </button>
                {move || last_checked.get().map(|ts| {
                    let date = chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    view! { <span class="last-checked">"Last checked: " {date}</span> }
                })}
            </div>

            {move || error.get().map(|e| view! {
                <div class="error-banner">{e}</div>
            })}

            <section class="updates-section">
                <h2>"Backends"</h2>
                <div class="updates-list">
                    {move || updates.get().backends.iter().map(|b| view! {
                        <div class="update-item" class:update-available=b.update_available>
                            <div class="update-item__info">
                                <span class="update-item__name">{b.item_id.clone()}</span>
                                <span class="update-item__version">
                                    {b.current_version.clone().unwrap_or_else(|| "—".to_string())}
                                </span>
                                {if b.update_available {
                                    view! {
                                        <span class="update-badge">
                                            " → " {b.latest_version.clone().unwrap_or_default()}
                                        </span>
                                    }
                                } else {
                                    view! { <span class="up-to-date-badge">"✓ Up to date"</span> }
                                }}
                            </div>
                            <div class="update-item__actions">
                                {if b.update_available {
                                    view! {
                                        <button class="btn btn-secondary"
                                            on:click=move |_| on_update_backend(b.item_id.clone())>
                                            "Update"
                                        </button>
                                    }
                                }}
                                <button class="btn btn-ghost"
                                    on:click=move |_| {
                                        wasm_bindgen_futures::spawn_local(async move {
                                            let url = format!("/api/updates/check/backend/{}", b.item_id);
                                            let _ = gloo_net::http::Request::post(&url).send().await;
                                        });
                                    }>
                                    "Refresh"
                                </button>
                            </div>
                        </div>
                    }).collect::<Vec<_>>()}
                </div>
            </section>

            <section class="updates-section">
                <h2>"Models"</h2>
                <div class="updates-list">
                    {move || updates.get().models.iter().map(|m| view! {
                        <div class="update-item" class:update-available=m.update_available>
                            <div class="update-item__info">
                                <span class="update-item__name">{m.item_id.clone()}</span>
                                <span class="update-item__version">
                                    {m.current_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("—").to_string()}
                                </span>
                                {if m.update_available {
                                    view! {
                                        <span class="update-badge">
                                            " → " {m.latest_version.as_ref().map(|v| &v[..8.min(v.len())]).unwrap_or("").to_string()}
                                        </span>
                                    }
                                } else {
                                    view! { <span class="up-to-date-badge">"✓ Up to date"</span> }
                                }}
                            </div>
                            <div class="update-item__actions">
                                {if m.update_available {
                                    view! {
                                        <button class="btn btn-secondary"
                                            on:click=move |_| on_refresh_model(m.item_id.clone())>
                                            "Re-pull"
                                        </button>
                                    }
                                }}
                                <a href=format!("/models/{}", m.item_id) class="btn btn-ghost">
                                    "Edit"
                                </a>
                            </div>
                        </div>
                    }).collect::<Vec<_>>()}
                </div>
            </section>
        </div>
    }
}
```

### Sidebar badge (add to `sidebar.rs`):

```rust
// Add to sidebar imports:
// No chrono - use JavaScript Date via web_sys

// Add to Sidebar component:
let update_badge_visible = RwSignal::new(false);

// Check for updates on mount (separate from self-update check)
Effect::new(move |_| {
    wasm_bindgen_futures::spawn_local(async move {
        if let Ok(resp) = gloo_net::http::Request::get("/api/updates")
            .send()
            .await
        {
            if let Ok(data) = resp.json::<serde_json::Value>().await {
                let has_updates = data.get("backends")
                    .and_then(|b| b.as_array())
                    .map(|arr| arr.iter().any(|b| b.get("update_available").and_then(|u| u.as_bool()).unwrap_or(false)))
                    .unwrap_or(false)
                    || data.get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| arr.iter().any(|m| m.get("update_available").and_then(|u| u.as_bool()).unwrap_or(false)))
                    .unwrap_or(false);
                update_badge_visible.set(has_updates);
            }
        }
    });
});

// Add to sidebar nav:
<A href="/updates" attr:class="sidebar-item" attr:data-tooltip="Updates" on:click=move |_| mobile_open.set(false)>
    <span class="sidebar-item__icon">"🔄"</span>
    <span class="sidebar-item__text">"Updates"</span>
    {move || update_badge_visible.get().then(|| view! {
        <span class="sidebar-badge">"!"</span>
    })}
</A>
```

**Route registration in `lib.rs`:**

Add the Updates page to the Leptos router:
```rust
// In crates/koji-web/src/lib.rs, add to Routes:
<Route path=path!("/updates") view=pages::updates::Updates />
```

**Steps:**
- [ ] Create `crates/koji-web/src/pages/updates.rs` with the Updates page component
- [ ] Export from `pages/mod.rs`
- [ ] Add Updates link to sidebar in `sidebar.rs`
- [ ] Add badge logic to show when updates are available
- [ ] Add `<Route path=path!("/updates") .../>` to `lib.rs` router
- [ ] Add CSS for updates page styles
- [ ] Run `cargo build --package koji-web`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat(frontend): add Updates Center page`

**Acceptance criteria:**
- [ ] Page loads at `/updates` route
- [ ] Shows list of backends with version info
- [ ] Shows list of models with version info
- [ ] "Check Now" button triggers re-check
- [ ] Update buttons redirect to appropriate handlers
- [ ] Sidebar shows Updates link with badge when updates available
- [ ] Route registered in Leptos router

---

## Task 7: Cleanup Hooks

**Context:**
When a backend is removed or a model is deleted/renamed, we must clean up any corresponding `update_checks` records. This prevents stale entries from appearing in the Updates Center.

**Files:**
- Modify: `crates/koji-web/src/api/backends.rs` (add cleanup to `remove_backend`)
- Modify: `crates/koji-web/src/api.rs` (add cleanup to `delete_model` and `rename_model`)

**What to implement:**

### In `backends.rs`, add to `remove_backend` handler:

After removing from registry:
```rust
// Clean up update_check record
if let Ok(open) = koji_core::db::open(&config_dir) {
    let _ = koji_core::db::queries::delete_update_check(
        &open.conn,
        "backend",
        &name,
    );
}
```

### In `api.rs`, add to `delete_model` handler:

After removing from config:
```rust
// Clean up update_check record
if let Ok(open) = koji_core::db::open(&config_dir) {
    let _ = koji_core::db::queries::delete_update_check(
        &open.conn,
        "model",
        &id,
    );
}
```

### In `api.rs`, add to `rename_model` handler:

After renaming:
```rust
// Clean up update_check record for old ID
if let Ok(open) = koji_core::db::open(&config_dir) {
    let _ = koji_core::db::queries::delete_update_check(
        &open.conn,
        "model",
        &id,
    );
}
```

**Steps:**
- [ ] Add `delete_update_check` call to `remove_backend` in backends.rs
- [ ] Add `delete_update_check` call to `delete_model` in api.rs
- [ ] Add `delete_update_check` call to `rename_model` in api.rs
- [ ] Run `cargo build --package koji-web`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `fix(cleanup): delete update_check records on model/backend removal`

**Acceptance criteria:**
- [ ] Removing a backend cleans up its update_check record
- [ ] Deleting a model cleans up its update_check record
- [ ] Renaming a model cleans up the old update_check record

---

## Task 8: Integration & Testing

**Context:**
After implementing all the pieces, we need to verify everything works together correctly. This includes testing the full flow, running all tests, and checking for any regressions.

**Files:**
- Add: `crates/koji-core/src/updates/tests.rs`
- Add: `crates/koji-web/tests/updates_api.rs`

**What to implement:**

### Integration test in koji-core:

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_full_update_check_flow() {
        let temp_dir = tempdir().unwrap();
        let checker = UpdateChecker::new();

        // Initially should_check returns true (never checked)
        let should = checker.should_check(temp_dir.path()).await.unwrap();
        assert!(should);

        // Run a check
        checker.run_check(temp_dir.path()).await.unwrap();

        // Now should_check should return false (just checked)
        let should = checker.should_check(temp_dir.path()).await.unwrap();
        assert!(!should);

        // Get results
        let results = checker.get_results(temp_dir.path()).await.unwrap();
        // Results may be empty if no backends/models configured
        assert!(results.iter().all(|r| r.item_type == "backend" || r.item_type == "model"));
    }
}
```

### API smoke test:

Reference the existing test patterns in `crates/koji-web/tests/` (e.g., `backends_api.rs`) for constructing test `AppState`. The key fields needed:

```rust
// Helper to create minimal test AppState
fn test_app_state() -> Arc<AppState> {
    Arc::new(AppState {
        proxy_base_url: "http://localhost:11434".to_string(),
        client: reqwest::Client::new(),
        logs_dir: Some(tempfile::tempdir().unwrap().path().to_path_buf()),
        config_path: Some(tempfile::tempdir().unwrap().path().join("config.toml")),
        proxy_config: None,
        jobs: None,
        capabilities: None,
        binary_version: "test".to_string(),
        update_tx: Arc::new(tokio::sync::Mutex::new(None)),
        upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    })
}

#[tokio::test]
async fn test_get_updates_returns_json() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = test_app_state();

    tokio::spawn(async move {
        let app = build_router(state);
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(&format!("http://{}/api/updates", addr))
        .send()
        .await
        .unwrap();

    // Should return 200 even with empty/initial state
    assert!(resp.status().is_success());
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("backends").is_some());
    assert!(body.get("models").is_some());
}
```

**Note:** For full integration tests with actual update checking, you may need to set up a temporary directory with config.toml and koji.db.

### Verification steps:

1. Run full test suite:
   ```bash
   cargo test --workspace
   ```

2. Run clippy:
   ```bash
   cargo clippy --workspace -- -D warnings
   ```

3. Run fmt:
   ```bash
   cargo fmt --all
   ```

4. Build release:
   ```bash
   cargo build --release --workspace
   ```

**Steps:**
- [ ] Add integration test for UpdateChecker in `crates/koji-core/src/updates/tests.rs`
- [ ] Add API smoke test in `crates/koji-web/tests/updates_api.rs`
- [ ] Run `cargo test --workspace`
- [ ] Fix any failing tests
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --release --workspace`
- [ ] Commit with message: `test: add integration tests for Updates Center`

**Acceptance criteria:**
- [ ] All tests pass
- [ ] Clippy passes with no warnings
- [ ] Code is properly formatted
- [ ] Release build succeeds

---

## Summary

| Task | Description | Package |
|------|-------------|--------|
| 1 | Database migration v6 | koji-core |
| 2 | DB query functions | koji-core |
| 3 | Configuration field | koji-core, koji-web |
| 4 | Background checker module | koji-core |
| 5 | API endpoints | koji-web |
| 6 | Frontend page | koji-web |
| 7 | Cleanup hooks | koji-web |
| 8 | Integration & testing | both |

**Estimated total tasks:** 8 independent, committable tasks
**Test commands:**
- `cargo test --workspace` - Run all tests
- `cargo clippy --workspace -- -D warnings` - Lint
- `cargo fmt --all` - Format
- `cargo build --release --workspace` - Release build

---

