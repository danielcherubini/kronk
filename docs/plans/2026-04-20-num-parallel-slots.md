# Num Parallel Slots Plan

**Goal:** Add a `num_parallel` field to model configs that multiplies the effective context length at inference time.

**Architecture:** A new `Option<u32>` field on `ModelConfig` stored in SQLite, resolved multiplicatively in two places: (1) when injecting `-c` into the backend launch command, and (2) in the opencode `/api/models` list response. Default is `Some(1)` via serde default. Migration v14 adds the column with `DEFAULT 1 CHECK(num_parallel >= 1)`.

**Tech Stack:** Rust, SQLite (rusqlite), Axum (koji-web), Leptos (frontend), TOML serialization

---

### Task 1: Core Config — Add `num_parallel` to ModelConfig

**Context:**
This task adds the `num_parallel` field to the core `ModelConfig` type and its DB record conversion methods. This is the foundation that all other tasks depend on. The field uses `Option<u32>` with a serde default of `Some(1)` so new models always serialize explicitly as `1`.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs`
- Test: `crates/koji-core/src/config/types.rs` (inline `mod tests`)

**What to implement:**

1. Add field to `ModelConfig` struct (near line ~190, after `context_length: Option<u32>`):
```rust
    /// Number of parallel contexts. Multiplies the effective context length.
    /// Default is Some(1). None at runtime is treated as 1.
    #[serde(default = "default_num_parallel")]
    pub num_parallel: Option<u32>,
```

2. Add helper function (near other defaults, after `fn default_enabled`):
```rust
fn default_num_parallel() -> Option<u32> {
    Some(1)
}
```

3. Update `to_db_record()` — add `num_parallel: self.num_parallel,` to the returned `ModelConfigRecord`.

4. Update `from_db_record()` — add `num_parallel: record.num_parallel,` to the `Self { ... }` construction.

5. Update the round-trip test `test_model_config_round_trip`:
   - Add `num_parallel: Some(2),` to the test's `ModelConfig` construction
   - Add `assert_eq!(round_trip.num_parallel, mc.num_parallel);` after other field assertions

**Steps:**
- [ ] Add `num_parallel: Option<u32>` field with serde default to `ModelConfig` struct in `config/types.rs`
- [ ] Add `fn default_num_parallel() -> Option<u32> { Some(1) }` helper function
- [ ] Update `to_db_record()` to include `num_parallel: self.num_parallel,`
- [ ] Update `from_db_record()` to include `num_parallel: record.num_parallel,`
- [ ] Update the round-trip test to set `num_parallel: Some(2)` in the ModelConfig construction and add `assert_eq!(round_trip.num_parallel, mc.num_parallel);`
- [ ] Run `cargo test --package koji-core test_model_config_round_trip` — verify it passes with the new field
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package koji-core` — ensure no compile errors
- [ ] Commit with message: "feat(core): add num_parallel field to ModelConfig"

**Acceptance criteria:**
- [ ] `ModelConfig` has `num_parallel: Option<u32>` with serde default of `Some(1)`
- [ ] `to_db_record()` includes `num_parallel` in the record
- [ ] `from_db_record()` reads `num_parallel` from the record
- [ ] Round-trip test passes with `num_parallel: Some(2)`

---

### Task 2: Database Migration and Query Layer

**Context:**
This task adds the `num_parallel` column to the SQLite schema via migration v14, and updates all DB query functions to read/write it. The migration uses `ALTER TABLE ADD COLUMN` with a `CHECK(num_parallel >= 1)` constraint to prevent invalid values at the DB level.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs`
- Modify: `crates/koji-core/src/db/queries/types.rs`
- Modify: `crates/koji-core/src/db/queries/model_config_queries.rs`

**What to implement:**

1. **migrations.rs**: Add migration v14 entry (after the v13 entry, before the closing `]`):
```rust
(
    14,
    r#"
        ALTER TABLE model_configs ADD COLUMN num_parallel INTEGER DEFAULT 1 CHECK(num_parallel >= 1);
    "#,
),
```
Update `LATEST_VERSION` from `13` to `14`.

2. **queries/types.rs**: Add field to `ModelConfigRecord`:
```rust
    pub num_parallel: Option<u32>,
```
Place after `context_length: Option<u32>`.

3. **queries/model_config_queries.rs**: Update all 4 query functions. The column is added after `context_length` in the table, so it shifts downstream indices by +1.

   For each function, update three places:
   - Column list (INSERT SELECT or explicit column names)
   - Parameter placeholder `?N` and params![] list
   - Row mapping via `row.get(N)?`

   **IMPORTANT**: Adding one column shifts ALL downstream indices by +1. Be meticulous.

   Current column order and indices in model_configs table:
   ```
   0:id, 1:repo_id, 2:display_name, 3:backend, 4:enabled,
   5:selected_quant, 6:selected_mmproj, 7:context_length,
   8:gpu_layers, 9:port, 10:args, 11:sampling,
   12:modalities, 13:profile, 14:api_name, 15:health_check,
   16:created_at, 17:updated_at
   ```

   After adding num_parallel after context_length:
   ```
   0:id, 1:repo_id, 2:display_name, 3:backend, 4:enabled,
   5:selected_quant, 6:selected_mmproj, 7:context_length,
   8:num_parallel, 9:gpu_layers, 10:port, 11:args,
   12:sampling, 13:modalities, 14:profile, 15:api_name,
   16:health_check, 17:created_at, 18:updated_at
   ```

   **upsert_model_config**:
   - Column list: add `, num_parallel` after `, context_length`
   - INSERT VALUES: add `, ?18` after `, ?17` (was 17 columns, now 18)
   - ON CONFLICT SET: add `, num_parallel = excluded.num_parallel` immediately after `context_length = excluded.context_length,` (maintains column order consistency)
   - params![]: add `record.num_parallel` after `record.context_length`

   **get_model_config, get_model_config_by_repo_id, get_all_model_configs**:
   - SELECT columns: add `num_parallel` after `context_length`
   - Row mapping: change all downstream indices by +1:
     - `context_length: row.get(7)?,` stays as-is
     - Add `num_parallel: row.get(8)?,`
     - Change `gpu_layers: row.get(8)?,` → `gpu_layers: row.get(9)?,`
     - Change `port: row.get(9)?,` → `port: row.get(10)?,`
     - Change `args: row.get(10)?,` → `args: row.get(11)?,`
     - Change `sampling: row.get(11)?,` → `sampling: row.get(12)?,`
     - Change `modalities: row.get(12)?,` → `modalities: row.get(13)?,`
     - Change `profile: row.get(13)?,` → `profile: row.get(14)?,`
     - Change `api_name: row.get(14)?,` → `api_name: row.get(15)?,`
     - Change `health_check: row.get(15)?,` → `health_check: row.get(16)?,`
     - Change `created_at: row.get(16)?,` → `created_at: row.get(17)?,`
     - Change `updated_at: row.get(17)?,` → `updated_at: row.get(18)?,`

**Steps:**
- [ ] Update `LATEST_VERSION` to `14` in `migrations.rs`
- [ ] Add migration v14 entry with `ALTER TABLE model_configs ADD COLUMN num_parallel INTEGER DEFAULT 1 CHECK(num_parallel >= 1);`
- [ ] Add `num_parallel: Option<u32>` to `ModelConfigRecord` in `queries/types.rs` after `context_length`
- [ ] Update `upsert_model_config`: add column, parameter (?18), and params entry for `num_parallel`
- [ ] Update `get_model_config`: add column to SELECT, add row.get(8) mapping
- [ ] Update `get_model_config_by_repo_id`: same as get_model_config
- [ ] Update `get_all_model_configs`: same as get_model_config
- [ ] Run `cargo check --package koji-core` — fix any compile errors
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(core): add num_parallel column to DB schema via migration v14"

**Acceptance criteria:**
- [ ] Migration v14 adds `num_parallel INTEGER DEFAULT 1 CHECK(num_parallel >= 1)`
- [ ] All 4 query functions read and write `num_parallel`
- [ ] Parameter indices are correct (no off-by-one errors)
- [ ] Code compiles without errors

---

### Task 3: Launch Logic — Multiply Context by num_parallel in build_full_args

**Context:**
This is the core runtime behavior. When building the backend launch command, the effective context length must be `resolved_context × num_parallel`. We use `saturating_mul` to prevent integer overflow (e.g., context=1_000_000 × slots=10_000 would overflow u32). This affects the `-c` flag injected into the llama.cpp command line.

**Files:**
- Modify: `crates/koji-core/src/config/resolve/mod.rs`

**What to implement:**

In `build_full_args`, locate the existing context injection block (around lines 276-289):
```rust
// Inject -c (context length) only if not already present.
let ctx = ctx_override.or(server.context_length).or_else(|| {
    server
        .quant
        .as_ref()
        .and_then(|q| server.quants.get(q).and_then(|qe| qe.context_length))
});
if let Some(ctx) = ctx {
    let already_has_c = grouped
        .iter()
        .any(|e| matches!(crate::config::flag_name(e), Some("-c") | Some("--ctx-size")));
    if !already_has_c {
        grouped.push(format!("-c {}", ctx));
    }
}
```

Replace with:
```rust
// Inject -c (context length) only if not already present.
let ctx = ctx_override.or(server.context_length).or_else(|| {
    server
        .quant
        .as_ref()
        .and_then(|q| server.quants.get(q).and_then(|qe| qe.context_length))
});
if let Some(ctx) = ctx {
    let already_has_c = grouped
        .iter()
        .any(|e| matches!(crate::config::flag_name(e), Some("-c") | Some("--ctx-size")));
    if !already_has_c {
        let slots = server.num_parallel.unwrap_or(1);
        let effective_ctx = ctx.saturating_mul(slots);
        grouped.push(format!("-c {}", effective_ctx));
    }
}
```

**Steps:**
- [ ] Find the context injection block in `build_full_args` (search for "Inject -c")
- [ ] Add `let slots = server.num_parallel.unwrap_or(1);` before the `grouped.push`
- [ ] Change `format!("-c {}", ctx)` to use `ctx.saturating_mul(slots)` instead of raw `ctx`
- [ ] Run `cargo check --package koji-core` — verify no errors
- [ ] Run `cargo test --package koji-core` — ensure existing tests still pass
- [ ] Verify saturating_mul works: context=1_000_000 × slots=10_000 should produce `-c 4294967295` (u32::MAX) without panic. Run `cargo test --package koji-core -- --nocapture` and check the output.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(core): multiply context by num_parallel in build_full_args"

**Acceptance criteria:**
- [ ] Effective context = resolved_context × num_parallel (using saturating_mul)
- [ ] `-c` flag uses the multiplied value
- [ ] Existing tests pass (no regression)
- [ ] `saturating_mul` prevents overflow for large values

---

### Task 4: Opencode API — Use Effective Context in Model List Response

**Context:**
The `/api/models` endpoint returns model metadata including `context_length`. This must also reflect the multiplied effective context so that OpenCode and other consumers see the correct window size. The multiplication happens after the existing resolution chain (cfg.context_length → card fallback).

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers/models.rs`

**What to implement:**

In `handle_opencode_list_models`, locate the context resolution block (around lines 238-249):
```rust
let context_length = if let Some(ctx) = cfg.context_length {
    Some(ctx)
} else {
    let card = state.get_model_card(id).await;
    card.and_then(|c| {
        let quant_key = cfg.quant.as_deref().unwrap_or_default();
        c.quants
            .get(quant_key)
            .and_then(|q| q.context_length)
            .or(c.model.default_context_length)
    })
};
```

**After this block**, add a new line that shadows `context_length` with the multiplied value:
```rust
let context_length = context_length.map(|ctx| ctx.saturating_mul(cfg.num_parallel.unwrap_or(1)));
```

Do NOT rewrite the if-else block. Simply add one new line after the closing `};` of the existing block.

**Steps:**
- [ ] Find context resolution in `handle_opencode_list_models` (search for "context_length = if let Some(ctx)")
- [ ] Add ONE new line after the existing block's closing `};`: `let context_length = context_length.map(|ctx| ctx.saturating_mul(cfg.num_parallel.unwrap_or(1)));`
  - Do NOT modify or rewrite the existing if-else block
- [ ] Run `cargo check --package koji-core` — verify no errors
- [ ] Run `cargo test --package koji-core` — ensure tests pass
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(core): apply num_parallel multiplication in opencode list API"

**Acceptance criteria:**
- [ ] `/api/models` returns effective context (resolved × slots) in `context_length` and `limit.context`
- [ ] No double-multiplication (this is a separate code path from build_full_args)
- [ ] Existing tests pass

---

### Task 5: Web API — Add num_parallel to ModelBody, CRUD, and Info Serialization

**Context:**
This task adds `num_parallel` to the web API layer so the frontend can read and write it. Three changes are needed: (1) `ModelBody` deserialization struct for incoming requests, (2) `apply_model_body()` passthrough in CRUD operations, and (3) `model_entry_json()` serialization for GET responses.

**Files:**
- Modify: `crates/koji-web/src/api/models/crud.rs`
- Modify: `crates/koji-web/src/api/models/info.rs`

**What to implement:**

1. **crud.rs — ModelBody struct** (add after `context_length` field):
```rust
    #[serde(default)]
    pub num_parallel: Option<u32>,
```

2. **crud.rs — apply_model_body()**: In the returned `ModelConfig` struct literal, add:
```rust
num_parallel: body.num_parallel,
```
Place after `context_length: body.context_length,`.

3. **info.rs — model_entry_json()**: Find the function that builds JSON for GET /api/models and GET /api/models/:id responses. Add to the JSON object:
```rust
"num_parallel": record.num_parallel,
```
Place after `"context_length": record.context_length,` (or equivalent existing context field).

**Steps:**
- [ ] Add `pub num_parallel: Option<u32>` with `#[serde(default)]` to `ModelBody` in `crud.rs`
- [ ] Add `num_parallel: body.num_parallel,` to the ModelConfig return in `apply_model_body()`
- [ ] Find `model_entry_json()` in `info.rs` — add `"num_parallel": record.num_parallel,` to the JSON object
- [ ] Run `cargo check --package koji-web` — verify no errors
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): add num_parallel to ModelBody, CRUD passthrough, and info serialization"

**Acceptance criteria:**
- [ ] `ModelBody` accepts `num_parallel` from JSON requests
- [ ] `apply_model_body()` passes `num_parallel` through to ModelConfig
- [ ] `model_entry_json()` includes `num_parallel` in GET responses
- [ ] No compile errors in koji-web

---

### Task 6: Web Frontend — Add num_parallel Input and Save Flow

**Context:**
This task adds the UI for setting `num_parallel` in the model editor. A number input appears next to Context Length in the General section. The save flow includes it in the POST/PUT body, and the initialization effect populates it when switching between models.

**Files:**
- Modify: `crates/koji-web/src/pages/model_editor/types.rs`
- Modify: `crates/koji-web/src/pages/model_editor/api.rs`
- Modify: `crates/koji-web/src/pages/model_editor/general_form.rs`
- Modify: `crates/koji-web/src/pages/model_editor/mod.rs` (form initialization)

**What to implement:**

1. **types.rs**: Add `num_parallel: Option<u32>` to both structs:
   - `ModelForm`: add after `context_length: Option<u32>`, derive Default (it already has `#[derive(Default)]`)
   - `ModelDetail`: add after `context_length: Option<u32>`

2. **mod.rs**: In the Effect that populates form from ModelDetail (around line ~130), add to the `ModelForm` struct literal:
```rust
num_parallel: d.num_parallel,
```
Place after `context_length: d.context_length,`. This ensures existing models loaded from the API display their num_parallel value in the UI.

2. **api.rs — fetch_model() for "new"**: In the default ModelDetail construction (when `id == "new"`), add:
```rust
num_parallel: Some(1),
```

3. **api.rs — save_model()**: In the body JSON object, add:
```rust
"num_parallel": form.num_parallel,
```
Place after `"context_length": form.context_length,`.

4. **general_form.rs — initialization effect** (around line ~50): Add inside the existing `Effect::new` block:
```rust
set_input_value(
    "field-num-parallel",
    &f.num_parallel.map(|v| v.to_string()).unwrap_or_default(),
);
```

5. **general_form.rs — UI input**: In the `form_grid` div, after the `ContextLengthSelector` block and before the port field, add:
```rust
<label class="form-label" for="field-num-parallel">"Num parallel slots"</label>
<input
    id="field-num-parallel"
    class="form-input"
    type="number"
    min="1"
    placeholder="1"
    on:input=move |ev| {
        form.update(|f| {
            if let Some(form) = f {
                form.num_parallel = target_value(&ev).parse::<u32>().ok();
            }
        });
    }
/>
```

**Steps:**
- [ ] Add `num_parallel: Option<u32>` to `ModelForm` in `types.rs` (after context_length)
- [ ] Add `num_parallel: Option<u32>` to `ModelDetail` in `types.rs` (after context_length)
- [ ] In `api.rs` fetch_model("new"): add `num_parallel: Some(1),` to default ModelDetail
- [ ] In `api.rs` save_model(): add `"num_parallel": form.num_parallel,` to body JSON
- [ ] In `general_form.rs`: add `set_input_value("field-num-parallel", ...)` in initialization effect
- [ ] In `general_form.rs`: add the number input element after ContextLengthSelector
- [ ] Run `cargo check --package koji-web` — verify no errors (this compiles the WASM frontend)
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat(web): add num_parallel input to model editor UI"

**Acceptance criteria:**
- [ ] `ModelForm` and `ModelDetail` both have `num_parallel: Option<u32>`
- [ ] New model defaults to `Some(1)` in ModelDetail
- [ ] Save flow includes `num_parallel` in POST/PUT body
- [ ] Form input initializes from existing model data when switching models
- [ ] Number input has type="number", min="1", placeholder="1"

---

### Task 7: Integration Verification — Build, Test, and Format

**Context:**
Final verification that all changes compile together, tests pass, and code is properly formatted. This ensures the feature works end-to-end across all layers.

**Files:**
- All modified files from Tasks 1-6

**What to implement:**
- Run full workspace build and test suite
- Verify formatting
- No new warnings from clippy

**Steps:**
- [ ] Run `cargo fmt --all` — verify no changes needed
- [ ] Run `cargo check --workspace` — verify no errors
- [ ] Run `cargo clippy --workspace -- -D warnings` — fix any warnings
- [ ] Run `cargo test --workspace` — all tests pass
- [ ] Run `cargo build --workspace` — release build succeeds
- [ ] Commit with message: "chore: format and verify num_parallel feature"

**Acceptance criteria:**
- [ ] `cargo fmt --all` reports no changes
- [ ] `cargo clippy --workspace -- -D warnings` passes clean
- [ ] All workspace tests pass
- [ ] Full workspace build succeeds (debug and release)
