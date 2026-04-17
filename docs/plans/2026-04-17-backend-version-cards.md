# Backend Version Cards Plan

**Goal:** Allow users to see and switch between multiple installed versions of the same backend (e.g., llama.cpp b8827 and b8407) via visual version cards in the UI, new API endpoints, and CLI commands.

**Architecture:** The SQLite `backend_installations` table already supports multiple versions with an `is_active` flag. We expose this through new `BackendRegistry` methods, new REST API endpoints, a redesigned backends page showing one card per version, and new CLI commands. Default args remain per-backend (shared across versions) via config.toml.

**Tech Stack:** Rust (koji-core, koji-web, koji-cli), SQLite (rusqlite), Leptos (wasm frontend), Axum (REST API).

---

## Task 1: Backend Registry — New DB Queries and Methods

**Context:**
The `backend_installations` table already has an `is_active` column and a `list_backend_versions()` query that returns all versions. However, the public `BackendRegistry` API only exposes active-only methods (`get()`, `list()`). We need to add methods to list all versions and activate a specific version. This is the foundation — all other tasks depend on this.

**Files:**
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-core/src/db/queries/backend_queries.rs`
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-core/src/backends/registry/registry_ops.rs`
- Test: `/home/daniel/Coding/Rust/koji/crates/koji-core/src/backends/registry/registry_ops.rs` (inline `#[cfg(test)]` module)

**What to implement:**

### 1a. New DB query functions in `backend_queries.rs`

Add these two functions after the existing `delete_backend_installation()` function:

```rust
/// Deactivate all versions for a backend name, then activate the specified version.
///
/// This is an atomic operation executed in a transaction:
/// 1. SET is_active = 0 for all rows with the given name
/// 2. SET is_active = 1 for the row matching (name, version)
///
/// Returns Ok(true) if the version was found and activated, Ok(false) if no matching row exists.
pub fn activate_backend_version(
    conn: &Connection,
    name: &str,
    version: &str,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;

    // Deactivate all versions for this backend
    tx.execute(
        "UPDATE backend_installations SET is_active = 0 WHERE name = ?1",
        [name],
    )?;

    // Activate the requested version
    let changes = tx.execute(
        "UPDATE backend_installations SET is_active = 1 WHERE name = ?1 AND version = ?2",
        (name, version),
    )?;

    tx.commit()?;
    Ok(changes > 0)
}
```

### 1b. New `BackendRegistry` methods in `registry_ops.rs`

Add these three methods to the `impl BackendRegistry` block (after the existing `update_version` method, before the private helper methods):

```rust
/// List ALL versions of a backend (active + inactive), ordered by installed_at DESC.
///
/// Returns Ok(None) if no backend with that name exists at all.
pub fn list_all_versions(&self, name: &str) -> Result<Option<Vec<BackendInfo>>> {
    let records =
        list_backend_versions(&self.conn, name).with_context(|| format!("Failed to query versions for backend '{}'", name))?;

    if records.is_empty() {
        return Ok(None);
    }

    records
        .into_iter()
        .map(Self::record_to_backend_info)
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// Activate a specific version of a backend.
///
/// Deactivates all other versions and activates the requested one.
/// Returns Ok(true) if the version was found and activated, Ok(false) if not found.
pub fn activate(&mut self, name: &str, version: &str) -> Result<bool> {
    activate_backend_version(&self.conn, name, version)
        .with_context(|| format!("Failed to activate backend '{}' version '{}'", name, version))
}

/// Remove a single (name, version) installation from the registry.
///
/// **Note:** This method handles **DB operations only** — it does NOT delete files from disk.
/// File deletion is the caller’s responsibility (e.g., in the CLI command).
///
/// If this was the active version and other versions remain, the newest remaining
/// version is activated. If this was the last version, the row is simply deleted
/// (no active version remains for that backend name).
    // Get the record before deleting, to check if it was active and to get the path
    let record = get_backend_by_version(&self.conn, name, version)
        .with_context(|| format!("Failed to query backend '{}' version '{}'", name, version))?;

    let was_active = record.as_ref().map_or(false, |r| r.is_active);

    // Delete the DB row
    delete_backend_installation(&self.conn, name, version)
        .with_context(|| format!("Failed to remove backend '{}' version '{}'", name, version))?;

    // If this was the active version, we need to activate another one if available
    if was_active {
        let remaining = list_backend_versions(&self.conn, name)
            .with_context(|| format!("Failed to query remaining versions for backend '{}'", name))?;

        if !remaining.is_empty() {
            // Activate the newest remaining version (first in DESC order)
            let newest = &remaining[0];
            activate_backend_version(&self.conn, name, &newest.version)
                .with_context(|| format!("Failed to activate fallback version '{}' for backend '{}'", newest.version, name))?;
        }
    }

    Ok(())
}
```

### 1c. Tests in `registry_ops.rs`

Add these tests to the existing `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_registry_list_all_versions() {
    let registry = BackendRegistry::open_in_memory().unwrap();

    // No versions for unknown backend
    assert!(registry.list_all_versions("nonexistent").unwrap().is_none());

    // Add two versions of the same backend
    let mut registry = BackendRegistry::open_in_memory().unwrap();

    let info1 = make_backend_info("llama_cpp", "b8407");
    let info2 = make_backend_info("llama_cpp", "b9000");

    registry.add(info1).unwrap();
    registry.add(info2).unwrap();

    let versions = registry.list_all_versions("llama_cpp").unwrap().unwrap();
    assert_eq!(versions.len(), 2);
    // Newest should be first (order by installed_at DESC)
    assert_eq!(versions[0].version, "b9000");
    assert_eq!(versions[1].version, "b8407");
}

#[test]
fn test_registry_activate_version() {
    let mut registry = BackendRegistry::open_in_memory().unwrap();

    registry.add(make_backend_info("llama_cpp", "b8407")).unwrap();
    registry.add(make_backend_info("llama_cpp", "b9000")).unwrap();

    // b9000 is active (added last)
    let active = registry.get("llama_cpp").unwrap().unwrap();
    assert_eq!(active.version, "b9000");

    // Activate b8407
    let result = registry.activate("llama_cpp", "b8407").unwrap();
    assert!(result);

    // Now b8407 should be active
    let active = registry.get("llama_cpp").unwrap().unwrap();
    assert_eq!(active.version, "b8407");
}

#[test]
fn test_registry_activate_nonexistent_version() {
    let mut registry = BackendRegistry::open_in_memory().unwrap();

    registry.add(make_backend_info("llama_cpp", "b8407")).unwrap();

    let result = registry.activate("llama_cpp", "nonexistent").unwrap();
    assert!(!result);

    // Existing version should still be active
    let active = registry.get("llama_cpp").unwrap().unwrap();
    assert_eq!(active.version, "b8407");
}

#[test]
fn test_registry_remove_version() {
    let mut registry = BackendRegistry::open_in_memory().unwrap();

    registry.add(make_backend_info("llama_cpp", "b8407")).unwrap();
    registry.add(make_backend_info("llama_cpp", "b9000")).unwrap();

    // Both versions exist
    let all = registry.list_all_versions("llama_cpp").unwrap().unwrap();
    assert_eq!(all.len(), 2);

    // Remove b8407
    registry.remove_version("llama_cpp", "b8407").unwrap();

    // Only b9000 remains and should be active
    let all = registry.list_all_versions("llama_cpp").unwrap().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].version, "b9000");

    let active = registry.get("llama_cpp").unwrap().unwrap();
    assert_eq!(active.version, "b9000");
}

#[test]
fn test_registry_remove_last_version_deactivates_others() {
    let mut registry = BackendRegistry::open_in_memory().unwrap();

    registry.add(make_backend_info("llama_cpp", "b8407")).unwrap();
    registry.add(make_backend_info("llama_cpp", "b9000")).unwrap();

    // Remove the active one (b9000) — b8407 should become active
    registry.remove_version("llama_cpp", "b9000").unwrap();

    let all = registry.list_all_versions("llama_cpp").unwrap().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].version, "b8407");
}

    #[test]
    fn test_registry_remove_last_version_cleans_up() {
        let mut registry = BackendRegistry::open_in_memory().unwrap();

        registry.add(make_backend_info("llama_cpp", "b8407")).unwrap();

        // Remove the only version
        registry.remove_version("llama_cpp", "b8407").unwrap();

        // No versions remain
        assert!(registry.list_all_versions("llama_cpp").unwrap().is_none());

        // list() returns empty for this backend
        let active = registry.get("llama_cpp").unwrap();
        assert!(active.is_none());
    }
```

**Steps:**
- [ ] Add `activate_backend_version()` function to `/home/daniel/Coding/Rust/koji/crates/koji-core/src/db/queries/backend_queries.rs`
- [ ] Add `list_all_versions()`, `activate()`, and `remove_version()` methods to `BackendRegistry` in `/home/daniel/Coding/Rust/koji/crates/koji-core/src/backends/registry/registry_ops.rs`
- [ ] Add the 5 tests listed above to the `#[cfg(test)] mod tests` block in `registry_ops.rs`
- [ ] Run `cd /home/daniel/Coding/Rust/koji && cargo test --package koji-core -- backends::registry::tests` — all 9 tests pass (4 existing + 5 new)
- [ ] Run `cargo test --workspace` — no regressions
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace` — succeeds
- [ ] Commit with message: `feat(core): add list_all_versions, activate, and remove_version to BackendRegistry`

**Acceptance criteria:**
- [ ] `BackendRegistry::list_all_versions(name)` returns all versions for a backend, ordered by installed_at DESC
- [ ] `BackendRegistry::activate(name, version)` activates a specific version and deactivates others
- [ ] `BackendRegistry::remove_version(name, version)` removes one version and activates another if the removed one was active
- [ ] All 5 new tests pass
- [ ] Existing tests still pass (no regression)

---

## Task 2: Web API — Version Endpoints and List Backends Update

**Context:**
The backends page fetches data from `GET /api/backends`. Currently this returns one card per backend type. We need to: (1) add a new endpoint to get all versions of a specific backend, (2) add an endpoint to activate a version, and (3) update the existing list endpoint to return all versions as separate cards.

**Files:**
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-web/src/api/backends.rs`
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-web/src/server.rs`
- Test: `/home/daniel/Coding/Rust/koji/crates/koji-web/tests/backends_api.rs` (enable existing ignored tests)

**What to implement:**

### 2a. New DTOs in `api/backends.rs`

#### Add `is_active` field to existing `BackendCardDto`

In the existing `BackendCardDto` struct (around line ~65 in `api/backends.rs`), add a new field at the end:

```rust
pub struct BackendCardDto {
    pub r#type: String,
    pub display_name: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    #[serde(skip_serializing_if = "UpdateStatusDto::is_default")]
    pub update: UpdateStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    // NEW: Whether this specific version card is the active one
    #[serde(default)]
    pub is_active: bool,
}
```

#### Add new DTOs after the existing `DeleteResponse` struct:

```rust
/// Version info returned by the versions endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<GpuTypeDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
    pub is_active: bool,
}

/// Response for GET /api/backends/:name/versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionsResponse {
    pub versions: Vec<BackendVersionDto>,
    pub active_version: Option<String>,
}

/// Request body for POST /api/backends/:name/activate.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateRequest {
    pub version: String,
}

/// Response for POST /api/backends/:name/activate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivateResponse {
    pub version: String,
    pub is_active: bool,
}
```

### 2b. New handler: `list_backend_versions`

Add this function to `api/backends.rs`:

```rust
/// GET /api/backends/:name/versions
pub async fn list_backend_versions(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    // Validate name (prevent path traversal)
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid backend name"})),
        )
            .into_response();
    }

    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    let config_dir_clone = config_dir.clone();
    let registry_result: Result<koji_core::backends::BackendRegistry, _> =
        tokio::task::spawn_blocking(move || {
            koji_core::backends::BackendRegistry::open(&config_dir_clone)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match registry_result {
        Ok(registry) => {
            let versions_opt = match registry.list_all_versions(&name) {
                Ok(v) => v,
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": format!("Failed to list versions: {}", e)})),
                    )
                        .into_response();
                }
            };

            let versions = match versions_opt {
                Some(v) => v,
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(serde_json::json!({"error": format!("Backend '{}' not found", name)})),
                    )
                        .into_response();
                }
            };

            let dto_versions: Vec<BackendVersionDto> = versions
                .iter()
                .map(|info| BackendVersionDto {
                    name: info.name.clone(),
                    version: info.version.clone(),
                    path: info.path.to_string_lossy().to_string(),
                    installed_at: info.installed_at,
                    gpu_type: info.gpu_type.as_ref().map(|g| g.into()),
                    source: info.source.as_ref().map(|s| s.into()),
                    is_active: registry.get(&name).map_or(false, |active| {
                        active.map_or(false, |a| a.version == info.version)
                    }),
                })
                .collect();

            let active_version = registry.get(&name).ok().flatten().map(|a| a.version);

            Json(BackendVersionsResponse {
                versions: dto_versions,
                active_version,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to open registry: {}", e)})),
        )
            .into_response(),
    }
}
```

### 2c. New handler: `activate_backend_version`

Add this function to `api/backends.rs`:

```rust
/// POST /api/backends/:name/activate
pub async fn activate_backend_version(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<ActivateRequest>,
) -> impl IntoResponse {
    // Validate name
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Invalid backend name"})),
        )
            .into_response();
    }

    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = match config_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "Cannot determine config directory"})),
            )
                .into_response();
        }
    };

    let config_dir_clone = config_dir.clone();
    let version_clone = req.version.clone();
    let registry_result: Result<(koji_core::backends::BackendRegistry, bool), _> =
        tokio::task::spawn_blocking(move || {
            let mut reg = koji_core::backends::BackendRegistry::open(&config_dir_clone)?;
            let activated = reg.activate(&name, &version_clone)?;
            Ok((reg, activated))
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn error: {}", e))
        .and_then(|r| r);

    match registry_result {
        Ok((_, activated)) => {
            if !activated {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": format!("Version '{}' not found for backend '{}'", version_clone, name)
                    })),
                )
                    .into_response();
            }

            Json(ActivateResponse {
                version: req.version,
                is_active: true,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("Failed to activate: {}", e)})),
        )
            .into_response(),
    }
}
```

### 2d. Update `list_backends` handler to return all versions

In the existing `list_backends` function, change the card construction logic. Instead of creating one `BackendCardDto` per backend type (taking only the first/latest), create one card per version.

Specifically, replace this block in `list_backends`:

```rust
// OLD: Build a map of installed backends by backend_type (take first/latest)
let mut installed_by_type: std::collections::HashMap<String, BackendInfo> =
    std::collections::HashMap::new();
for info in &all_backends {
    let bt = info.backend_type.to_string();
    installed_by_type.entry(bt).or_insert(info.clone());
}

// OLD: Always emit both known cards
for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
    let installed = installed_by_type.get(&type_.to_string());
    // ... one card per type
}
```

With new logic that creates a card for each version:

```rust
// NEW: Group all backends by (backend_type, name) to get all versions
let mut by_type_and_name: std::collections::HashMap<(String, String), Vec<BackendInfo>> =
    std::collections::HashMap::new();
for info in &all_backends {
    let key = (info.backend_type.to_string(), info.name.clone());
    by_type_and_name.entry(key).or_default().push(info.clone());
}

// NEW: Emit cards for all versions of known backends
for (type_, display_name, release_notes_url) in KNOWN_BACKENDS {
    let default_args = default_args_map.get(&type_.to_string()).cloned().unwrap_or_default();

    // Check if there are any installations with this backend type
    let versions: Vec<_> = by_type_and_name
        .iter()
        .filter(|((bt, _), _)| *bt == type_)
        .flat_map(|(_, versions)| versions)
        .collect();

    if !versions.is_empty() {
        // One card per version (sorted by installed_at DESC)
        let mut sorted_versions = versions.clone();
        sorted_versions.sort_by(|a, b| b.installed_at.cmp(&a.installed_at));

        for info in &sorted_versions {
            backends.push(BackendCardDto {
                r#type: type_.to_string(),
                display_name: display_name.to_string(),
                installed: true,
                info: Some(BackendInfoDto::from(info.clone())),
                update: UpdateStatusDto::default(),
                release_notes_url: release_notes_url.map(String::from),
                default_args: default_args.clone(),
            });
        }
    } else {
        // No versions installed — show uninstalled card
        backends.push(BackendCardDto::default_uninstalled(
            type_,
            display_name,
            *release_notes_url,
            default_args,
        ));
    }
}

// NEW: Custom backends — one card per version (sorted by installed_at DESC)
for ((bt, name), versions) in &by_type_and_name {
    if bt != "llama_cpp" && bt != "ik_llama" {
        // Sort versions by installed_at DESC to ensure deterministic ordering
        let mut sorted_versions = versions.clone();
        sorted_versions.sort_by(|a, b| b.installed_at.cmp(&a.installed_at));

        for info in &sorted_versions {
            let default_args = default_args_map.get(bt).cloned().unwrap_or_default();
            custom.push(BackendCardDto {
                r#type: format!("{}", info.backend_type),
                display_name: format!("Custom ({})", name),
                installed: true,
                info: Some(BackendInfoDto::from(info.clone())),
                update: UpdateStatusDto::default(),
                release_notes_url: None,
                default_args,
            });
        }
    }
}
```

### 2e. Route registration in `server.rs`

Add two new routes to the `backend_routes` Router in `build_router()`:

```rust
// After the existing .route("/api/backends/check-updates", ...) line, add:
.route("/api/backends/:name/versions", get(list_backend_versions))
.route(
    "/api/backends/:name/activate",
    post(activate_backend_version),
)
```

Also update the import at the top of `server.rs`:

```rust
use crate::api::backends::{
    activate_backend_version, check_backend_updates, get_job, install_backend, job_events_sse,
    list_backends, list_backend_versions, remove_backend, system_capabilities, update_backend,
    update_backend_default_args, CapabilitiesCache,
};
```

**Steps:**
- [ ] Add `BackendVersionDto`, `BackendVersionsResponse`, `ActivateRequest`, `ActivateResponse` DTOs to `/home/daniel/Coding/Rust/koji/crates/koji-web/src/api/backends.rs`
- [ ] Add `list_backend_versions()` handler function to `api/backends.rs`
- [ ] Add `activate_backend_version()` handler function to `api/backends.rs`
- [ ] Update `list_backends()` handler to emit one card per version instead of one per type
- [ ] Add route registrations in `/home/daniel/Coding/Rust/koji/crates/koji-web/src/server.rs`
- [ ] Update the import line in `server.rs` to include new handlers
- [ ] Run `cd /home/daniel/Coding/Rust/koji && cargo build --package koji-web` — succeeds
- [ ] Run `cargo test --package koji-web -- backends_api` — no regressions
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat(web): add version endpoints and return all versions in list_backends`

**Acceptance criteria:**
- [ ] `GET /api/backends/:name/versions` returns all versions with active indicator
- [ ] `POST /api/backends/:name/activate` activates a specific version
- [ ] `GET /api/backends` returns one card per version (not one per type)
- [ ] Backend builds successfully

---

## Task 3: UI — Version Cards Component and Backends Page

**Context:**
The backends page currently shows one `BackendCard` per backend type. With multiple versions, we need to show one card per version. The existing `BackendCard` component already displays version info — we just need to add an "Activate" button for inactive versions and a visual "Active" badge.

**Files:**
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-web/src/components/backend_card.rs`
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-web/src/pages/backends.rs`

**What to implement:**

### 3a. Update `BackendCardDto` in BOTH locations

**Important:** `BackendCardDto` is defined in **two places** and both must be updated:
1. `/home/daniel/Coding/Rust/koji/crates/koji-web/src/api/backends.rs` — server-side DTO (used for JSON serialization in API responses)
2. `/home/daniel/Coding/Rust/koji/crates/koji-web/src/components/backend_card.rs` — client-side DTO (used by Leptos wasm frontend)

Add a new field to track whether this specific version card is the active one:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendCardDto {
    pub r#type: String,
    pub display_name: String,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    #[serde(default)]
    pub update: UpdateStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    // NEW: Whether this specific version card is the active one
    #[serde(default)]
    pub is_active: bool,
}
```

### 3b. Update `BackendCard` component to show Activate button

In the `BackendCard` component's view! macro, add an "Activate" button for inactive versions. After the existing "Uninstall" button block, add:

```rust
{if installed && !is_active {
    let cb = on_activate;
    let bt = type_activate.clone();
    view! {
        <button
            type="button"
            class="btn btn-primary"
            style="background:#22c55e;"
            on:click=move |_| {
                if let Some(c) = cb {
                    c.run(bt.clone());
                }
            }
        >
            "Activate"
        </button>
    }.into_any()
} else {
    view! { <span/> }.into_any()
}}
```

Also update the legend to show an "Active" badge when `is_active` is true:

```rust
{if is_active {
    view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Active"</span> }.into_any()
} else if installed {
    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Installed"</span> }.into_any()
} else {
    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Not installed"</span> }.into_any()
}}
```

Add a new prop to the `BackendCard` component for the activate callback:

```rust
/// Called with the backend type when "Activate" is clicked.
#[prop(optional)]
on_activate: Option<Callback<String>>,
```

### 3c. Update Backends page to pass version data

In `/home/daniel/Coding/Rust/koji/crates/koji-web/src/pages/backends.rs`, update the card rendering to include `is_active`:

```rust
// In the card rendering loop, change:
cards.push(view! {
    <BackendCard
        backend=BackendCardDto {
            b,
            is_active: true,  // or derive from info
            ..Default::default()
        }
        on_install=on_install_click
        on_update=on_update_click
        on_check_updates=on_check_updates_click
        on_delete=on_delete_click
        on_default_args_change=on_default_args_change
        on_activate=on_activate_click  // NEW
    />
}.into_any());
```

Add the activate callback:

```rust
let on_activate_click = Callback::new(move |(backend_type, version): (String, String)| {
    action_error.set(None);
    wasm_bindgen_futures::spawn_local(async move {
        let url = format!("/api/backends/{}/activate", backend_type);
        let body = serde_json::json!({ "version": version });
        match gloo_net::http::Request::post(&url)
            .json(&body)
            .unwrap()
            .send()
            .await
        {
            Ok(resp) if resp.ok() => {
                refresh_tick.update(|n| *n += 1);
            }
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_default();
                action_error.set(Some(format!("Activate failed: {text}")));
            }
            Err(e) => action_error.set(Some(format!("Activate request failed: {e}"))),
        }
    });
});
```

**Important note:** The activate endpoint needs to know both the backend name AND the version. The current callback signature `Callback<String>` only passes the backend type. We need to change this.

Update the `on_activate` prop to pass `(backend_name, version)`:

```rust
/// Called with (backend_type, version) when "Activate" is clicked.
#[prop(optional)]
on_activate: Option<Callback<(String, String)>>,
```

And in the Backends page, update the callback:

```rust
let on_activate_click = Callback::new(move |(backend_type, version): (String, String)| {
    action_error.set(None);
    wasm_bindgen_futures::spawn_local(async move {
        let url = format!("/api/backends/{}/activate", backend_type);
        let body = serde_json::json!({ "version": version });
        match gloo_net::http::Request::post(&url)
            .json(&body)
            .unwrap()
            .send()
            .await
        {
            Ok(resp) if resp.ok() => {
                refresh_tick.update(|n| *n += 1);
            }
            Ok(resp) => {
                let text = resp.text().await.unwrap_or_default();
                action_error.set(Some(format!("Activate failed: {text}")));
            }
            Err(e) => action_error.set(Some(format!("Activate request failed: {e}"))),
        }
    });
});
```

### 3d. Update list_backends response in the web API to include is_active per card

In `api/backends.rs`, when building `BackendCardDto` entries, set `is_active` based on whether this version matches the active version:

```rust
// In list_backends, when creating each BackendCardDto:
let is_active = info.version == active_version;  // where active_version comes from registry.get()
```

**Steps:**
- [ ] Add `is_active: bool` field to `BackendCardDto` in `/home/daniel/Coding/Rust/koji/crates/koji-web/src/components/backend_card.rs`
- [ ] Add `on_activate: Option<Callback<(String, String)>>` prop to `BackendCard` component
- [ ] Update the legend in `BackendCard` to show "Active" badge for active versions
- [ ] Add "Activate" button to `BackendCard` for inactive installed versions
- [ ] Update `/home/daniel/Coding/Rust/koji/crates/koji-web/src/pages/backends.rs` to pass `is_active` and `on_activate` callback
- [ ] Update API response building in `api/backends.rs` to set `is_active` per card
- [ ] Build the web frontend: `cd /home/daniel/Coding/Rust/koji/crates/koji-web && cargo build` — succeeds
- [ ] Run `cargo test --workspace` — no regressions
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat(web): show version cards with activate button in backends page`

**Acceptance criteria:**
- [ ] Each installed version shows as a separate card
- [ ] Active version has a green "Active" badge
- [ ] Inactive versions show an "Activate" button
- [ ] Clicking "Activate" calls the API and refreshes the page
- [ ] Default args are still shown per-card (shared from config.toml)

---

## Task 4: CLI — Version Commands

**Context:**
The CLI needs commands to list all versions and switch between them. This gives power users a quick way to manage versions without editing config.toml.

**Files:**
- Modify: `/home/daniel/Coding/Rust/koji/crates/koji-cli/src/commands/backend.rs`

**What to implement:**

### 4a. New subcommands in `BackendSubcommand` enum

Add these variants:

```rust
/// List all versions of a backend (not just the active one)
AllVersions {
    /// Name of the backend (omit to list all backends with all their versions)
    #[arg(long)]
    name: Option<String>,
},

/// Activate a specific version of a backend
Switch {
    /// Name of the backend
    name: String,
    /// Version to activate
    version: String,
},

/// Remove a single version (not all versions)
RemoveVersion {
    /// Name of the backend
    name: String,
    /// Version to remove
    version: String,
},
```

### 4b. Route the new subcommands in `run()`

```rust
BackendSubcommand::AllVersions { name } => cmd_all_versions(config, name.as_deref()).await,
BackendSubcommand::Switch { name, version } => cmd_switch(config, &name, &version).await,
BackendSubcommand::RemoveVersion { name, version } => cmd_remove_version(config, &name, &version).await,
```

### 4c. Implement `cmd_all_versions()`

```rust
async fn cmd_all_versions(_config: &Config, name: Option<&str>) -> Result<()> {
    let registry = BackendRegistry::open(&registry_config_dir()?)?;
    let active_backends = registry.list()?;

    if active_backends.is_empty() {
        println!("No backends installed.");
        return Ok(());
    }

    // Collect all versions to display
    struct VersionEntry {
        name: String,
        backend_type: BackendType,
        version: String,
        path: std::path::PathBuf,
        gpu_type: Option<koji_core::gpu::GpuType>,
        is_active: bool,
    }

    let mut entries: Vec<VersionEntry> = Vec::new();

    if let Some(target_name) = name {
        // Show all versions for a specific backend
        match registry.list_all_versions(target_name)? {
            Some(versions) => {
                for v in versions {
                    entries.push(VersionEntry {
                        name: v.name.clone(),
                        backend_type: v.backend_type.clone(),
                        version: v.version.clone(),
                        path: v.path.clone(),
                        gpu_type: v.gpu_type.clone(),
                        is_active: v.version == registry.get(target_name)?.map(|a| a.version).unwrap_or_default(),
                    });
                }
            }
            None => {
                println!("Backend '{}' not found.", target_name);
                return Ok(());
            }
        }
    } else {
        // Show all versions for all backends
        for active in &active_backends {
            let name = active.name.clone();
            let backend_type = active.backend_type.clone();
            let gpu_type = active.gpu_type.clone();
            let active_version = active.version.clone();

            // Get all versions for this backend
            let all_versions = match registry.list_all_versions(&name)? {
                Some(v) => v,
                None => vec![active.clone()],
            };

            for v in all_versions {
                entries.push(VersionEntry {
                    name: v.name.clone(),
                    backend_type: v.backend_type.clone(),
                    version: v.version.clone(),
                    path: v.path.clone(),
                    gpu_type: v.gpu_type.clone(),
                    is_active: v.version == active_version,
                });
            }
        }
    }

    if entries.is_empty() {
        println!("No versions found.");
        return Ok(());
    }

    println!("Backend versions:\n");
    for entry in &entries {
        let active_marker = if entry.is_active { " * active" } else { "" };
        println!(
            "  {} [{}]{} (v{})",
            entry.name, entry.backend_type, active_marker, entry.version
        );
        println!("    Path:     {}", entry.path.display());
        if let Some(ref gpu) = entry.gpu_type {
            println!("    GPU:      {:?}", gpu);
        }
        println!();
    }

    // Show usage tip
    if let Some(target) = name {
        println!("To activate a version: koji backend switch {} <version>", target);
    } else {
        println!("To activate a version: koji backend switch <backend_name> <version>");
    }

    Ok(())
}
```

### 4d. Implement `cmd_switch()`

```rust
async fn cmd_switch(_config: &Config, name: &str, version: &str) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Check the version exists
    let versions = match registry.list_all_versions(name)? {
        Some(v) => v,
        None => anyhow::bail!(
            "Backend '{}' not found. Run `koji backend list` to see installed backends.",
            name
        ),
    };

    let version_exists = versions.iter().any(|v| v.version == version);
    if !version_exists {
        let available: Vec<_> = versions.iter().map(|v| &v.version).collect();
        anyhow::bail!(
            "Version '{}' not found for backend '{}'. Available: {}",
            version,
            name,
            available.join(", ")
        );
    }

    // Activate the version
    let activated = registry.activate(name, version)?;
    if !activated {
        anyhow::bail!("Failed to activate version '{}'", version);
    }

    println!(
        "Activated backend '{}' version '{}'.",
        name, version
    );

    Ok(())
}
```

### 4e. Implement `cmd_remove_version()`

**Critical ordering:** Files must be deleted **before** DB operations. If file deletion fails after DB deletion, the backend is removed from registry but files remain on disk (orphaned). This matches the existing `cmd_remove()` pattern.

The `BackendRegistry::remove_version()` method (from Task 1) handles **DB operations only** — it removes the row and activates another version if needed. It does NOT delete files.

```rust
async fn cmd_remove_version(_config: &Config, name: &str, version: &str) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Get the version info before removing
    let record = crate::db::queries::get_backend_by_version(
        &Config::open_db(),
        name,
        version,
    )?
    .ok_or_else(|| anyhow!("Backend '{}' version '{}' not found", name, version))?;

    println!("Removing backend '{}' version '{}'", name, version);
    println!("  Path: {}", record.path);

    let confirm = inquire::Confirm::new("Are you sure? This will delete the backend files.")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // STEP 1: Delete files FIRST (before any DB changes)
    let info = BackendInfo {
        name: record.name.clone(),
        backend_type: record.backend_type.parse()?,
        version: record.version.clone(),
        path: std::path::PathBuf::from(&record.path),
        installed_at: record.installed_at,
        gpu_type: None,
        source: None,
    };

    if info.path.exists() {
        safe_remove_installation(&info)?;
    }

    // STEP 2: Remove from registry (activates another version if this was active)
    registry.remove_version(name, version)?;

    println!("Version '{}' removed.", version);

    Ok(())
}
```

### 4f. Update `cmd_list()` to show active marker

In the existing `cmd_list()` function, mark the active version with a `*`:

```rust
// In the loop printing backends:
println!("  {} [{}]{} (v{})", backend.name, backend.backend_type, if is_active { " * active" } else { "" }, backend.version);
```

Where `is_active` can be derived from `registry.get(name)` matching the version.

**Steps:**
- [ ] Add `AllVersions`, `Switch`, and `RemoveVersion` subcommands to `BackendSubcommand` enum in `/home/daniel/Coding/Rust/koji/crates/koji-cli/src/commands/backend.rs`
- [ ] Add routing in `run()` function
- [ ] Implement `cmd_all_versions()`, `cmd_switch()`, and `cmd_remove_version()` functions
- [ ] Update `cmd_list()` to show active marker with `*`
- [ ] Run `cd /home/daniel/Coding/Rust/koji && cargo build --package koji-cli` — succeeds
- [ ] Run `cargo test --workspace` — no regressions
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `feat(cli): add backend switch, all-versions, and remove-version commands`

**Acceptance criteria:**
- [ ] `koji backend list --all` shows all versions with `* active` marker
- [ ] `koji backend switch <name> <version>` activates a version
- [ ] `koji backend switch` validates the version exists before activating
- [ ] `koji backend remove-version <name> <version>` removes one version (files + DB)
- [ ] CLI builds successfully

---

## Task 5: Integration — Update Default Args for All Versions

**Context:**
When the user edits default args in the backends page, the changes are stored in config.toml under `[backends.<name>]`. Since default args are shared across all versions (per-backend), we need to make sure the API and UI handle this correctly. This is mostly about ensuring the existing `update_backend_default_args` handler works with the new multi-version data model — no changes should be needed, but we should verify.

**Files:**
- Verify: `/home/daniel/Coding/Rust/koji/crates/koji-web/src/api/backends.rs` (existing `update_backend_default_args`)
- Verify: `/home/daniel/Coding/Rust/koji/crates/koji-core/src/config/types.rs` (`BackendConfig.version` field)

**What to implement:**

### 5a. Verify existing default args flow works with multi-version

The existing `POST /api/backends/:name/default-args` handler writes to config.toml. Since config.toml stores args under `[backends.<name>]` (not per-version), this should work unchanged. However, we need to ensure the `list_backends` endpoint correctly passes default args for each version card.

In the updated `list_backends` handler from Task 2, verify that `default_args` is fetched by backend type and shared across all version cards of the same type:

```rust
// Already handled in Task 2d — default_args is fetched by type key and reused for all versions
let default_args = default_args_map.get(&type_.to_string()).cloned().unwrap_or_default();
```

### 5b. Verify `resolve_backend_path` works with multi-version

The existing `resolve_backend_path()` in `/home/daniel/Coding/Rust/koji/crates/koji-core/src/config/resolve/mod.rs` already handles version pins from config.toml and falls back to the active version. No changes needed — just verify it works:

- If `config.backends[name].version` is set → looks up that specific version in DB
- If not set → uses the active version from DB (`is_active = 1`)

**Steps:**
- [ ] Verify `update_backend_default_args` handler works unchanged with multi-version cards
- [ ] Verify `resolve_backend_path()` correctly resolves to active version when no pin is set
- [ ] Run `cd /home/daniel/Coding/Rust/koji && cargo test --workspace` — all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `chore(integration): verify default args and path resolution work with multi-version backends`

**Acceptance criteria:**
- [ ] Default args editing works for all version cards (writes to config.toml per-backend)
- [ ] `resolve_backend_path()` returns the correct binary path based on active version or config pin
- [ ] All workspace tests pass

---

## Implementation Order

1. **Task 1** — Backend Registry API (foundation, no dependencies)
2. **Task 2** — Web API endpoints (depends on Task 1)
3. **Task 3** — UI Version Cards (depends on Task 2)
4. **Task 4** — CLI Commands (depends on Task 1, can be done in parallel with 2/3 after commit)
5. **Task 5** — Integration verification (depends on all above)
