# Split `config/resolve/tests.rs` Into Topic Groups

**Goal:** Split the 2,214-line test file into 4 focused test modules grouped by topic.

**Architecture:** Replace single `tests.rs` with a `tests/` directory containing `mod.rs` (helper + re-exports), `path_resolution.rs`, `args_building.rs`, `server_resolution.rs`, and `kv_cache_types.rs`.

**Tech Stack:** Rust, existing test dependencies (no new deps).

---

## Context

The file `crates/tama-core/src/config/resolve/tests.rs` is 2,214 lines containing 33 tests and one helper function. The tests fall into 4 clear groups:

1. **Path resolution** (5 tests) — `test_resolve_backend_path_*`
2. **Args building** (20 tests) — `test_build_*args*`
3. **Server resolution** (5 tests) — `test_resolve_by_api_name`, `test_api_name_takes_priority`, `test_backward_compat_no_api_name`, `test_resolve_server_by_api_name`
4. **KV cache type** (6 tests) — `test_kv_cache_type_*`

The parent module declares `#[cfg(test)] mod tests;` which resolves to `tests.rs`. We'll replace it with a `tests/` directory.

---

### Task 1: Create `tests/` directory and move all tests

**Context:** Replace the single `tests.rs` file with a `tests/` directory. The `tests/mod.rs` holds the shared helper function and module declarations. All 33 tests are moved into sub-modules in this single task — neither the scaffold nor the empty sub-modules are independently verifiable, so we do it all at once.

**Files:**
- Create: `crates/tama-core/src/config/resolve/tests/mod.rs`
- Create: `crates/tama-core/src/config/resolve/tests/path_resolution.rs`
- Create: `crates/tama-core/src/config/resolve/tests/args_building.rs`
- Create: `crates/tama-core/src/config/resolve/tests/server_resolution.rs`
- Create: `crates/tama-core/src/config/resolve/tests/kv_cache_types.rs`
- Delete: `crates/tama-core/src/config/resolve/tests.rs` (after all code is moved)

**What to implement:**

1. Create `crates/tama-core/src/config/resolve/tests/` directory.

2. Create `tests/mod.rs` with:
   - Module declarations: `mod path_resolution; mod args_building; mod server_resolution; mod kv_cache_types;`
   - The `make_test_config` helper function (move from tests.rs)
   - Imports needed by the helper (use `crate::` paths matching existing convention)

3. Move all 33 tests into the 4 sub-modules (see lists below).

4. Update `config/resolve/mod.rs` — no change needed, `#[cfg(test)] mod tests;` auto-resolves to `tests/mod.rs`.

**What to implement:**

Read the old `tests.rs` (or `tests.rs.ref` if renamed) and move tests into sub-modules:

**`path_resolution.rs`** (5 tests):
- `test_resolve_backend_path_from_db`
- `test_resolve_backend_path_fallback`
- `test_resolve_backend_path_error`
- `test_resolve_backend_path_version_pin`
- `test_resolve_backend_path_version_pin_not_found`

**`args_building.rs`** (19 tests):
- `test_build_full_args_unified`
- `test_build_full_args_ctx_override`
- `test_build_full_args_no_sampling`
- `test_build_full_args_no_quants`
- `test_build_args_dedupes_backend_vs_model_flags`
- `test_build_args_sampling_overrides_inline_temp_in_args`
- `test_build_full_args_dedupes_backend_vs_model_flags`
- `test_build_full_args_returns_flat_tokens_with_quoted_path`
- `test_build_full_args_context_multiplied_by_num_parallel`
- `test_build_full_args_context_saturating_overflow`
- `test_build_full_args_context_no_num_parallel_defaults_to_one`
- `test_build_full_args_injects_np_flag`
- `test_build_full_args_no_np_when_default`
- `test_build_full_args_skips_np_when_already_present`
- `test_build_full_args_unified_n_slots`
- `test_build_full_args_non_unified_n_slots`
- `test_build_full_args_unified_default`
- `test_build_full_args_ctx_override_unified`
- `test_build_full_args_kv_unified_not_duplicated_when_in_user_args` (belongs here because it tests `build_full_args` kv-unified dedup logic, not cache-type injection)
- (any other `test_build_*args*` tests not listed above)

**`server_resolution.rs`** (4 tests):
- `test_resolve_by_api_name`
- `test_api_name_takes_priority`
- `test_backward_compat_no_api_name`
- `test_resolve_server_by_api_name`
- (any other `test_resolve_*` or `test_api_name_*` tests not in path_resolution)

**`kv_cache_types.rs`** (5 tests):
- `test_kv_cache_type_args_injected_when_set`
- `test_kv_cache_type_args_not_injected_when_none`
- `test_kv_cache_type_args_not_injected_for_non_llama_backend`
- `test_kv_cache_type_args_no_duplicate_when_in_user_args`
- `test_kv_cache_type_args_not_injected_for_empty_string`
- (any other `test_kv_cache_type_*` tests)

Each sub-module needs these imports (use `crate::` paths — match the existing convention):
```rust
use std::collections::BTreeMap;
use tempfile::tempdir;
use crate::config::types::QuantEntry;
use crate::config::BackendConfig;
use crate::db::queries::BackendInstallationRecord;
use crate::db::{open_in_memory, queries::insert_backend_installation};
use super::super::*;  // imports from resolve/mod.rs (Config methods, etc.)
use super::make_test_config;
```

Not every sub-module needs every import — only include what's actually used. For example:
- `path_resolution.rs` needs `BackendInstallationRecord`, `open_in_memory`, `insert_backend_installation` but NOT `tempdir` or `BTreeMap`
- `args_building.rs` needs `tempdir`, `BTreeMap`, `QuantEntry` but NOT DB imports
- `server_resolution.rs` needs `open_in_memory` but NOT `tempdir`
- `kv_cache_types.rs` needs `tempdir`, `BTreeMap` but NOT DB imports

After moving all tests:
1. Verify `tests.rs` has zero remaining code
2. Delete `crates/tama-core/src/config/resolve/tests.rs`

**Steps:**
- [ ] Create `crates/tama-core/src/config/resolve/tests/` directory
- [ ] Create `tests/mod.rs` with module decls and `make_test_config` helper
- [ ] Copy all 33 tests from `tests.rs` into the appropriate sub-modules
- [ ] Add required imports to each sub-module (use `crate::` paths)
- [ ] Delete `crates/tama-core/src/config/resolve/tests.rs`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package tama-core`
- [ ] Run `cargo test --package tama-core` — verify ALL 33 tests still pass

**Acceptance criteria:**
- [ ] `tests/` directory exists with `mod.rs` and 4 sub-module files
- [ ] Each sub-module contains the correct tests
- [ ] `cargo check --package tama-core` passes
- [ ] `cargo test --package tama-core` passes (all 33 tests)
- [ ] Old `tests.rs` is deleted

---

### Task 2: Final verification

**Context:** Verify the entire workspace builds, lints, and tests correctly after the split.

**Files:**
- No file changes expected

**Steps:**
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Verify file sizes — no file exceeds 1,500 lines

**Acceptance criteria:**
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] No file exceeds 1,500 lines
