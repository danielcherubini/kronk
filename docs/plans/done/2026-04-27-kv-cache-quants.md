# KV Cache Quantization Dropdowns Plan

**Goal:** Add K and V cache quantization dropdown selectors to the model editor form, wired through all layers down to llama-server CLI flags (`--cache-type-k` / `--cache-type-v`).

**Architecture:** Two new optional string fields flow from web UI → ModelForm (web types) → POST/PUT body (API layer) → server-side ModelBody → apply_model_body() → ModelConfig core type → DB → build_full_args() which injects `-ctk` and `-ctv` flags for llama.cpp backends. The UI uses a dropdown with known values + a conditional text input for custom values.

**Tech Stack:** Rust, Leptos (WASM), SQLite/rusqlite, serde

---

### Task 1: Add DB columns, migration, and core type fields for KV cache types

**Context:**
The data model needs to persist `cache_type_k` and `cache_type_v` values so they survive restarts. This task adds the database schema migration (version 18), updates the ModelConfigRecord struct, and adds corresponding optional string fields to ModelConfig with proper serde attributes for TOML config serialization.

**Files:**
- Modify: `crates/tama-core/src/db/queries/types.rs` — add fields to `ModelConfigRecord`
- Modify: `crates/tama-core/src/db/migrations.rs` — add migration version 18
- Modify: `crates/tama-core/src/config/types.rs` — add fields to `ModelConfig`, update `to_db_record()`, update `from_db_record()`

**What to implement:**

1. In `crates/tama-core/src/db/queries/types.rs`, add two new fields after `gpu_layers` in `ModelConfigRecord`:
```rust
pub cache_type_k: Option<String>,
pub cache_type_v: Option<String>,
```

2. In `crates/tama-core/src/db/migrations.rs`, add migration version 18 after the existing version 17 entry (around line 447):
```rust
(
    18,
    r#"
        ALTER TABLE model_configs ADD COLUMN cache_type_k TEXT;
        ALTER TABLE model_configs ADD COLUMN cache_type_v TEXT;
    "#,
),
```

3. In `crates/tama-core/src/config/types.rs`, add two fields to `ModelConfig` (after `gpu_layers` at ~line 175):
```rust
/// KV cache data type for K head (e.g., "f16", "q4_0"). Passed as --cache-type-k.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cache_type_k: Option<String>,

/// KV cache data type for V head (e.g., "f16", "q8_0"). Passed as --cache-type-v.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub cache_type_v: Option<String>,
```

4. Update `ModelConfig::to_db_record()` — copy both fields into the record struct initialization (search for `gpu_layers: self.gpu_layers,` and add after it):
```rust
cache_type_k: self.cache_type_k.clone(),
cache_type_v: self.cache_type_v.clone(),
```

5. Update `ModelConfig::from_db_record()` — populate from record (search for `gpu_layers: record.gpu_layers,` and add after it):
```rust
cache_type_k: record.cache_type_k.clone(),
cache_type_v: record.cache_type_v.clone(),
```

**Steps:**
- [ ] Read `crates/tama-core/src/db/queries/types.rs` lines 1-30 to understand the existing record structure, then add `cache_type_k: Option<String>` and `cache_type_v: Option<String>` fields after `gpu_layers`.
- [ ] Read `crates/tama-core/src/db/migrations.rs` around line 447 to find the migration version 17 entry (kv_unified). Add a new entry after it for version 18 with two ALTER TABLE statements for TEXT nullable columns.
- [ ] Read `crates/tama-core/src/config/types.rs` around line 175 to find `gpu_layers` field in `ModelConfig` struct, add the two new fields after it with `#[serde(default, skip_serializing_if = "Option::is_none")]` attributes.
- [ ] Update `to_db_record()` method in the same file — search for `gpu_layers: self.gpu_layers,` and add two lines after it copying cache_type_k/v.
- [ ] Update `from_db_record()` method — search for `gpu_layers: record.gpu_layers,` and add two lines after it reading cache_type_k/v.
- [ ] Run `cargo test --package tama-core config::types::tests` to verify existing tests still pass. The round_trip test will fail because it doesn't set the new fields yet — fix by adding `cache_type_k: None,` and `cache_type_v: None,` to the original `ModelConfig` construction (around line 240), then re-run until all assertions pass.
- [ ] Run `cargo fmt --all`
  - Did it succeed? If not, fix formatting issues before continuing.
- [ ] Commit with message: "feat(core): add KV cache type fields to config model (cache_type_k/v)"

**Acceptance criteria:**
- [ ] ModelConfigRecord has two new Option<String> columns for cache_type_k and cache_type_v
- [ ] Migration version 18 adds these as TEXT nullable columns
- [ ] ModelConfig has corresponding serde fields with #[serde(default, skip_serializing_if = "Option::is_none")] attributes
- [ ] to_db_record() copies both fields; from_db_record() reads them back
- [ ] test_model_config_round_trip passes after adding the new field assertions (both None)

---

### Task 2: Wire KV cache types into CLI arg builder

**Context:**
Now that the data model holds these values, they need to be translated into actual llama-server command-line arguments. The `build_full_args()` function in resolve/mod.rs already injects `-ngl`, `-c`, `-np`, and `--kv-unified`. Follow the exact same pattern: check if backend is llama.cpp compatible (backend_is_llama_cpp()), then inject `-ctk` / `-ctv` flags only when the field is Some.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs` — add injection logic in build_full_args() after kv_unified block (~line 330-335)

**What to implement:**

After the existing `--kv-unified` injection block (~lines 330-335), insert two new blocks following the exact same pattern as gpu_layers (~lines 318-327):

For K cache type:
```rust
// Inject --cache-type-k only if set and backend supports it.
if is_llama_cpp_backend {
    if let Some(ref ct_k) = server.cache_type_k {
        let already_has_ctk = grouped.iter().any(|e| matches!(crate::config::flag_name(e), Some("-ctk") | Some("--cache-type-k")));
        if !already_has_ctk && !ct_k.is_empty() {
            grouped.push(format!("-ctk {}", ct_k));
        }
    }
}
```

For V cache type (same pattern):
```rust
// Inject --cache-type-v only if set and backend supports it.
if is_llama_cpp_backend {
    if let Some(ref ct_v) = server.cache_type_v {
        let already_has_ctv = grouped.iter().any(|e| matches!(crate::config::flag_name(e), Some("-ctv") | Some("--cache-type-v")));
        if !already_has_ctv && !ct_v.is_empty() {
            grouped.push(format!("-ctv {}", ct_v));
        }
    }
}
```

**Steps:**
- [ ] Read `crates/tama-core/src/config/resolve/mod.rs` around lines 330-340 to find the existing kv_unified injection block. It looks like: `if is_llama_cpp_backend && server.kv_unified { ... }`
- [ ] After that entire block (before the sampling merge block), add two new blocks for cache_type_k and cache_type_v using the gpu_layers pattern (~lines 318-327) as reference. Use `-ctk` / `--cache-type-k` flag names and same check logic with `flag_name()`.
- [ ] Write a unit test in `crates/tama-core/src/config/resolve/tests.rs` (or wherever resolve tests live) that creates a ModelConfig with `cache_type_k: Some("q4_0")` and `cache_type_v: Some("q8_0")`, calls `build_full_args()`, and asserts both `-ctk q4_0` and `-ctv q8_0` appear in the output. Test that flags don't appear when fields are None or when backend is not llama.cpp.
- [ ] Run `cargo test --package tama-core config::resolve::tests::test_kv_cache_type_args` — did it fail? If yes, proceed to implement. If no, investigate why.
- [ ] Run `cargo clippy --package tama-core -- -D warnings` to verify no lint issues. If it fails, fix the clippy warnings before continuing.
- [ ] Commit with message: "feat(core): inject KV cache type flags into build_full_args"

**Acceptance criteria:**
- [ ] When server.cache_type_k = Some("q4_0"), args contain "-ctk q4_0" (only for llama.cpp backends)
- [ ] Same pattern for cache_type_v with -ctv/--cache-type-v
- [ ] Flags are not injected if already present in user's custom args
- [ ] Unit test verifies injection behavior for both backends and None values

---

### Task 3: Update server-side API layer (ModelBody, apply_model_body, info.rs, queries)

**Context:**
This is the critical server-side plumbing that was missing from the original plan. The web client sends JSON with cache_type_k/v fields, but without updating the server-side API layer, those fields would be silently dropped. This task ensures the data flows from the HTTP request body through to the ModelConfig type and back to the client on GET requests.

**Files:**
- Modify: `crates/tama-web/src/api/models/crud.rs` — add fields to `ModelBody` struct (~line 23), wire through `apply_model_body()` (~line 57)
- Modify: `crates/tama-web/src/api/models/info.rs` — add fields to `model_entry_json()` (~line 108)
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs` — add columns to INSERT/SELECT queries and params
- Modify: `crates/tama-web/src/types/config.rs` — add fields to mirror `ModelConfig` (~line 155) and `From` conversions (~line 504)

**What to implement:**

1. In `crates/tama-web/src/api/models/crud.rs`, add fields to `ModelBody` struct after `gpu_layers` (~line 48):
```rust
#[serde(default)]
pub cache_type_k: Option<String>,
#[serde(default)]
pub cache_type_v: Option<String>,
```

2. In `apply_model_body()` function (~line 57), add to the `base` ModelConfig defaults (after `gpu_layers: None,`):
```rust
cache_type_k: None,
cache_type_v: None,
```

3. In the returned `ModelConfig` construction (~line 97), add after `gpu_layers: body.gpu_layers,`:
```rust
cache_type_k: body.cache_type_k,
cache_type_v: body.cache_type_v,
```

4. In `crates/tama-web/src/api/models/info.rs`, add to `model_entry_json()` after `"gpu_layers": record.gpu_layers,` (~line 109):
```json
"cache_type_k": record.cache_type_k.as_ref().map(|s| s.to_string()),
"cache_type_v": record.cache_type_v.as_ref().map(|s| s.to_string()),
```

5. In `crates/tama-core/src/db/queries/model_config_queries.rs`:
   - In `upsert_model_config()` (~line 15): add `cache_type_k, cache_type_v` to column list after `gpu_layers, port,`
   - In VALUES clause (~line 27): add `?19, ?20` after `?17, ?18,` (check exact param count)
   - In ON CONFLICT SET (~line 29): add `cache_type_k = excluded.cache_type_k, cache_type_v = excluded.cache_type_v,` after `gpu_layers = excluded.gpu_layers,`
   - In params![] (~line 48): add `record.cache_type_k, record.cache_type_v,` after `record.gpu_layers,`
   - In `get_model_config()` (~line 74): add `cache_type_k, cache_type_v` to SELECT list after `gpu_layers, port,`
   - In `row.get()` calls (~line 90): add `cache_type_k: row.get(11)?, cache_type_v: row.get(12)?,` after `gpu_layers: row.get(10)?,`
   - Same pattern for `get_model_config_by_repo_id()` (~line 116) and `get_all_model_configs()` (~line 155)

6. In `crates/tama-web/src/types/config.rs`, add to mirror `ModelConfig` (~line 155) after `gpu_layers`:
```rust
pub cache_type_k: Option<String>,
pub cache_type_v: Option<String>,
```

7. In `From<CoreModelConfig>` (~line 504) and `From<ModelConfig>` (~line 531), add after `gpu_layers: m.gpu_layers,`:
```rust
cache_type_k: m.cache_type_k,
cache_type_v: m.cache_type_v,
```

**Steps:**
- [ ] Read `crud.rs` lines 23-50 to find ModelBody struct. Add cache_type_k and cache_type_v fields after gpu_layers with #[serde(default)].
- [ ] Read `crud.rs` lines 57-130 to find apply_model_body function. Add to base defaults (after gpu_layers: None,) and to returned ModelConfig (after gpu_layers: body.gpu_layers,).
- [ ] Read `info.rs` lines 105-115 to find model_entry_json function. Add cache_type_k and cache_type_v entries after gpu_layers.
- [ ] Read `model_config_queries.rs` completely to understand the query structure. Count exact parameter positions for INSERT and SELECT statements. Add cache_type_k/v columns to all INSERT/SELECT queries and update params![] arrays. Add row.get() calls with correct indices (11 and 12 after gpu_layers at 10).
- [ ] Read `types/config.rs` lines 155-165 to find mirror ModelConfig. Add fields after gpu_layers. Read lines 500-540 to find From conversions. Add fields after gpu_layers in both.
- [ ] Run `cargo clippy --package tama-web --features ssr -- -D warnings`. Fix any issues before continuing.
- [ ] Commit with message: "feat(web): wire KV cache type fields through server-side API layer"

**Acceptance criteria:**
- [ ] ModelBody struct has cache_type_k and cache_type_v fields with #[serde(default)]
- [ ] apply_model_body() wires fields from body to ModelConfig
- [ ] model_entry_json() includes both fields in GET response
- [ ] All DB queries (upsert, get, get_all) include new columns in INSERT/SELECT
- [ ] Mirror ModelConfig in types/config.rs has both fields
- [ ] From conversions include both fields

---

### Task 4: Update model_editor/mod.rs ModelDetail→ModelForm mapping

**Context:**
When the server returns ModelDetail JSON and it's deserialized, the code in model_editor/mod.rs manually constructs a ModelForm from the ModelDetail fields. If cache_type_k/v are added to both structs but NOT to this mapping, the values load from the server but are silently lost when constructing the form signal.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor/mod.rs` — update ModelForm construction (~line 187) and initial_form reconstruction (~line 357)

**What to implement:**

1. In the ModelForm construction (~line 187), after `gpu_layers: d.gpu_layers,`:
```rust
cache_type_k: d.cache_type_k,
cache_type_v: d.cache_type_v,
```

2. In the initial_form reconstruction (~line 357), after `gpu_layers: initial_form.gpu_layers,`:
```rust
cache_type_k: initial_form.cache_type_k,
cache_type_v: initial_form.cache_type_v,
```

**Steps:**
- [ ] Read `crates/tama-web/src/pages/model_editor/mod.rs` around line 187 to find the ModelForm construction block. Add cache_type_k and cache_type_v fields after gpu_layers.
- [ ] Read around line 357 to find the initial_form reconstruction block. Add the same two fields after gpu_layers.
- [ ] Run `cargo clippy --package tama-web --features ssr -- -D warnings`. Fix any issues before continuing.
- [ ] Commit with message: "feat(web): wire KV cache type fields through model_editor ModelForm mapping"

**Acceptance criteria:**
- [ ] ModelForm construction includes cache_type_k: d.cache_type_k and cache_type_v: d.cache_type_v
- [ ] initial_form reconstruction includes both fields from initial_form
- [ ] No clippy warnings

---

### Task 5: Add KV quantization dropdowns in the model editor UI

**Context:**
Now wire up the actual form inputs. Two select elements between "Context length" and "Num parallel slots". Each has a known-values list (f32, f16, bf16, q8_0, q4_0, q4_1, iq4_nl, q5_0, q5_1) plus an empty option at top. The form signal reads/writes cache_type_k/cache_type_v via on_change handlers. Also wire init effect to populate values when switching between models. For custom values not in the known list, a conditional text input appears below the select.

**Files:**
- Modify: `crates/tama-web/src/pages/model_editor/general_form.rs` — add two dropdown components, update Effect for initialization, insert into view! macro

**What to implement:**

1. Add constant at file level (after MODALITY_OPTIONS):
```rust
const KV_QUANT_OPTIONS: &[&str] = &["f32", "f16", "bf16", "q8_0", "q4_0", "q4_1", "iq4_nl", "q5_0", "q5_1"];
```

2. In Effect block (~line 37), add initialization:
```rust
set_input_value("field-kv-quant-k", f.cache_type_k.as_deref().unwrap_or_default());
set_input_value("field-kv-quant-v", f.cache_type_v.as_deref().unwrap_or_default());
```

3. In view! macro, insert two `<label>`/`<select>` blocks **between** the ContextLengthSelector block and the "Num parallel slots" label (~line 120). Each dropdown:
- Label for="field-kv-quant-{k|v}" with text + form-hint div explaining KV cache quantization benefit
- Select element iterating over KV_QUANT_OPTIONS plus empty option at top ("Default (f16)")  
- on_change handler updates form.cache_type_k/v — sets None when value is empty, Some(val) otherwise

4. After each select, add a conditional text input that appears only if the selected value is not in known options:
```rust
{move || {
    let current = form.get().as_ref()
        .and_then(|f| f.cache_type_k.as_deref());
    match current {
        Some(val) if !KV_QUANT_OPTIONS.contains(&val) => view! {
            <input class="form-input" type="text" placeholder="Custom quant value..." prop:value=val on:input=move |ev| {
                let v = target_value(&ev);
                form.update(|f| {
                    if let Some(form) = f {
                        form.cache_type_k = if v.is_empty() { None } else { Some(v) };
                    }
                });
            } />
        }.into(),
        _ => view! {}.into(),
    }
}}
```

**Steps:**
- [ ] Read `crates/tama-web/src/pages/model_editor/general_form.rs`. Add constant KV_QUANT_OPTIONS after MODALITY_OPTIONS block (line 14).
- [ ] In Effect (~line 37), add set_input_value calls for "field-kv-quant-k" and "field-kv-quant-v". Use `.as_deref().unwrap_or_default()` to handle None → empty string.
- [ ] Find ContextLengthSelector closing tag (around line 120) — this is where you insert two dropdown blocks, before `<label class="form-label" for="field-num-parallel">`.
- [ ] Insert first K dropdown: label with form-hint div explaining KV cache quantization benefit. Select element iterating over KV_QUANT_OPTIONS plus empty option at top ("Default (f16)"). on_change handler sets None when value is empty, Some(val) otherwise. Use event_target_value for Leptos WASM compatibility.
- [ ] Insert second V dropdown: identical pattern but "KV cache type V" and field name "field-kv-quant-v". Uses form.cache_type_v.
- [ ] After each select, add conditional text input block (pattern from step 4 above) that shows when current value is Some and not in KV_QUANT_OPTIONS.
- [ ] Run `cargo clippy --package tama-web --features ssr -- -D warnings`. Fix any issues before continuing.
- [ ] Commit with message: "feat(web): add KV cache quantization dropdowns to model editor"

**Acceptance criteria:**
- [ ] Two select elements appear between Context length and Num parallel slots in the form grid
- [ ] Each dropdown shows known values + empty option at top ("Default (f16)")  
- [ ] Selecting a value sets Some(String) on cache_type_k/v; selecting "Default" clears to None
- [ ] Conditional text input appears when value is not in known options
- [ ] Values persist when switching between models via Effect initialization

---

### Task 6: Build, test, and verify end-to-end

**Context:**
Final integration pass — run full workspace clippy + tests. Also build WASM frontend with trunk (dist directory fix from CI should handle include_dir). Verify all new fields flow correctly through every layer. This is the final quality gate before merging.

**Files:**
- Test: `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --workspace -- --nocapture`  
- Build: verify dist/ exists or run trunk build

**Steps:**
- [ ] Run `cargo fmt --all`. Did it succeed? If not, fix formatting issues before continuing.
- [ ] Run `cargo clippy --workspace --all-targets -- -D warnings`. Fix any lint errors and re-run until clean pass. This is a gate — do NOT proceed if there are failures.
- [ ] Run `cargo test --workspace -- --nocapture` — full workspace testsuite with nocapture for visibility. Did it all pass? If not, fix failing tests before continuing.
- [ ] Verify dist/ directory exists in crates/tama-web/dist/. Run: `ls crates/tama-web/dist/index.html` to confirm. If missing, run `cd crates/tama-web && trunk build`.
- [ ] Commit with message: "chore: final clippy + test pass for KV cache quantization feature"

**Acceptance criteria:**
- [ ] cargo clippy passes workspace-wide (zero lint errors)
- [ ] All tests pass including updated round_trip test  
- [ ] WASM frontend builds successfully via trunk build or dist/ exists from prior CI fix
- [ ] No uncommitted formatting issues remain
