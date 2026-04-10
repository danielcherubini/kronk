# Split Large Files Plan

**Goal:** Refactor koji-core files over 1000 LOC into smaller, logical submodules using `mod.rs` + submodule files pattern, improving readability and maintainability.

**Architecture:** Each large file becomes a directory with a `mod.rs` that re-exports public items. Submodule files contain the actual implementation grouped by domain. External callers see no API change — all `pub use` re-exports remain identical.

**Tech Stack:** Rust, axum, serde, rusqlite, anyhow

---

### Task 1: Split `db/queries.rs` (1343 LOC) into submodules

**Context:**
`queries.rs` is 1343 lines with clear section comments (`// ---`) separating domain groups: types, model queries, active model queries, backend queries, metrics queries, and tests. This is the cleanest split — the file already has natural boundaries and the module changes from a flat file to a directory with 5 submodule files + `mod.rs`.

The `db/mod.rs` currently declares `pub mod queries;` and all external code references items as `crate::db::queries::ItemName` (or `koji_core::db::queries::ItemName` from other crates). After the split, `db/mod.rs` will keep `pub mod queries;` and `queries/mod.rs` will re-export everything, so all external references continue to work unchanged.

**Files:**
- Create: `crates/koji-core/src/db/queries/mod.rs`
- Create: `crates/koji-core/src/db/queries/types.rs`
- Create: `crates/koji-core/src/db/queries/model_queries.rs`
- Create: `crates/koji-core/src/db/queries/active_model_queries.rs`
- Create: `crates/koji-core/src/db/queries/backend_queries.rs`
- Create: `crates/koji-core/src/db/queries/metrics_queries.rs`
- Delete: `crates/koji-core/src/db/queries.rs`

**What to implement:**

1. `types.rs` — Move these struct definitions from `queries.rs` lines 13-63:
   - `ModelPullRecord`, `ModelFileRecord`, `DownloadLogEntry`, `ActiveModelRecord`
   - All are `pub struct` with `#[derive(Debug, Clone)]`
   - Keep their doc comments

2. `model_queries.rs` — Move these functions from lines 73-237:
   - `upsert_model_pull`, `get_model_pull`, `upsert_model_file`, `update_verification`, `get_model_files`, `log_download`, `delete_model_records`
   - All take `&Connection` as first arg (except `upsert_model_file` which also takes struct fields)
   - Import `use super::types::{ModelPullRecord, ModelFileRecord, DownloadLogEntry};`
   - Import `use anyhow::Result;` and `use rusqlite::Connection;`

3. `active_model_queries.rs` — Move from lines 244-337:
   - `insert_active_model`, `remove_active_model`, `get_active_models`, `clear_active_models`, `touch_active_model`, `rename_active_model`
   - Import `use super::types::ActiveModelRecord;`
   - Import `use anyhow::Result;` and `use rusqlite::Connection;`

4. `backend_queries.rs` — Move from lines 317-517:
   - `BackendInstallationRecord` struct (lines 323-343), `insert_backend_installation`, `get_active_backend`, `list_active_backends`, `list_backend_versions`, `get_backend_by_version`, `delete_backend_installation`, `delete_all_backend_versions`
   - Import `use anyhow::Result;` and `use rusqlite::Connection;` (no `bail` needed in these functions)

5. `metrics_queries.rs` — Move from lines 499-600:
   - `SystemMetricsRow` struct (lines 505-517), `insert_system_metric`, `get_system_metrics_since`, `get_recent_system_metrics`
   - Import `use anyhow::{bail, Result};` and `use rusqlite::Connection;` (note: `bail!` is used by `get_recent_system_metrics`)

6. `mod.rs` — Re-export everything:
   ```rust
   mod types;
   mod model_queries;
   mod active_model_queries;
   mod backend_queries;
   mod metrics_queries;

   pub use types::*;
   pub use model_queries::*;
   pub use active_model_queries::*;
   pub use backend_queries::*;
   pub use metrics_queries::*;

   #[cfg(test)]
   mod tests;
   ```

7. Tests (lines 607-1343) — Move to `crates/koji-core/src/db/queries/tests.rs` as `mod tests;` in `mod.rs`. Tests import from `super::*` which resolves correctly through the re-exports.

**Steps:**
- [ ] Create directory `crates/koji-core/src/db/queries/`
- [ ] Create `types.rs` with struct definitions from lines 13-63 of `queries.rs`
- [ ] Create `model_queries.rs` with functions from lines 73-237
- [ ] Create `active_model_queries.rs` with functions from lines 244-337
- [ ] Create `backend_queries.rs` with struct + functions from lines 317-517
- [ ] Create `metrics_queries.rs` with struct + functions from lines 499-600
- [ ] Create `tests.rs` with all test functions from lines 607-1343
- [ ] Create `mod.rs` with module declarations and `pub use` re-exports
- [ ] Delete `crates/koji-core/src/db/queries.rs`
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix import paths and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: split db/queries.rs into domain submodules"

**Acceptance criteria:**
- [ ] All existing tests pass without modification to test logic
- [ ] All external references (`crate::db::queries::ItemName`) still resolve correctly
- [ ] No clippy warnings
- [ ] Each submodule file is under 300 LOC

---

### Task 2: Split `proxy/koji_handlers.rs` (1433 LOC) into submodules

**Context:**
`koji_handlers.rs` is 1433 lines containing handler functions for the Koji management API. The file has a natural split into three domains: model lifecycle endpoints (list/get/load/unload), pull/download endpoints (pull model, get pull job, pull job stream), and system endpoints (health, HF quants, restart, metrics stream). The `QuantEntry`, `QuantDownloadSpec`, `PullRequest`, `PullResponse`, `ModelResponse`, `RestartResponse` structs and `SystemHealthResponse` are shared types used across handlers. Note: `is_safe_path_component` is used in both `spawn_download_job` (pull) and `handle_hf_list_quants` (system), so it must live in `types.rs` as `pub(super)`. The `CONFIG_WRITE_LOCK` static and `spawn_download_job` private helper are used only by pull-related handlers.

The router at `proxy/server/router.rs` imports 10 handler functions via `use crate::proxy::koji_handlers::{...}`. After the split, `koji_handlers/mod.rs` will re-export all these, so the router import stays identical.

**Files:**
- Create: `crates/koji-core/src/proxy/koji_handlers/mod.rs`
- Create: `crates/koji-core/src/proxy/koji_handlers/types.rs`
- Create: `crates/koji-core/src/proxy/koji_handlers/models.rs`
- Create: `crates/koji-core/src/proxy/koji_handlers/pull.rs`
- Create: `crates/koji-core/src/proxy/koji_handlers/system.rs`
- Delete: `crates/koji-core/src/proxy/koji_handlers.rs`

**What to implement:**

1. `types.rs` — Shared types and constants from lines 1-86:
   - Imports: `axum`, `serde`, `crate::gpu::VramInfo`, `crate::proxy::{pull_jobs, ProxyState}`
   - `MAX_CONCURRENT_PULLS` const (line 23)
   - `CONFIG_WRITE_LOCK` static (line 26) — must be `pub(super)` since `pull.rs` uses it
   - `is_safe_path_component` function (line 258) — must be `pub(super)` since both `pull.rs` and `system.rs` use it
   - `QuantEntry`, `QuantDownloadSpec`, `PullRequest`, `PullResponse`, `ModelResponse`, `RestartResponse` structs
   - Note: `SystemHealthResponse` (line 977) should go in `system.rs` since it's only used there

2. `models.rs` — Model lifecycle handlers from lines 88-265:
   - `handle_koji_list_models` (lines 88-110)
   - `handle_koji_get_model` (lines 112-165)
   - `handle_koji_load_model` (lines 167-210)
   - `handle_koji_unload_model` (lines 212-256)
   - Import from `super::types::ModelResponse`
   - Import `crate::proxy::ProxyState`, axum extractors

3. `pull.rs` — Pull/download handlers from lines 258-975:
   - `spawn_download_job` (lines 269-647) — private async helper
   - `_setup_model_after_pull_with_config` — must be `pub(crate)` so tests in `tests.rs` can call it
   - `setup_model_after_pull` — must be `pub(crate)` so tests can call it
   - `handle_koji_pull_model` (lines 649-884)
   - `handle_koji_get_pull_job` (lines 886-932)
   - `handle_pull_job_stream` (lines 934-975)
   - Import `super::types::{MAX_CONCURRENT_PULLS, CONFIG_WRITE_LOCK, PullRequest, PullResponse, QuantEntry, QuantDownloadSpec, is_safe_path_component}`
   - Import `futures_util::stream;`, `std::convert::Infallible` (used by `handle_pull_job_stream`)
   - Import all crate dependencies used by these functions

4. `system.rs` — System handlers from lines 977-1433 (excluding tests, which stay in mod.rs):
   - `SystemHealthResponse` struct (lines 977-987)
   - `handle_koji_system_health` (lines 989-1009)
   - `handle_hf_list_quants` (lines 1011-1189) — uses `is_safe_path_component` from `types.rs`
   - `handle_koji_system_restart` (lines 1191-1217)
   - `handle_system_metrics_stream` (lines 1219-1243)
   - Import `super::types::{QuantEntry, RestartResponse, is_safe_path_component}`
   - Import `async_stream` (used by `handle_system_metrics_stream`), `std::convert::Infallible` and `futures_util::Stream` (for SSE return types)
   - Note: `SystemHealthResponse` is only used in `handle_koji_system_health`, keep it in this file

5. `mod.rs` — Re-exports:
   ```rust
   mod types;
   mod models;
   mod pull;
   mod system;

   pub use types::{QuantEntry, QuantDownloadSpec, PullRequest, PullResponse, ModelResponse, RestartResponse, MAX_CONCURRENT_PULLS};
   pub use models::{handle_koji_list_models, handle_koji_get_model, handle_koji_load_model, handle_koji_unload_model};
   pub use pull::{handle_koji_pull_model, handle_koji_get_pull_job, handle_pull_job_stream};
   pub use system::{handle_koji_system_health, handle_hf_list_quants, handle_koji_system_restart, handle_system_metrics_stream};

   #[cfg(test)]
   mod tests;
   ```

   Note: `CONFIG_WRITE_LOCK`, `is_safe_path_component`, `_setup_model_after_pull_with_config`, and `setup_model_after_pull` are NOT re-exported — they remain `pub(super)` or `pub(crate)` within the module tree. Only public handler functions and types need re-exports.

6. Tests (lines 1245-1433) — Move to `crates/koji-core/src/proxy/koji_handlers/tests.rs`. These tests use `crate::proxy::ProxyState`, `crate::config::Config`, etc. Tests that call `_setup_model_after_pull_with_config` directly must import it explicitly with `use super::pull::_setup_model_after_pull_with_config;` since glob imports (`super::*`) from `mod.rs` re-exports do NOT bring in `pub(crate)` sibling module items.

**Steps:**
- [ ] Create directory `crates/koji-core/src/proxy/koji_handlers/`
- [ ] Create `types.rs` with shared types and constants
- [ ] Create `models.rs` with model lifecycle handlers
- [ ] Create `pull.rs` with pull/download handlers and private helpers
- [ ] Create `system.rs` with system handlers + `SystemHealthResponse`
- [ ] Create `mod.rs` with module declarations and `pub use` re-exports
- [ ] Create `tests.rs` with all test functions
- [ ] Delete `crates/koji-core/src/proxy/koji_handlers.rs`
- [ ] Verify `proxy/mod.rs` still has `pub mod koji_handlers;` — no change needed
- [ ] Verify `proxy/server/router.rs` import still resolves correctly
- [ ] Run `cargo test --package koji-core`
  - Did all tests pass? If not, fix import paths and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: split proxy/koji_handlers.rs into domain submodules"

**Acceptance criteria:**
- [ ] All existing tests pass without modification to test logic
- [ ] `proxy/server/router.rs` import remains unchanged
- [ ] No clippy warnings
- [ ] Each submodule file is under 700 LOC (pull.rs will be the largest at ~700)

---

### Task 3: Split `config/resolve.rs` (1267 LOC) — extract tests

**Context:**
`resolve.rs` is 1267 lines but the implementation itself is only ~400 lines (lines 1-404). The remaining ~860 lines are all tests (lines 405-1267 under `#[cfg(test)]`). This is the simplest split: move tests to a separate file, keep implementation in place.

**Files:**
- Modify: `crates/koji-core/src/config/resolve.rs` (remove test module)
- Create: `crates/koji-core/src/config/resolve/tests.rs`

**What to implement:**

1. Move the entire `#[cfg(test)] mod tests { ... }` block from `resolve.rs` (lines 405-1267) to `resolve/tests.rs`

2. In `resolve.rs`, replace the `#[cfg(test)] mod tests { ... }` block with:
   ```rust
   #[cfg(test)]
   mod tests;
   ```
   **Important:** Converting from a file module to a directory module requires moving `resolve.rs` → `resolve/mod.rs`. Rust does not allow a `resolve.rs` file and a `resolve/` directory to coexist for the same module name.

3. In `resolve/tests.rs`, the test module contents need to import from `super`:
   - `use super::*;` gives access to all `impl Config` methods
   - Keep all existing `use` statements and test helper functions

**Steps:**
- [ ] Read the full test section from `resolve.rs` (lines 405-1267)
- [ ] Create `crates/koji-core/src/config/resolve/` directory
- [ ] Move `resolve.rs` to `resolve/mod.rs` (this converts the file module to a directory module)
   - Alternatively: keep `resolve.rs` and add `#[cfg(test)] mod tests;` with a `tests.rs` file alongside it (Rust supports both patterns, but directory module is more consistent with the rest of this plan)
- [ ] Create `resolve/tests.rs` with the moved test code
- [ ] Run `cargo test --package koji-core -- config::resolve`
  - Did all tests pass? If not, fix import paths and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: extract resolve.rs tests into separate file"

**Acceptance criteria:**
- [ ] All resolve tests pass
- [ ] `resolve.rs` (or `resolve/mod.rs`) is under 400 LOC
- [ ] No clippy warnings

---

### Task 4: Split `config/migrate.rs` (1022 LOC) — extract tests

**Context:**
Same pattern as Task 3. `migrate.rs` is 1022 lines: ~500 lines of implementation (lines 1-496), ~500 lines of tests (lines 497-1022). Move tests to a separate file.

**Files:**
- Modify: `crates/koji-core/src/config/migrate.rs` → move to `migrate/mod.rs`
- Create: `crates/koji-core/src/config/migrate/tests.rs`

**What to implement:**

1. Move the entire `#[cfg(test)] mod tests { ... }` block from `migrate.rs` (lines 497-1022) to `migrate/tests.rs`

2. In `migrate/mod.rs`, replace the `#[cfg(test)] mod tests { ... }` block with:
   ```rust
   #[cfg(test)]
   mod tests;
   ```

3. In `migrate/tests.rs`, `use super::*;` gives access to all public functions

**Steps:**
- [ ] Create `crates/koji-core/src/config/migrate/` directory
- [ ] Move `migrate.rs` to `migrate/mod.rs`
- [ ] Extract test module to `migrate/tests.rs`
- [ ] Run `cargo test --package koji-core -- config::migrate`
  - Did all tests pass? If not, fix import paths and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: extract migrate.rs tests into separate file"

**Acceptance criteria:**
- [ ] All migrate tests pass
- [ ] `migrate/mod.rs` is under 500 LOC
- [ ] No clippy warnings

---

### Dependency Order

These tasks can be done in any order since they touch different files. However, the recommended order is:

1. **Task 1** (queries.rs) — Cleanest split, lowest risk, serves as a template
2. **Task 3** (resolve.rs tests) — Simplest, just moving tests
3. **Task 4** (migrate.rs tests) — Same pattern as Task 3
4. **Task 2** (koji_handlers.rs) — Most complex, most files, do last

Each task must compile and pass tests independently before committing.