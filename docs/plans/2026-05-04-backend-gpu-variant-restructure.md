# Backend GPU Variant Restructure Plan

**Goal:** Allow multiple GPU variants of the same backend (e.g., llama_cpp with Vulkan AND ROCm) to coexist by restructuring the folder layout and adding `gpu_variant` as a second key dimension.

**Architecture:** Add `gpu_variant: String` to `BackendInfo` and DB schema. Folder structure changes from `backends/<name>/` to `backends/<type>/<gpu_variant>/<version>/`. All registry methods gain `gpu_variant` parameter. Legacy installations are auto-migrated on first launch.

**Tech Stack:** Rust, SQLite (rusqlite), Leptos (WebUI)

---

## Task 1: Data model, DB migration, queries & registry (merged)

**Context:**
The core change is adding `gpu_variant` as a folder-identifying key derived from `GpuType`. Currently the DB has `UNIQUE(name, version)` which prevents the same version from existing under different GPU variants. This task adds the field, derives it from `GpuType`, migrates the DB schema, updates all queries and registry methods, and updates all call-sites that construct `BackendInfo`. These are merged into one task because adding `gpu_variant` to the structs breaks compilation everywhere they're used — the codebase must compile at the end of this single commit.

**NOTE:** `gpu_variant: String` fields use `#[serde(default)]` so deserialization of old data doesn't break. The default resolves to `"cpu"`.

**Files:**
- Modify: `crates/tama-core/src/gpu/detect.rs`
- Modify: `crates/tama-core/src/backends/registry/registry_ops.rs`
- Modify: `crates/tama-core/src/backends/mod.rs` (tests)
- Modify: `crates/tama-core/src/backends/installer/mod.rs` (tests)
- Modify: `crates/tama-core/src/backends/tts_kokoro/mod.rs`
- Modify: `crates/tama-core/src/db/queries/backend_queries.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/mod.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/config/resolve/mod.rs` (resolve_backend_path)
- Modify: `crates/tama-core/src/config/types.rs` (BackendConfig)
- Modify: `crates/tama-core/src/backup/archive.rs`
- Modify: `crates/tama-core/src/backup/merge.rs`

**What to implement:**

### A. Data model changes

1. **Add `variant_folder()` method to `GpuType`** in `gpu/detect.rs`:
```rust
impl GpuType {
    /// Returns the folder name used for this GPU variant.
    /// e.g. "cpu", "vulkan", "metal", "cuda", "rocm", "custom"
    pub fn variant_folder(&self) -> &str {
        match self {
            GpuType::CpuOnly => "cpu",
            GpuType::Vulkan => "vulkan",
            GpuType::Metal => "metal",
            GpuType::Cuda { .. } => "cuda",
            GpuType::RocM { .. } => "rocm",
            GpuType::Custom => "custom",
        }
    }
}
```

2. **Add `gpu_variant: String` field to `BackendInfo`** in `registry_ops.rs`:
```rust
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    pub gpu_type: Option<GpuType>,
    #[serde(default)]
    pub gpu_variant: String,   // NEW: folder key, e.g. "cpu", "vulkan", "cuda"
    pub source: Option<BackendSource>,
}
```

3. **Add `gpu_variant: String` field to `BackendInstallationRecord`** in `db/queries/types.rs`:
```rust
pub struct BackendInstallationRecord {
    pub id: i64,
    pub name: String,
    pub backend_type: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    pub gpu_type: Option<String>,
    pub gpu_variant: String,   // NEW
    pub source: Option<String>,
    pub is_active: bool,
}
```

4. **Add `gpu_variant: Option<String>` to `BackendConfig`** in `config/types.rs`:
   - This allows users to pin a specific GPU variant in config.toml
   - Used by `resolve_backend_path` when looking up which variant to use

### B. DB migration v20

5. **DB migration v20** in `migrations.rs`:
   - Increment `LATEST_VERSION` to 20
   - Migration must rebuild `backend_installations` table (DROP + RENAME pattern like v9) because we need to change the UNIQUE constraint
   - Steps:
     a. Create `backend_installations_new` with `gpu_variant TEXT NOT NULL DEFAULT 'cpu'` and `UNIQUE(name, gpu_variant, version)`
     b. Copy data from old to new (all existing rows get `gpu_variant = 'cpu'`)
     c. Drop old table, rename new to `backend_installations`
     d. Create index `idx_backend_installations_name_variant ON backend_installations(name, gpu_variant)`
   - Add v20 to `FK_OFF_MIGRATIONS` array (because DROP TABLE with FKs ON would cascade)
   - Add test `test_migration_v20_adds_gpu_variant`

### C. Query & registry updates

6. **Update `insert_backend_installation`** in `backend_queries.rs`:
   - Add `gpu_variant` to INSERT columns
   - Deactivate logic: `UPDATE ... SET is_active = 0 WHERE name = ?1 AND gpu_variant = ?2 AND version != ?3`
   - **Semantic change:** Installing a new version only deactivates other versions of the *same variant*, not other variants

7. **Update `get_active_backend`**:
   - Add `gpu_variant: &str` parameter
   - Query: `WHERE name = ?1 AND gpu_variant = ?2 AND is_active = 1`

8. **Update `list_active_backends`**:
   - No parameter change (returns ALL active backends across all variants)
   - SELECT must include `gpu_variant` column

9. **Update `list_backend_versions`**:
   - Add optional `gpu_variant: Option<&str>` parameter
   - When `Some(variant)`: `WHERE name = ?1 AND gpu_variant = ?2`
   - When `None`: `WHERE name = ?1` (all variants)
   - SELECT must include `gpu_variant` column

10. **Update `get_backend_by_version`**:
    - Add `gpu_variant: &str` parameter
    - Query: `WHERE name = ?1 AND gpu_variant = ?2 AND version = ?3`

11. **Update `delete_backend_installation`**:
    - Add `gpu_variant: &str` parameter

12. **Update `activate_backend_version`**:
    - Add `gpu_variant: &str` parameter

13. **Update `delete_all_backend_versions`**:
    - Add optional `gpu_variant: Option<&str>` parameter

14. **Update `BackendRegistry` methods** in `registry_ops.rs`:
    - `get(&self, name, gpu_variant) -> Result<Option<BackendInfo>>`
    - `list_all_versions(&self, name, gpu_variant: Option<&str>) -> Result<Option<Vec<BackendInfo>>>`
    - `activate(&mut self, name, gpu_variant, version) -> Result<bool>`
    - `remove_version(&mut self, name, gpu_variant, version) -> Result<()>`
    - `remove(&mut self, name, gpu_variant: Option<&str>) -> Result<()>`
    - `update_version` — preserve gpu_variant from existing record
    - `record_to_backend_info` — read `gpu_variant` from record
    - `backend_info_to_record` — write `gpu_variant` to record

### D. Call-site updates (must compile)

15. **Update all `BackendInfo` construction sites**:
    - `backends/mod.rs` tests — add `gpu_variant: "cpu"`
    - `backends/installer/mod.rs` tests — add `gpu_variant: "cpu"`
    - `backends/tts_kokoro/mod.rs` — add `gpu_variant: "cpu"` (TTS uses CPU)
    - `registry_ops.rs` tests — add `gpu_variant` to all test fixtures
    - Any other construction sites found during compilation

16. **Update `resolve_backend_path`** in `config/resolve/mod.rs`:
    - Current: calls `get_active_backend(conn, name)` and `get_backend_by_version(conn, name, pinned_version)`
    - New: check `BackendConfig.gpu_variant` first
    - If `gpu_variant` is set in config → use it
    - If not set → call `list_backend_versions(conn, name, None)` to get all variants
      - If only one variant exists → use it
      - If multiple variants → use the first active one (or log warning and pick first)
    - Pass `gpu_variant` to `get_active_backend` and `get_backend_by_version`

17. **Update backup/restore** in `backup/archive.rs` and `backup/merge.rs`:
    - Add `gpu_variant` to CREATE TABLE statements
    - Add `gpu_variant` to INSERT statements
    - Update test fixtures

**Steps:**
- [ ] Write test for `GpuType::variant_folder()` covering all variants
- [ ] Run `cargo test --package tama-core gpu::detect::tests`
- [ ] Implement `variant_folder()` method on `GpuType`
- [ ] Add `gpu_variant` field to `BackendInfo` with `#[serde(default)]`
- [ ] Add `gpu_variant` field to `BackendInstallationRecord`
- [ ] Add `gpu_variant: Option<String>` to `BackendConfig`
- [ ] Write migration v20 SQL
- [ ] Add v20 to `FK_OFF_MIGRATIONS`
- [ ] Write test `test_migration_v20_adds_gpu_variant`
- [ ] Update all 7 query functions in `backend_queries.rs`
- [ ] Update `BackendRegistry` methods in `registry_ops.rs`
- [ ] Update `record_to_backend_info` / `backend_info_to_record`
- [ ] Update all SELECT statements to include `gpu_variant` column
- [ ] Fix all `BackendInfo` construction sites (tests, TTS, etc.)
- [ ] Update `resolve_backend_path` to handle gpu_variant
- [ ] Update backup/archive.rs CREATE TABLE and INSERT statements
- [ ] Update backup/merge.rs INSERT statements
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add gpu_variant to data model, queries, registry, and call-sites"

**Acceptance criteria:**
- [ ] `GpuType::variant_folder()` returns correct string for all 6 variants
- [ ] `BackendInfo` has `gpu_variant: String` field with `#[serde(default)]`
- [ ] `BackendInstallationRecord` has `gpu_variant: String` field
- [ ] `BackendConfig` has `gpu_variant: Option<String>` field
- [ ] Migration v20 rebuilds table with `UNIQUE(name, gpu_variant, version)`
- [ ] All backend queries include `gpu_variant` in WHERE clauses
- [ ] `BackendRegistry::get()` requires both `name` and `gpu_variant`
- [ ] `resolve_backend_path` handles gpu_variant resolution
- [ ] Backup/restore SQL includes gpu_variant column
- [ ] Workspace compiles cleanly (cargo build --workspace)
- [ ] All tests pass (cargo test --workspace)

---

## Task 3: Path computation & installer

**Context:**
The installer needs to compute the new versioned path structure: `backends/<type>/<gpu_variant>/<version>/`. The `prepare_target_dir` function needs to handle parent directory creation separately from the version directory existence check.

**Files:**
- Modify: `crates/tama-core/src/backends/mod.rs`
- Modify: `crates/tama-core/src/backends/installer/mod.rs`
- Modify: `crates/tama-core/src/backends/installer/prebuilt.rs`
- Modify: `crates/tama-core/src/backends/installer/source/install.rs`

**What to implement:**

1. **Add `get_backend_install_path` helper** in `backends/mod.rs`:
```rust
/// Compute the installation directory for a backend given its type, GPU variant, and version.
/// Returns: backends_dir / backend_type / gpu_variant / version
pub fn get_backend_install_path(
    backends_dir: &Path,
    backend_type: &BackendType,
    gpu_variant: &str,
    version: &str,
) -> PathBuf {
    backends_dir
        .join(backend_type.to_string())
        .join(gpu_variant)
        .join(version)
}
```

2. **Update `InstallOptions`** in `installer/mod.rs`:
   - Add `gpu_variant: String` field
   - **Keep `target_dir` as a field** — callers compute it using `get_backend_install_path` before constructing `InstallOptions`. This is the least disruptive approach (no API change for callers who already pass `target_dir`).

3. **Update `prepare_target_dir`** in `installer/prebuilt.rs`:
   - Always create parent directories (`backends/llama_cpp/cuda/`) if missing
   - Only check existence of the final version directory (`b8407/`)
   - `allow_overwrite=true` removes only the version directory

4. **Update `install_prebuilt`** in `installer/prebuilt.rs`:
   - Compute `target_dir` using `get_backend_install_path`
   - Pass version to target dir computation

5. **Update `install_from_source`** in `installer/source/install.rs`:
   - Compute `target_dir` using `get_backend_install_path`
   - Binary is installed into the version folder

6. **Update `safe_remove_installation`** in `backends/mod.rs`:
   - For binary backends: remove `path.parent()` (the version folder)
   - For directory backends (TTS): remove `path` itself (the base_dir)
   - Logic already handles this distinction via `is_dir()` check

7. **Update `install_tts_kokoro`** in `backends/tts_kokoro/`:
   - Use `get_backend_install_path` with `gpu_variant = "cpu"` (TTS doesn't use GPU)
   - Read the paths module to understand current TTS path computation

**Steps:**
- [ ] Implement `get_backend_install_path` helper with tests
- [ ] Run `cargo test --package tama-core backends::tests`
- [ ] Add `gpu_variant` field to `InstallOptions`
- [ ] Update `prepare_target_dir` to handle parent dirs vs version dir
- [ ] Update `install_prebuilt` to use new path computation
- [ ] Update `install_from_source` to use new path computation
- [ ] Verify `safe_remove_installation` works with new path structure (path.parent() = version folder)
- [ ] Update TTS kokoro installer to use `gpu_variant = "cpu"`
- [ ] Run `cargo test --package tama-core backends::installer`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Commit with message: "feat: restructure backend install paths to type/variant/version"

**Acceptance criteria:**
- [ ] `get_backend_install_path` returns correct nested path for all type/variant/version combos
- [ ] `prepare_target_dir` creates parent dirs but only checks version dir existence
- [ ] Prebuilt installer installs to correct versioned path
- [ ] Source installer installs to correct versioned path
- [ ] TTS installer uses `gpu_variant = "cpu"`

---

## Task 4: CLI commands

**Context:**
The CLI commands need to work with the new `gpu_variant` dimension. The `install` command derives `gpu_variant` from the selected GPU type. The `list` command shows variants. The `remove` and `switch` commands accept `--gpu` flags.

**Files:**
- Modify: `crates/tama-cli/src/commands/backend/mod.rs`
- Modify: `crates/tama-cli/src/commands/backend/parse.rs`

**What to implement:**

1. **Update `cmd_install`**:
   - After GPU type selection, derive `gpu_variant` via `gpu_type.variant_folder()`
   - Compute `target_dir` using `get_backend_install_path(backends_dir?, &backend_type, gpu_variant, &version)`
   - Pass `gpu_variant` when creating `BackendInfo`
   - Register with `registry.add(BackendInfo { gpu_variant, ... })`

2. **Update `cmd_list`**:
   - Call `registry.list()` to get all active backends (across all variants)
   - Display format: `llama_cpp [vulkan] * active (v b8407)`
   - Group by backend name, show variants as sub-entries
   - Show GPU type label from `gpu_type` field

3. **Update `cmd_remove`**:
   - Add `--gpu` optional flag
   - When `--gpu` provided: look up variant, call `registry.remove(name, Some(gpu_variant))`
   - When `--gpu` omitted: call `registry.remove(name, None)` (removes all variants)
   - For file deletion: iterate all variants and delete each

4. **Update `cmd_switch`**:
   - Add `--gpu` flag (required when multiple variants exist)
   - If `--gpu` omitted and only one variant exists → auto-infer
   - If `--gpu` omitted and multiple variants → error: "Multiple variants exist for 'llama_cpp'. Use --gpu to specify (cpu, cuda, vulkan, rocm)"
   - Call `registry.activate(name, gpu_variant, version)`

5. **Update `cmd_remove_version`**:
   - Add `--gpu` flag
   - Same auto-infer logic as switch

6. **Update `cmd_update`**:
   - Preserve `gpu_variant` from existing backend info
   - Install new version to same variant's version folder
   - Call `registry.update_version(name, gpu_variant, new_version, ...)`

7. **Update `cmd_all_versions`**:
   - Show variants in output: `llama_cpp [vulkan] (v b8407)`
   - Group by backend name, then variant

8. **No new helpers needed** — callers derive `gpu_variant` from parsed `GpuType` via `gpu_type.variant_folder()` directly. Do NOT add a separate `parse_gpu_variant` function.

**Steps:**
- [ ] Update `cmd_install` to derive `gpu_variant` via `gpu_type.variant_folder()` and use `get_backend_install_path`
- [ ] Update `cmd_list` display format to show `[variant]` badge
- [ ] Update `cmd_remove` with `--gpu` flag
- [ ] Update `cmd_switch` with `--gpu` flag and auto-infer logic
- [ ] Update `cmd_remove_version` with `--gpu` flag
- [ ] Update `cmd_update` to preserve `gpu_variant`
- [ ] Update `cmd_all_versions` to show variants
- [ ] Run `cargo test --package tama-cli commands::backend`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: update CLI commands for gpu_variant support"

**Acceptance criteria:**
- [ ] `tama backend install llama_cpp --gpu vulkan` installs to `backends/llama_cpp/vulkan/<version>/`
- [ ] `tama backend list` shows `[variant]` badge for each backend
- [ ] `tama backend switch llama_cpp b8407 --gpu cuda` works correctly
- [ ] `tama backend switch` auto-infers variant when only one exists
- [ ] `tama backend remove` with `--gpu` removes only that variant

---

## Task 5: Legacy migration logic

**Context:**
Users with existing installations have backends in the old flat structure (`backends/llama_cpp/llama-server`). On first launch after upgrade, the system must detect and migrate these to the new structure.

**Files:**
- Modify: `crates/tama-core/src/backends/mod.rs` (or create new file `backends/migration.rs`)
- Modify: `crates/tama-core/src/backends/registry/registry_ops.rs`
- Modify: `crates/tama-core/src/db/mod.rs` (call migration from open)

**What to implement:**

1. **Create `migrate_legacy_backends` function** in `backends/migration.rs`:
   - Called during `BackendRegistry::open()` after DB migrations run
   - **Idempotent design:** Each record is checked individually — if the path already matches the new pattern, it's skipped. This means a crashed migration can safely retry.
   - Steps:
     a. For each backend in DB:
        - Check if path matches old pattern (binary directly in `backends/<name>/` — i.e., path parent == `backends_dir/<name>/`)
        - If path already matches new pattern (`backends/<type>/<variant>/<version>/`), skip this record
        - If legacy path detected:
          i. Derive `gpu_variant` from DB's `gpu_type` field (use `variant_folder()`)
          ii. If `gpu_type` is `None`: try heuristic (check binary name for "cuda"/"rocm"/"vulkan" hints), default to `"cpu"` with warning log
          iii. Compute new path: `get_backend_install_path(backends_dir, &backend_type, gpu_variant, &version)`
          iv. Check if new path already exists (files were moved but DB update failed) → if so, just update DB record
          v. If new path doesn't exist: create parent directories, move binary (and shared libs) from old to new path
          vi. Update DB record: set `gpu_variant`, update `path`
     b. After all records processed, write marker file `.tama-migration-v2-done` in backends dir

2. **Recovery logic**:
   - If migration crashes mid-way, marker file won't exist
   - On next launch, migration iterates all records again
   - Already-migrated records are skipped (path matches new pattern)
   - Records with files moved but DB not updated: detected by "old path missing, new path exists" → DB is fixed
   - Records with neither old nor new path: log error, skip (user must reinstall)
   - Log all migration actions for debugging

3. **Call from `BackendRegistry::open`**:
   - After `crate::db::open(config_dir)`, call `migrate_legacy_backends(&conn, &backends_dir)`

**Steps:**
- [ ] Create `backends/migration.rs` with `migrate_legacy_backends` function
- [ ] Implement legacy path detection (check if path parent == `backends/<name>/`)
- [ ] Implement gpu_variant derivation from gpu_type with heuristic fallback
- [ ] Implement file move logic (create dirs, move files, update DB)
- [ ] Implement marker file check/write
- [ ] Add rollback detection (old path missing, new path exists → update DB)
- [ ] Call `migrate_legacy_backends` from `BackendRegistry::open()`
- [ ] Add `mod migration` to `backends/mod.rs`
- [ ] Write tests for migration logic (use temp dirs)
- [ ] Run `cargo test --package tama-core backends::migration`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add legacy backend migration from flat to variant structure"

**Acceptance criteria:**
- [ ] Migration detects legacy paths and moves them to new structure
- [ ] Migration derives `gpu_variant` from `gpu_type` field
- [ ] Migration handles `gpu_type = None` with heuristic + warning
- [ ] Marker file prevents re-migration
- [ ] Rollback detection fixes partially-migrated state

---

## Task 6: WebUI updates

**Context:**
The WebUI needs to reflect the new variant-aware backend model. Backend cards show variant info. The install modal is updated to work with variants. Uninstalled backends are hidden (with a collapsed "Available Backends" section).

**Files:**
- Modify: `crates/tama-web/src/components/backend_card.rs`
- Modify: `crates/tama-web/src/components/install_modal.rs`
- Modify: `crates/tama-web/src/api/backends/types.rs` (API DTOs)
- Modify: `crates/tama-web/src/api/backends/install.rs` (install endpoint)
- Modify: `crates/tama-web/src/api/backends/manage.rs` (update/remove/activate endpoints)
- Modify: `crates/tama-web/src/api/backends/list.rs` (list endpoint)

**What to implement:**

1. **Update `BackendCardDto`** in `backend_card.rs`:
   - Add `gpu_variant: String` field
   - Update serialization/deserialization

2. **Update `BackendVersionDto`**:
   - Add `gpu_variant: String` field

3. **Update `BackendCard` component**:
   - Show variant badge: `llama.cpp [Vulkan]`
   - When multiple variants of the same backend exist, show separate cards (one per variant)
   - Group by backend type in the parent component

4. **Update `InstallModal`**:
   - No major changes needed — it already has GPU type selection
   - The backend type is passed in, GPU type is selected by user
   - `gpu_variant` is derived server-side from the selected GPU type

5. **API DTO updates** in `api/backends/types.rs`:
   - Add `gpu_variant: String` to `BackendCardDto`, `BackendInfoDto`, `BackendVersionDto`
   - Both the API types file AND the component DTOs in `backend_card.rs` need this field

6. **API install endpoint** in `api/backends/install.rs`:
   - Derive `gpu_variant` from `gpu_type.variant_folder()` after GPU selection
   - Compute `target_dir` using `get_backend_install_path`
   - Pass `gpu_variant` when constructing `BackendInfo`
   - Pass `gpu_variant` to `registry.add()`

7. **API manage endpoints** in `api/backends/manage.rs`:
   - `update_backend`: preserve `gpu_variant` from existing record
   - `remove_backend`: accept optional `gpu_variant` parameter
   - `remove_backend_version`: accept `gpu_variant` parameter
   - `activate_backend_version`: accept `gpu_variant` parameter

8. **API list endpoint** in `api/backends/list.rs`:
   - Current: iterates `KNOWN_BACKENDS` by type, calls `registry.get(type_)`
   - New: call `registry.list()` to get all active backends (across all variants)
   - Group by backend type, create one card per `(type, variant)` pair
   - Return separate cards for each installed variant

9. **"Available Backends" section**:
   - In the parent component that renders backend cards, add a collapsed section showing uninstalled backend types with "Install" buttons
   - This preserves the discovery path while not cluttering the main view

**Steps:**
- [ ] Add `gpu_variant` field to API DTOs in `api/backends/types.rs`
- [ ] Add `gpu_variant` field to component DTOs in `components/backend_card.rs`
- [ ] Update `BackendCard` component to display variant badge
- [ ] Update `api/backends/install.rs` to derive gpu_variant and use get_backend_install_path
- [ ] Update `api/backends/manage.rs` to pass gpu_variant to registry methods
- [ ] Update `api/backends/list.rs` to use registry.list() and group by type+variant
- [ ] Add "Available Backends" collapsed section in parent component
- [ ] Run `cargo test --package tama-web`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: update WebUI for gpu_variant backend support"

**Acceptance criteria:**
- [ ] Backend cards show `[variant]` badge
- [ ] API responses include `gpu_variant` field
- [ ] Multiple variants of same backend show as separate cards
- [ ] "Available Backends" section shows uninstalled types (collapsed by default)

---

## Verification

After all tasks are complete:

```bash
# Full workspace check
cargo check --workspace
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace

# Manual verification
# 1. Install llama_cpp with CPU: tama backend install llama_cpp --gpu cpu
# 2. Install llama_cpp with Vulkan: tama backend install llama_cpp --gpu vulkan
# 3. List: verify both show with [cpu] and [vulkan] badges
# 4. Switch: tama backend switch llama_cpp <version> --gpu cpu
# 5. Remove variant: tama backend remove llama_cpp --gpu cpu
# 6. Verify folder structure: ls -la ~/.local/share/tama/backends/llama_cpp/
```
