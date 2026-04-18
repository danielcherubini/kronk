# File Size Refactor — Split Large Files into Sub-modules

**Goal:** Split the 10 largest source files (all >400 lines) into smaller, focused sub-modules to improve maintainability and readability.

**Architecture:** Each large file is split by logical concern — DTOs/types go together, endpoints group by resource, CLI commands group by action. All changes are within existing crate directories; no new crates are created.

**Tech Stack:** Rust, cargo workspace (koji-core, koji-cli, koji-web)

---

### Task 1: Split `model.rs` (2,410 lines) into sub-modules

**Context:**
The CLI model command handler (`crates/koji-cli/src/commands/model.rs`) is the largest file at 2,410 lines. It contains helper utilities, multiple independent command handlers (pull, ls, rm, scan, prune, enable, disable, create, update, search, verify, migrate), and tests. Splitting by logical concern makes each handler easier to find and modify.

**Files:**
- Create: `crates/koji-cli/src/commands/model/utils.rs` — helper functions
- Create: `crates/koji-cli/src/commands/model/pull.rs` — cmd_pull + cmd_scan
- Create: `crates/koji-cli/src/commands/model/list_rm.rs` — cmd_ls + cmd_rm
- Create: `crates/koji-cli/src/commands/model/prune.rs` — cmd_prune
- Create: `crates/koji-cli/src/commands/model/enable_disable.rs` — cmd_enable + cmd_disable
- Create: `crates/koji-cli/src/commands/model/create.rs` — cmd_create
- Create: `crates/koji-cli/src/commands/model/update.rs` — cmd_update + cmd_search
- Create: `crates/koji-cli/src/commands/model/verify.rs` — cmd_verify + cmd_verify_existing
- Create: `crates/koji-cli/src/commands/model/migrate.rs` — cmd_migrate
- Modify: `crates/koji-cli/src/commands/model/mod.rs` — declare sub-modules, re-export `run` and `ModelCommands`
- Modify: `crates/koji-cli/src/commands/model.rs` — delete (all code moved to sub-modules)

**What to implement:**

**IMPORTANT:** All `cmd_*` functions must be declared as `pub(super)` (not `fn` or `pub fn`) so that the dispatcher in `model/mod.rs` can call them. The original `cmd_scan` is `pub(crate)` — keep it as `pub(crate)` since it may be called from outside.

1. **`model/utils.rs`** — Move these standalone functions:
   - `manual_timestamp()`
   - `secs_to_datetime(secs: u64)`
   - `unique_quant_key(quants: &HashMap<String, QuantInfo>, base_key: &str, filename: &str)`
   - `format_downloads(n: u64)` — used by cmd_ls

2. **`model/pull.rs`** — Move these functions and their imports:
   - `pub(super) fn cmd_pull(config: &Config, repo_id: &str) -> Result<()>`
   - `pub(crate) fn cmd_scan(config: &Config) -> Result<()>`
   Keep the import block that was used by these functions.

3. **`model/list_rm.rs`** — Move:
   - `pub(super) fn cmd_ls(config: &Config, ...)`
   - `pub(super) fn cmd_rm(config: &Config, model_id: &str) -> Result<()>`

4. **`model/prune.rs`** — Move:
   - `pub(super) fn cmd_prune(config: &Config, dry_run: bool, yes: bool) -> Result<()>`
   - The nested `format_bytes` function inside cmd_prune stays where it is (it's small and only used there).

5. **`model/enable_disable.rs`** — Move:
   - `pub(super) fn cmd_enable(_config: &Config, name: &str) -> Result<()>`
   - `pub(super) fn cmd_disable(_config: &Config, name: &str) -> Result<()>`

6. **`model/create.rs`** — Move:
   - `pub(super) fn cmd_create(config, name, model, quant, profile, backend)`

7. **`model/update.rs`** — Move:
   - `pub(super) fn cmd_update(config, model, check, refresh, yes)`
   - `pub(super) fn cmd_search(config, query, sort, limit, pull)`

8. **`model/verify.rs`** — Move:
   - `pub(super) fn cmd_verify(config, model)`
   - `pub(super) fn cmd_verify_existing(config, model, verbose)`

9. **`model/migrate.rs`** — Move:
   - `pub(super) fn cmd_migrate(config: &Config) -> Result<()>`

10. **`model/mod.rs`** — New module file:
```rust
pub mod utils;
pub mod pull;
pub mod list_rm;
pub mod prune;
pub mod enable_disable;
pub mod create;
pub mod update;
pub mod verify;
pub mod migrate;

pub use crate::cli::ModelCommands;

pub async fn run(config: &koji_core::config::Config, command: ModelCommands) -> anyhow::Result<()> {
    use crate::cli::ModelCommands;
    match command {
        ModelCommands::Pull { repo } => pull::cmd_pull(config, &repo).await,
        ModelCommands::Ls { model, quant, profile } => list_rm::cmd_ls(config, model, quant, profile),
        ModelCommands::Enable { name } => enable_disable::cmd_enable(config, &name),
        ModelCommands::Disable { name } => enable_disable::cmd_disable(config, &name),
        ModelCommands::Create { name, model, quant, profile, backend } => create::cmd_create(config, name, &model, quant, profile, backend).await,
        ModelCommands::Rm { model } => list_rm::cmd_rm(config, &model),
        ModelCommands::Scan => pull::cmd_scan(config),
        ModelCommands::Prune { dry_run, yes } => prune::cmd_prune(config, dry_run, yes),
        ModelCommands::Update { model, check, refresh, yes } => update::cmd_update(config, model, check, refresh, yes).await,
        ModelCommands::Search { query, sort, limit, pull } => update::cmd_search(config, &query, &sort, limit, pull).await,
        ModelCommands::Verify { model } => verify::cmd_verify(config, model).await,
        ModelCommands::VerifyExisting { model, verbose } => verify::cmd_verify_existing(config, model, verbose).await,
        ModelCommands::Migrate => migrate::cmd_migrate(config),
    }
}
```

11. **Tests** — Move the `#[cfg(test)]` module from `model.rs` into the appropriate sub-module files:
    - `test_secs_to_datetime_*` and `test_manual_timestamp_format/year` → `utils.rs`
    - `test_unique_quant_key_*` → `utils.rs`
    - `test_format_downloads_*` → `utils.rs`
    - Scan integration tests (`test_scan_adds_new_files`, `test_scan_removes_missing_files`, `test_scan_removes_ghost_configs`, `test_scan_empty_dir_removes_everything`) and the `setup_test_env` helper → `pull.rs` (since they test cmd_scan)
    - Other command-specific tests stay in their respective sub-module files

12. **Delete** `model.rs` after all code is moved. The module entry point is now `model/mod.rs`.

**Steps:**
- [ ] Create `crates/koji-cli/src/commands/model/utils.rs` with: `manual_timestamp`, `secs_to_datetime`, `unique_quant_key`, `format_downloads`, and their tests
- [ ] Create `crates/koji-cli/src/commands/model/pull.rs` with: `cmd_pull`, `cmd_scan` and their imports
- [ ] Create `crates/koji-cli/src/commands/model/list_rm.rs` with: `cmd_ls`, `cmd_rm` and their imports
- [ ] Create `crates/koji-cli/src/commands/model/prune.rs` with: `cmd_prune`
- [ ] Create `crates/koji-cli/src/commands/model/enable_disable.rs` with: `cmd_enable`, `cmd_disable`
- [ ] Create `crates/koji-cli/src/commands/model/create.rs` with: `cmd_create`
- [ ] Create `crates/koji-cli/src/commands/model/update.rs` with: `cmd_update`, `cmd_search`
- [ ] Create `crates/koji-cli/src/commands/model/verify.rs` with: `cmd_verify`, `cmd_verify_existing`
- [ ] Create `crates/koji-cli/src/commands/model/migrate.rs` with: `cmd_migrate`
- [ ] Create `crates/koji-cli/src/commands/model/mod.rs` with module declarations and the `run` dispatcher
- [ ] Delete `crates/koji-cli/src/commands/model.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --package koji -- commands::model` to verify tests pass
- [ ] Commit with message: "refactor(cli): split model.rs into focused sub-modules"

**Acceptance criteria:**
- [ ] `crates/koji-cli/src/commands/model.rs` no longer exists
- [ ] `crates/koji-cli/src/commands/model/mod.rs` is the module entry point with `pub mod` declarations
- [ ] Each sub-module file is under 400 lines
- [ ] `cargo build --workspace` succeeds
- [ ] All model-related tests pass: `cargo test --package koji -- commands::model`

---

### Task 2: Split `backends.rs` (2,174 lines) into sub-modules

**Context:**
The web API backends endpoint file (`crates/koji-web/src/api/backends.rs`) is 2,174 lines. It contains DTOs/types, trait implementations, a capabilities cache, and many endpoint handlers for backend CRUD + lifecycle operations. Splitting separates data types from business logic.

**Files:**
- Create: `crates/koji-web/src/api/backends/types.rs` — all DTOs, From impls, UpdateStatusDto, ActiveJobDto, JobAdapter, CapabilitiesCache
- Create: `crates/koji-web/src/api/backends/install.rs` — install_backend, remove_backend endpoints
- Create: `crates/koji-web/src/api/backends/manage.rs` — update_backend, remove_backend_version, check_backend_updates, list_backend_versions, activate_backend_version, update_backend_default_args endpoints
- Create: `crates/koji-web/src/api/backends/jobs.rs` — get_job, job_events_sse, JobSnapshotDto
- Create: `crates/koji-web/src/api/backends/capabilities.rs` — system_capabilities endpoint
- Create: `crates/koji-web/src/api/backends/list.rs` — list_backends endpoint
- Modify: `crates/koji-web/src/api/backends/mod.rs` — declare sub-modules, re-export all public items
- Modify: `crates/koji-web/src/api/backends.rs` — delete (all code moved to sub-modules)

**What to implement:**

1. **`backends/types.rs`** — Move ALL public types and their impls:
   - `BackendListResponse` struct
   - `BackendCardDto` struct + its impl block
   - `impl From<koji_core::backends::BackendInfo> for BackendInfoDto`
   - `GpuTypeDto` enum
   - `impl From<&koji_core::gpu::GpuType> for GpuTypeDto`
   - `BackendSourceDto` enum
   - `impl From<&koji_core::backends::BackendSource> for BackendSourceDto`
   - `UpdateStatusDto` struct + `is_default()` impl
   - `ActiveJobDto` struct
   - `job_to_active_dto(j: &crate::jobs::Job) -> ActiveJobDto` function
   - `CapabilitiesDto` struct
   - `InstallRequest` struct
   - `InstallResponse` struct
   - `DeleteResponse` struct
   - `BackendVersionDto` struct
   - `BackendVersionsResponse` struct
   - `ActivateRequest` struct
   - `ActivateResponse` struct
   - `CheckUpdatesResponse` struct
   - `JobSnapshotDto` struct
   - `UpdateDefaultArgsRequest` struct
   - `ProgressSink` trait (imported from koji_core, only move the `impl ProgressSink for JobAdapter` block)
   - `JobAdapter` struct + its impls (`ProgressSink`, any other)
   - `CapabilitiesCache` struct + `new()` and `Default` impl

2. **`backends/install.rs`** — Move these endpoint handlers:
   - `install_backend` endpoint handler
   - `remove_backend` endpoint handler
   (Include the imports needed by these functions)

3. **`backends/manage.rs`** — Move these endpoint handlers:
   - `update_backend` endpoint handler
   - `remove_backend_version` endpoint handler
   - `check_backend_updates` endpoint handler
   - `list_backend_versions` endpoint handler
   - `activate_backend_version` endpoint handler
   - `update_backend_default_args` endpoint handler
   (Include the imports needed by these functions)

4. **`backends/jobs.rs`** — Move:
   - `get_job` endpoint handler
   - `job_events_sse` endpoint handler
   - `JobSnapshotDto` struct (already in types.rs, just re-export from there)

5. **`backends/capabilities.rs`** — Move:
   - `system_capabilities` endpoint handler

6. **`backends/list.rs`** — Move:
   - `list_backends` endpoint handler

5. **`backends/mod.rs`** — New module file:
```rust
pub mod types;
pub mod install;
pub mod manage;
pub mod jobs;
pub mod capabilities;
pub mod list;

// Re-export all public types and functions for backward compatibility
pub use types::*;
pub use install::*;
pub use manage::*;
pub use jobs::*;
pub use capabilities::*;
pub use list::*;
```

6. **Tests** — Move the `#[cfg(test)]` module from `backends.rs` into `types.rs` (since all tests test DTOs and `job_to_active_dto`).

7. **Delete** `backends.rs` after all code is moved. The module entry point is now `backends/mod.rs`.

**Steps:**
- [ ] Create `crates/koji-web/src/api/backends/types.rs` with all DTO types, From impls, JobAdapter, CapabilitiesCache, and tests
- [ ] Create `crates/koji-web/src/api/backends/install.rs` with install/uninstall/restore endpoints
- [ ] Create `crates/koji-web/src/api/backends/manage.rs` with enable/disable/update/version/list endpoints
- [ ] Create `crates/koji-web/src/api/backends/capabilities.rs` with capabilities endpoint
- [ ] Create `crates/koji-web/src/api/backends/mod.rs` with module declarations and re-exports
- [ ] Delete `crates/koji-web/src/api/backends.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --package koji-web` to verify tests pass
- [ ] Commit with message: "refactor(web): split backends.rs into focused sub-modules"

**Acceptance criteria:**
- [ ] `crates/koji-web/src/api/backends.rs` no longer exists
- [ ] `crates/koji-web/src/api/backends/mod.rs` is the module entry point
- [ ] Each sub-module file is under 400 lines
- [ ] `cargo build --workspace` succeeds
- [ ] All web API tests pass: `cargo test --package koji-web`

---

### Task 3: Split `api.rs` (1,813 lines) model endpoints into sub-modules

**Context:**
The web API model endpoints file (`crates/koji-web/src/api.rs`) is 1,813 lines. It's the main module file for the `api` module in koji-web. It contains model CRUD endpoints, helper functions, refresh/verify endpoints, and tests. The api module already has sub-modules (backends, backup, self_update, updates) but api.rs itself is still a single file.

**Files:**
- Create: `crates/koji-web/src/api/models/info.rs` — list_models, get_model, helper functions (`resolve_model_id`, `model_entry_json`, `load_repo_db_meta`, `RepoDbMeta`)
- Create: `crates/koji-web/src/api/models/crud.rs` — create_model, update_model, rename_model, delete_model, delete_quant, ModelBody, CreateModelBody, RenameBody, apply_model_body
- Create: `crates/koji-web/src/api/models/files.rs` — refresh_model_metadata, verify_model_files, file_record_json
- Create: `crates/koji-web/src/api/models/mod.rs` — module declarations, re-exports
- Modify: `crates/koji-web/src/api.rs` — keep non-model items at top level, add `pub mod models;` and `pub use models::*;`

**What to implement:**

1. **`models/info.rs`** — Move these functions:
   - `resolve_model_id(id_str: &str, conn: &rusqlite::Connection) -> anyhow::Result<Option<i64>>`
   - `load_repo_db_meta(config_dir: &std::path::Path, model_id: i64) -> RepoDbMeta`
   - `RepoDbMeta` struct (private)
   - `model_entry_json(...)` — the large serialization function (lines ~370-619)
   - `list_models` endpoint handler
   - `get_model` endpoint handler

2. **`models/crud.rs`** — Move these types and functions:
   - `ModelBody` struct
   - `CreateModelBody` struct
   - `RenameBody` struct
   - `apply_model_body(body: ModelBody, existing: Option<...>) -> ModelConfig`
   - `update_model` endpoint handler
   - `create_model` endpoint handler
   - `rename_model` endpoint handler
   - `delete_model` endpoint handler
   - `delete_quant` endpoint handler

3. **`models/files.rs`** — Move:
   - `file_record_json(rec: &ModelFileRecord) -> serde_json::Value`
   - `refresh_model_metadata` endpoint handler
   - `verify_model_files` endpoint handler

4. **`models/mod.rs`** — New module file:
```rust
pub mod info;
pub mod crud;
pub mod files;

pub use info::*;
pub use crud::*;
pub use files::*;
```

5. **Tests** — Move the `#[cfg(test)]` module from `api.rs` into `crud.rs` (since all tests test `apply_model_body`).

6. **`api.rs`** — Replace with the following structure:
```rust
// ── Non-model items stay at top level of api/ ────────────────────────────────

pub mod models;

// Re-export for backward compatibility
pub use models::*;

// ── Log endpoint ─────────────────────────────────────────────────────────────
pub struct LogsQuery {
    pub name: Option<String>,
    #[serde(default)]
    pub lines: usize,
}

fn default_lines() -> usize { 50 }

pub async fn get_logs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // ... (copy from api.rs)
}

// ── Config endpoints ─────────────────────────────────────────────────────────
pub struct ConfigBody {
    pub name: String,
    pub value: serde_json::Value,
}

pub struct StructuredConfigBody {
    pub name: String,
    pub data: serde_json::Value,
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // ... (copy from api.rs)
}

pub async fn save_config(State(state): State<Arc<AppState>>, Json(body): Json<ConfigBody>) -> impl IntoResponse {
    // ... (copy from api.rs)
}

pub async fn get_structured_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // ... (copy from api.rs)
}

pub async fn save_structured_config(State(state): State<Arc<AppState>>, Json(body): Json<StructuredConfigBody>) -> impl IntoResponse {
    // ... (copy from api.rs)
}

// ── Shared helpers (used by both model and non-model endpoints) ──────────────
fn load_config_from_state(state: &Arc<AppState>) -> anyhow::Result<(Config, PathBuf)> {
    // ... (copy from api.rs)
}

async fn sync_proxy_config(state: &AppState, new_config: Config) {
    // ... (copy from api.rs)
}

async fn trigger_proxy_reload(state: &AppState) -> Result<(), (StatusCode, serde_json::Value)> {
    // ... (copy from api.rs)
}
```

Key decision: `load_config_from_state`, `sync_proxy_config`, and `trigger_proxy_reload` stay at the top level of `api.rs` because they are used by both model endpoints (in the models/ sub-module) and non-model endpoints. They are imported from `super::` within the models/ sub-modules.

**Steps:**
- [ ] Create `crates/koji-web/src/api/models/info.rs` with info-related functions and list/get endpoints
- [ ] Create `crates/koji-web/src/api/models/crud.rs` with types, apply_model_body, and CRUD endpoints + tests
- [ ] Create `crates/koji-web/src/api/models/files.rs` with file operations endpoints
- [ ] Create `crates/koji-web/src/api/models/mod.rs` with module declarations and re-exports
- [ ] Replace `crates/koji-web/src/api.rs` with module declaration
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --package koji-web` to verify tests pass
- [ ] Commit with message: "refactor(web): split api.rs model endpoints into sub-modules"

**Acceptance criteria:**
- [ ] `crates/koji-web/src/api.rs` contains module declarations + non-model items (LogsQuery, ConfigBody, StructuredConfigBody, get_logs, get_config, save_config, get_structured_config, save_structured_config, load_config_from_state, sync_proxy_config, trigger_proxy_reload)
- [ ] `crates/koji-web/src/api/models/mod.rs` is the module entry point
- [ ] Each sub-module file is under 400 lines
- [ ] `cargo build --workspace` succeeds
- [ ] All web API tests pass: `cargo test --package koji-web`

---

### Task 4: Split `gpu.rs` (887 lines) into sub-modules

**Context:**
The GPU utilities file (`crates/koji-core/src/gpu.rs`) is 887 lines. It contains GPU detection (NVIDIA, AMD), VRAM querying, system metrics collection, build prerequisite detection, CUDA version detection, ROCm target detection, and context size suggestions. These are distinct concerns that should be separated.

**Files:**
- Create: `crates/koji-core/src/gpu/vram.rs` — VramInfo struct, query_vram, query_nvidia_vram, query_amd_vram, available_mib/bytes/total_bytes methods
- Create: `crates/koji-core/src/gpu/system.rs` — SystemMetrics, collect_system_metrics, query_gpu_utilization (NVIDIA + AMD)
- Create: `crates/koji-core/src/gpu/detect.rs` — detect_build_prerequisites, BuildPrerequisites, detect_cuda_version (+ nvcc/nvidia_smi helpers), detect_amdgpu_targets, parse_rocminfo_gfx_names, suggest_context_sizes
- Modify: `crates/koji-core/src/gpu/mod.rs` — declare sub-modules, re-export public items
- Modify: `crates/koji-core/src/gpu.rs` — delete (all code moved to sub-modules)

**What to implement:**

1. **`gpu/vram.rs`** — Move:
   - `VramInfo` struct + its impl block (available_mib, available_bytes, total_bytes)
   - `query_vram() -> Option<VramInfo>`
   - `query_nvidia_vram() -> Option<VramInfo>`
   - `query_amd_vram() -> Option<VramInfo>`

2. **`gpu/system.rs`** — Move:
   - `SystemMetrics` struct
   - `MetricSample` struct
   - `ModelStatus` struct
   - `collect_system_metrics_with(sys: &mut System) -> SystemMetrics`
   - `collect_system_metrics() -> SystemMetrics`
   - `query_gpu_utilization() -> Option<u8>`
   - `query_nvidia_gpu_utilization() -> Option<u8>`
   - `query_amd_gpu_utilization() -> Option<u8>`

3. **`gpu/detect.rs`** — Move:
   - `GpuType` enum
   - `BuildPrerequisites` struct
   - `ContextSuggestion` struct
   - `DEFAULT_CUDA_VERSION` const
   - `detect_build_prerequisites() -> BuildPrerequisites`
   - `detect_cuda_version() -> Option<String>`
   - `detect_cuda_version_nvcc() -> Option<String>`
   - `detect_cuda_version_nvidia_smi() -> Option<String>`
   - `detect_amdgpu_targets() -> Vec<String>`
   - `parse_rocminfo_gfx_names(stdout: &str) -> Vec<String>`
   - `suggest_context_sizes(model_bytes: u64, vram: Option<&VramInfo>) -> Vec<ContextSuggestion>`

4. **`gpu/mod.rs`** — New module file:
```rust
pub mod vram;
pub mod system;
pub mod detect;

pub use vram::{VramInfo, query_vram};
pub use system::{SystemMetrics, MetricSample, ModelStatus, collect_system_metrics, collect_system_metrics_with};
pub use detect::{
    GpuType, BuildPrerequisites, ContextSuggestion, DEFAULT_CUDA_VERSION,
    detect_build_prerequisites, detect_cuda_version, detect_amdgpu_targets,
    parse_rocminfo_gfx_names, suggest_context_sizes,
};
```

5. **Tests** — Move the `#[cfg(test)]` module from `gpu.rs` into appropriate sub-modules:
   - VRAM tests (`test_vram_info_available`, `test_vram_info_zero_available`, `test_vram_info_full`) → `vram.rs`
   - System metrics tests (`test_collect_system_metrics`, `test_collect_system_metrics_with_reuses_system`) → `system.rs`
   - Detection tests (`test_detect_build_prerequisites`, `test_detect_cuda_version_*`, `test_default_cuda_version_is_set`, `test_default_cuda_version_format`, `test_detect_amdgpu_targets_*`, `test_parse_rocminfo_gfx_names_*`) → `detect.rs`
   - Context size tests (`test_suggest_context_sizes_*`) → `detect.rs`

6. **Delete** `gpu.rs` after all code is moved. The module entry point is now `gpu/mod.rs`.

**Steps:**
- [ ] Create `crates/koji-core/src/gpu/vram.rs` with VramInfo, query functions, and VRAM tests
- [ ] Create `crates/koji-core/src/gpu/system.rs` with SystemMetrics, system metrics collection, GPU utilization, and related tests
- [ ] Create `crates/koji-core/src/gpu/detect.rs` with detection functions and related tests
- [ ] Create `crates/koji-core/src/gpu/mod.rs` with module declarations and re-exports
- [ ] Delete `crates/koji-core/src/gpu.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --package koji-core -- gpu` to verify tests pass
- [ ] Commit with message: "refactor(core): split gpu.rs into focused sub-modules"

**Acceptance criteria:**
- [ ] `crates/koji-core/src/gpu.rs` no longer exists
- [ ] `crates/koji-core/src/gpu/mod.rs` is the module entry point
- [ ] Each sub-module file is under 400 lines
- [ ] `cargo build --workspace` succeeds
- [ ] All GPU-related tests pass: `cargo test --package koji-core -- gpu`

---

### Task 5: Split remaining large files (source.rs, backend.rs, model_editor/mod.rs)

**Context:**
Three remaining files exceed 400 lines that haven't been addressed yet:
- `crates/koji-core/src/backends/installer/source.rs` (1,018 lines) — installer source detection
- `crates/koji-cli/src/commands/backend.rs` (913 lines) — CLI backend commands
- `crates/koji-web/src/pages/model_editor/mod.rs` (848 lines) — Leptos page component

**Files:**

For `source.rs`:
- Create: `crates/koji-core/src/backends/installer/source/detect.rs` — `find_llvm_bin`, `find_vcvarsall`, `detect_hip_env`, `hip_env_from_hipconfig_output`
- Create: `crates/koji-core/src/backends/installer/source/build.rs` — `emit`, `build_cmake_args`
- Create: `crates/koji-core/src/backends/installer/source/install.rs` — `install_from_source`, `clone_repository`, `try_clone_latest_tag`, `configure_cmake`, `configure_cmake_windows`, `build_cmake`, `install_binary`
- Modify: `crates/koji-core/src/backends/installer/source/mod.rs` — module declarations, re-exports
- Modify: `crates/koji-core/src/backends/installer/source.rs` — delete

For `backend.rs`:
- Create: `crates/koji-cli/src/commands/backend/parse.rs` — `parse_backend_type`, `parse_gpu_type`, `registry_config_dir`, `current_unix_timestamp`
- Modify: `crates/koji-cli/src/commands/backend/mod.rs` — module declarations, re-exports
- Modify: `crates/koji-cli/src/commands/backend.rs` — keep command handlers here (they're the main concern), move parsing utilities to parse.rs

For `model_editor/mod.rs`:
- Create: `crates/koji-web/src/pages/model_editor/sections.rs` — Section enum, section rendering logic
- Create: `crates/koji-web/src/pages/model_editor/form.rs` — form components and state management
- Modify: `crates/koji-web/src/pages/model_editor/mod.rs` — module declarations, re-export `ModelEditor`

**What to implement:**

1. **`source/detect.rs`** — Move:
   - `find_llvm_bin() -> Option<PathBuf>`
   - `find_vcvarsall() -> Option<PathBuf>`
   - `hip_env_from_hipconfig_output(output: &str) -> Option<(String, String)>`
   - `detect_hip_env() -> Option<(String, String)>`

2. **`source/build.rs`** — Move:
   - `emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>)`
   - `build_cmake_args(...)` function

3. **`source/mod.rs`** — New module file:
```rust
pub mod detect;
pub mod build;
pub mod install;

pub use install::install_from_source;
```

4. **Tests for source.rs** — Move tests into the appropriate sub-modules:
   - `test_ik_llama_*`, `test_llama_cpp_*`, `test_rocm_*`, `test_non_rocm_*`, `test_hip_env_*` → `build.rs` (since they test cmake args and build config)
   - `copy_shared_libs` test helper → `install.rs`
   - `make_options` test helper → `build.rs`

5. **Delete** `source.rs` after all code is moved.

5. **`backend/parse.rs`** — Move:
   - `parse_backend_type(s: &str) -> Result<BackendType>`
   - `parse_gpu_type(gpu_str: &str) -> Result<GpuType>`
   - `registry_config_dir() -> Result<PathBuf>`
   - `current_unix_timestamp() -> i64`

6. **Tests for backend.rs** — Move parsing tests into `backend/parse.rs`:
   - All `test_parse_backend_type_*` tests → `parse.rs`
   - `test_parse_gpu_type_*` tests → `parse.rs` (if any)
   - `test_current_unix_timestamp_positive` → `parse.rs`

7. **`model_editor/sections.rs`** — Move the `Section` enum and its impl (name, icon methods). The Section enum is used by the main component for navigation.

8. **`model_editor/mod.rs`** — After moving Section, the remaining `ModelEditor` component is ~750 lines of tightly coupled reactive state and view! macro. For this refactor, move just the Section type out. The main component stays in mod.rs but will be under 400 lines after removing Section (the component itself is large but it's a single Leptos component — splitting Leptos components further requires careful signal ownership decisions that are beyond this refactor's scope).

Note: `model_editor/mod.rs` already has sub-modules (`api`, `extra_args_form`, `general_form`, `quants_vision_form`, `sampling_form`, `types`). The main `ModelEditor` component is the remaining monolith. Moving just Section brings mod.rs under 400 lines.

9. **Tests for model_editor** — There are no tests in this file.

**Steps:**
- [ ] Create `crates/koji-core/src/backends/installer/source/detect.rs` with detection functions
- [ ] Create `crates/koji-core/src/backends/installer/source/build.rs` with emit and build_cmake_args
- [ ] Create `crates/koji-core/src/backends/installer/source/install.rs` with install_from_source, clone_repository, try_clone_latest_tag, configure_cmake, configure_cmake_windows, build_cmake, install_binary
- [ ] Create `crates/koji-core/src/backends/installer/source/mod.rs` with module declarations and re-exports (re-export `install_from_source`)
- [ ] Delete `crates/koji-core/src/backends/installer/source.rs`
- [ ] Create `crates/koji-cli/src/commands/backend/parse.rs` with parsing utilities
- [ ] Update `crates/koji-cli/src/commands/backend/mod.rs` to declare parse submodule
- [ ] Move parsing functions from `backend.rs` to `backend/parse.rs`
- [ ] Create `crates/koji-web/src/pages/model_editor/sections.rs` with Section enum
- [ ] Update `crates/koji-web/src/pages/model_editor/mod.rs` to import Section from sections module
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace` to verify compilation
- [ ] Run `cargo test --workspace` to verify all tests pass
- [ ] Commit with message: "refactor: split remaining large files into sub-modules"

**Acceptance criteria:**
- [ ] No source file exceeds 400 lines (excluding `target/`)
- [ ] `cargo build --workspace` succeeds
- [ ] All tests pass: `cargo test --workspace`

---

### Task 6: Verify and clean up

**Context:**
After all splits are complete, verify that no file exceeds 400 lines, run the full test suite, and ensure code formatting is correct.

**Files:**
- Verify: All source files in `crates/koji-core/`, `crates/koji-cli/`, `crates/koji-web/`

**What to implement:**
- Run the verification script (same command used at the start)
- Fix any remaining files over 400 lines
- Run `cargo clippy --workspace -- -D warnings`
- Run `cargo test --workspace` one final time

**Steps:**
- [ ] Run `find crates/ -name "*.rs" | xargs wc -l | sort -rn | head -20` to verify no file exceeds 400 lines
- [ ] Fix any remaining large files if found
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "chore: verify post-refactor file sizes and run full test suite"

**Acceptance criteria:**
- [ ] No source file exceeds 400 lines
- [ ] `cargo clippy --workspace -- -D warnings` passes with no warnings
- [ ] `cargo test --workspace` passes all tests
- [ ] `cargo build --release --workspace` succeeds
