# KV Unified Support Plan

**Goal:** Add `--kv-unified` support to llama-server launch commands so parallel slots can share a unified KV cache pool instead of dedicated regions.

**Architecture:** A per-model `kv_unified: bool` config field controls whether `build_full_args()` injects the `--kv-unified` flag and which formula it uses for `-c`. When `kv_unified == true`: `-c = context_length` (shared pool). When `false`: `-c = context_length * num_parallel` (dedicated regions, current behavior). Default is `false` in serde for backward compat; new models created via UI/CLI default to `true`.

**Tech Stack:** Rust (tama-core config + proxy), SQLite migration, Leptos (tama-web model editor), clap (tama-cli flags)

---

### Task 1: Add kv_unified field to config types and DB schema

**Context:**
This task adds the foundational data structures. The `kv_unified` field must exist in `ModelConfig` (config/types.rs), be serialized/deserialized correctly with backward-compatible defaults, round-trip through the SQLite database via `ModelConfigRecord`, and have a migration that preserves existing behavior for current users.

The default MUST be `false` in serde so existing TOML configs without the field continue using the non-unified formula (`-c = ctx * slots`). The DB migration adds the column with `DEFAULT 0` (false), then sets `kv_unified = 1` for rows where `num_parallel IS NULL OR num_parallel <= 1` (since unified is a no-op for single slot).

**Files:**
- Modify: `crates/tama-core/src/config/types.rs` — add field to `ModelConfig`, default function, `to_db_record()`, `from_db_record()`
- Modify: `crates/tama-core/src/db/queries/types.rs` — add field to `ModelConfigRecord`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` — add column to INSERT and SELECT queries, shift all subsequent `row.get()` indices by 1
- Modify: `crates/tama-core/src/db/migrations.rs` — add migration v17 for new column (update `LATEST_VERSION` from 16 to 17)
- Test: `crates/tama-core/src/config/types.rs` — update existing `test_model_config_round_trip` test to include `kv_unified`

**What to implement:**

1. In `config/types.rs`, inside `ModelConfig` struct, add (place near `num_parallel` field):
   ```rust
   /// Whether all parallel slots share a single unified KV cache pool.
   /// When true, `-c` equals `context_length` regardless of `num_parallel`.
   /// When false, `-c = context_length * num_parallel` (each slot gets dedicated region).
   /// Default is false for backward compatibility. New models should use true.
   #[serde(default)]
   pub kv_unified: bool,
   ```

2. In `ModelConfig::to_db_record()`, add `kv_unified: self.kv_unified` to the record construction.

3. In `ModelConfig::from_db_record()`, add `kv_unified: record.kv_unified` to the Self construction.

4. In `db/queries/types.rs`, inside `ModelConfigRecord` struct, add:
   ```rust
   pub kv_unified: bool,
   ```

5. In `db/queries/model_config_queries.rs`:
   - In `upsert_model_config()`: add `kv_unified` to the INSERT columns, VALUES (`?N`), and ON CONFLICT UPDATE clause
   - In `get_model_config()`: add column to SELECT and `row.get(N)` mapping — shift all subsequent indices by 1
   - In `get_model_config_by_repo_id()`: same changes as above
   - In `get_all_model_configs()`: same changes as above
   - IMPORTANT: Every getter function has hardcoded column indices (0..N). Adding a new column means incrementing ALL subsequent `row.get()` call indices.

6. In `db/migrations.rs`:
   - Update `LATEST_VERSION` from 16 to 17
   - Add a new migration function v17 that runs:
     ```sql
     ALTER TABLE model_configs ADD COLUMN kv_unified INTEGER NOT NULL DEFAULT 0;
     UPDATE model_configs SET kv_unified = 1 WHERE num_parallel IS NULL OR num_parallel <= 1;
     ```
   - Use `INTEGER` (not `BOOLEAN`) for consistency with existing boolean columns like `enabled`
   - Register the migration in the migration list

7. Update `test_model_config_round_trip` in `config/types.rs` to include `kv_unified: true` in the test ModelConfig and assert it round-trips correctly.

**Steps:**
- [ ] Write a failing test for DB round-trip of `kv_unified` field (add to existing `test_model_config_round_trip`)
- [ ] Run `cargo test --package tama-core test_model_config_round_trip`
  - Did it fail because the field doesn't exist? If not, investigate.
- [ ] Add `kv_unified: bool` field to `ModelConfig` in `config/types.rs` with `#[serde(default)]`
- [ ] Update `to_db_record()` and `from_db_record()` methods
- [ ] Add `kv_unified: bool` field to `ModelConfigRecord` in `db/queries/types.rs`
- [ ] Update SQL queries in `model_config_queries.rs` (upsert, get by id, get by repo, get all)
- [ ] Add migration in `migrations.rs` (increment version, add ALTER TABLE + UPDATE)
- [ ] Run `cargo test --package tama-core test_model_config_round_trip`
  - Did it pass? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? Fix any failures.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add kv_unified field to ModelConfig and DB schema"

**Acceptance criteria:**
- [ ] `ModelConfig` has `kv_unified: bool` field defaulting to `false` via serde
- [ ] `to_db_record()` / `from_db_record()` round-trip preserves `kv_unified`
- [ ] DB migration adds column with DEFAULT 0, sets true for single-slot rows
- [ ] All tama-core tests pass
- [ ] Code compiles cleanly (fmt + clippy)

---

### Task 2: Update build_full_args to branch on kv_unified

**Context:**
This is the core behavioral change. `build_full_args()` in `config/resolve/mod.rs` currently always computes `-c = ctx * slots`. It must now check `server.kv_unified` and use different formulas. Additionally, when `kv_unified == true`, it must inject the `--kv-unified` flag into the args list.

The `ctx_override` parameter (used by benchmarks) is treated as raw per-slot context — the unified/non-unified formula applies to it identically. If a user manually added `--kv-unified` in their args array, we must not duplicate it.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs` — update `-c` injection logic and add `--kv-unified` injection
- Test: `crates/tama-core/src/config/resolve/tests.rs` — add new test cases

**What to implement:**

1. In `build_full_args()`, find the block that injects `-c`. Replace the formula logic:
   ```rust
   // Current code:
   let effective_ctx = ctx.saturating_mul(slots);
   grouped.push(format!("-c {}", effective_ctx));

   // New code:
   let total_ctx = if server.kv_unified {
       // Unified KV: all slots share one pool, -c = per_slot context
       ctx
   } else {
       // Non-unified: each slot gets dedicated region, -c = per_slot * slots
       ctx.saturating_mul(slots)
   };
   grouped.push(format!("-c {}", total_ctx));
   ```

2. After the `-ngl` injection block and BEFORE the sampling merge step, add `--kv-unified` injection (this keeps server-level flags grouped together, separate from sampling params):
   ```rust
   // Inject --kv-unified flag when enabled and not already present.
   if server.kv_unified {
       let already_has_kv_unified = grouped.iter().any(|e| {
           matches!(crate::config::flag_name(e), Some("--kv-unified"))
       });
       if !already_has_kv_unified {
           grouped.push("--kv-unified".to_string());
       }
   }
   ```

3. Add tests in `resolve/tests.rs`:
   - `test_build_full_args_unified_n_slots`: kv_unified=true, num_parallel=4, context_length=8192 → args contain `-c 8192` and `--kv-unified`
   - `test_build_full_args_non_unified_n_slots`: kv_unified=false, num_parallel=4, context_length=8192 → args contain `-c 32768`, no `--kv-unified`
   - `test_build_full_args_unified_default`: kv_unified omitted (defaults false), num_parallel=2 → uses non-unified formula (`-c = ctx * 2`)
   - `test_build_full_args_ctx_override_unified`: ctx_override=Some(4096), kv_unified=true, num_parallel=3 → args contain `-c 4096` (not 12288)

**Steps:**
- [ ] Write failing tests for unified/non-unified behavior in `resolve/tests.rs`
- [ ] Run `cargo test --package tama-core -- config::resolve::tests`
  - Did they fail because the logic doesn't branch? If not, investigate.
- [ ] Update the `-c` formula in `build_full_args()` to check `server.kv_unified`
- [ ] Add `--kv-unified` flag injection after `-np` block
- [ ] Run `cargo test --package tama-core -- config::resolve::tests`
  - Did all resolve tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: branch build_full_args on kv_unified flag"

**Acceptance criteria:**
- [ ] kv_unified=true → `-c = context_length`, `--kv-unified` flag injected
- [ ] kv_unified=false → `-c = context_length * num_parallel`, no `--kv-unified` flag
- [ ] ctx_override is treated as raw per-slot context (formula applies)
- [ ] `--kv-unified` is not duplicated if already in user args
- [ ] Default (false) preserves existing non-unified behavior
- [ ] All new tests pass

---

### Task 3: Add CLI support for kv_unified

**Context:**
The CLI's model create command constructs a `ModelConfig` and needs to include the `kv_unified` field. New models created via CLI should default to `true` (unified), since they're fresh configs benefiting from the better behavior.

**Files:**
- Modify: `crates/tama-cli/src/commands/model/create.rs` — add kv_unified field to ModelConfig construction
- Modify: `crates/tama-cli/src/handlers/server/add.rs` — add kv_unified field to ModelConfig construction (line ~139)
- Modify: `crates/tama-cli/src/args.rs` or relevant args struct — add CLI flags if applicable
- Modify: `crates/tama-cli/src/cli.rs` — add --kv-unified / --no-kv-unified flags to model create subcommand

**What to implement:**

1. In `tama-cli/src/commands/model/create.rs`, find where `ModelConfig` is constructed in `cmd_create`. Add `kv_unified: true` (default to unified for new models).

2. In `tama-cli/src/handlers/server/add.rs`, find the `ModelConfig` construction (~line 139). Add `kv_unified: true`.

3. If the CLI has flags for other ModelConfig fields (like `--context-length` or `--num-parallel`), add corresponding `--kv-unified` / `--no-kv-unified` flags. If no such flags exist for model config options, skip this — the field defaults to `true` for new models and can be edited via UI later.

4. Check if any other CLI commands construct `ModelConfig` (e.g., in `handlers/run.rs`, `handlers/service_cmd.rs`) and add the field where needed.

**Steps:**
- [ ] Find all places in tama-cli that construct `ModelConfig`
- [ ] Add `kv_unified: true` to each construction site (new models default to unified)
- [ ] If CLI has model config flags, add --kv-unified/--no-kv-unified flags
- [ ] Run `cargo build --package tama-cli`
  - Did it compile? Fix any missing field errors.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add kv_unified support to CLI model creation"

**Acceptance criteria:**
- [ ] All ModelConfig constructions in tama-cli include `kv_unified` field
- [ ] New models default to `kv_unified: true` via CLI
- [ ] Code compiles cleanly

---

### Task 4: Add Web UI support for kv_unified

**Context:**
The web UI's model editor needs to expose the `kv_unified` setting. This requires updating the backend API types (ModelBody), the frontend form types (ModelDetail, ModelForm), the API serialization/deserialization, and the visual form component with a checkbox.

New models created via the web UI should default to `true` (unified).

**Files:**
- Modify: `crates/tama-web/src/api/models/crud.rs` — add `kv_unified: Option<bool>` to `ModelBody`, wire through `apply_model_body()` with default-true for new models
- Modify: `crates/tama-web/src/api/models/info.rs` — add `kv_unified` to `model_entry_json()` response
- Modify: `crates/tama-web/src/pages/model_editor/types.rs` — add `kv_unified: bool` to `ModelDetail` and `ModelForm`
- Modify: `crates/tama-web/src/pages/model_editor/api.rs` — include `kv_unified` in JSON body for save, set default for new models
- Modify: `crates/tama-web/src/pages/model_editor/general_form.rs` — add checkbox UI element
- Modify: `crates/tama-web/src/types/config.rs` — add `kv_unified: bool` to mirror ModelConfig type + wire through both `From` impls

**What to implement:**

1. In `api/models/crud.rs`, inside the `ModelBody` struct, add:
   ```rust
   pub kv_unified: Option<bool>,
   ```
   In `apply_model_body()`:
   - Add `kv_unified: false` to the base config construction (when `existing` is `None`) — serde default handles this
   - In the final `ModelConfig` return, use: `kv_unified: body.kv_unified.unwrap_or(existing.map(|e| e.kv_unified).unwrap_or(true))`
     - This defaults new models to `true`, preserves existing value on update when body omits the field, and respects explicit values
   - Add tests: `apply_model_body_kv_unified_passthrough` and `apply_model_body_kv_unified_default_true_for_new`

2. In `api/models/info.rs`, in `model_entry_json()`, add to the JSON response:
   ```rust
   "kv_unified": record.kv_unified,
   ```

3. In `pages/model_editor/types.rs`:
   - In `ModelDetail`, add `pub kv_unified: bool,`
   - In `ModelForm`, add `pub kv_unified: bool,`

4. In `pages/model_editor/api.rs`:
   - In the JSON body construction for `save_model()`, add `"kv_unified": form.kv_unified,` (serde_json handles bool → JSON)
   - In `fetch_model()` for new models, set default to `true`

5. In `types/config.rs`:
   - Add `pub kv_unified: bool,` to the mirror `ModelConfig` struct (with `#[serde(default)]`)
   - In `From<CoreModelConfig> for ModelConfig`, add `kv_unified: m.kv_unified`
   - In `From<ModelConfig> for CoreModelConfig`, add `kv_unified: m.kv_unified`

6. In `pages/model_editor/general_form.rs`, add a checkbox after the "Num parallel slots" field:
   ```html
   <div class="form-group">
       <label class="form-label" for="field-kv-unified">Unified KV cache</label>
       <div class="form-hint">All parallel slots share a single context pool. Better for agent+subagent workflows.</div>
       <input
           type="checkbox"
           id="field-kv-unified"
           prop:checked=form.kv_unified
           on:change={move |ev: &ChangeEvent| {
               form.kv_unified = ev.target().checked();
           }}
       />
   </div>
   ```

**Steps:**
- [ ] Add `kv_unified: Option<bool>` field to `ModelBody` in `api/models/crud.rs`
- [ ] Wire through `apply_model_body()` function with default-true for new models logic
- [ ] Add `kv_unified` to `model_entry_json()` in `api/models/info.rs`
- [ ] Add `kv_unified: bool` to `ModelDetail` and `ModelForm` types
- [ ] Include in JSON body for save (`Some(form.kv_unified)`) and default true for new models in `fetch_model()`
- [ ] Add checkbox UI in general_form.rs
- [ ] Update `types/config.rs`: add field to mirror struct + both `From` impls
- [ ] Add tests: `apply_model_body_kv_unified_passthrough` and `apply_model_body_kv_unified_default_true_for_new`
- [ ] Run `cargo build --package tama-web`
  - Did it compile? Fix any missing field errors.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add kv_unified checkbox to web UI model editor"

**Acceptance criteria:**
- [ ] Web API accepts and persists `kv_unified` via ModelBody
- [ ] Frontend form has a checkbox for unified KV cache
- [ ] New models default to `true`, existing models load their stored value
- [ ] Code compiles cleanly

---

### Task 5: Integration tests and verification

**Context:**
Final task to ensure everything works end-to-end. Run the full test suite, verify the complete feature across all crates, and check that no regressions were introduced.

**Files:**
- Test: `crates/tama-core/src/config/resolve/tests.rs` — run existing + new tests
- Test: `crates/tama-cli/tests/tests.rs` — check CLI tests still pass
- Test: `crates/tama-web/tests/` — check web tests still pass

**What to implement:**
No new code. This task runs the full verification suite.

**Steps:**
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix failures before continuing.
- [ ] Run `cargo test --package tama-cli`
  - Did all tests pass? Fix any ModelConfig construction errors.
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? Fix any type mismatch errors.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it pass? Fix any clippy warnings (unused fields, etc.)
- [ ] Run `cargo fmt --all`
- [ ] Verify: create a test model config with kv_unified=true, num_parallel=4, context_length=8192 → build_full_args produces `-c 8192 --kv-unified -np 4`
- [ ] Verify: same config with kv_unified=false → build_full_args produces `-c 32768 -np 4` (no --kv-unified)
- [ ] Verify: web API GET /models returns `kv_unified` field in response
- [ ] Verify: creating a new model via web UI defaults `kv_unified` to true
- [ ] Commit with message: "test: verify kv_unified integration across workspace"

**Acceptance criteria:**
- [ ] All workspace tests pass (`cargo test --workspace`)
- [ ] Clippy passes with no warnings (`cargo clippy --workspace -- -D warnings`)
- [ ] Code is formatted (`cargo fmt --all`)
- [ ] Manual verification of both unified and non-unified arg output

---

## Summary

| Task | Files | Scope |
|------|-------|-------|
| 1 | config/types.rs, DB queries, migrations | Data model + schema |
| 2 | config/resolve/mod.rs | Launch command logic |
| 3 | tama-cli create command | CLI support |
| 4 | tama-web editor + API types | Web UI support |
| 5 | Full test suite | Integration verification |

Each task is independently commitable. Tasks 1-2 are core and should be done first. Tasks 3-4 can be done in parallel after Task 1. Task 5 verifies everything.
