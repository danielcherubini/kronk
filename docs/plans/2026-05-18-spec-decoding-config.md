# Spec Decoding Config Plan

**Goal:** Add a "Spec Decoding" section to the model editor that lets users enable speculative decoding (draft-mtp, ngram-simple) via checkboxes and parameters instead of raw CLI flags.

**Architecture:** New `SpecDecodingConfig` struct in `ModelConfig` stores spec type selection and parameters. On server startup, `build_full_args` injects `--spec-type`, `--spec-draft-n-max`, `--spec-draft-n-min`, and `--spec-draft-ngl` flags. The frontend renders a new section between Sampling and Quants & Vision.

**Tech Stack:** Rust (tama-core config types, tama-web Leptos frontend), SQLite migration v24

---

## Task 1: Data model, DB migration, and round-trip

**Context:**
Add `SpecDecodingConfig` to the core config types, update the DB record for persistence, and create migration v24 to add the column. This is the foundation — all other tasks depend on this.

**Files:**
- Modify: `crates/tama-core/src/config/types.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/migrations.rs`

**What to implement:**

1. In `crates/tama-core/src/config/types.rs`:
   - Add `SpecDecodingConfig` struct:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize, Default)]
     #[serde(rename_all = "camelCase")]
     pub struct SpecDecodingConfig {
         /// Enabled spec types (e.g. ["draft-mtp", "ngram-simple"]).
         /// Passed as comma-separated to --spec-type. Empty = disabled.
         #[serde(default)]
         pub spec_types: Vec<String>,
         /// Draft context length (--spec-draft-n-max). Range: 1-8.
         #[serde(default, skip_serializing_if = "Option::is_none")]
         pub n_max: Option<u32>,
         /// Minimum draft tokens (--spec-draft-n-min). Range: 1-8.
         #[serde(default, skip_serializing_if = "Option::is_none")]
         pub n_min: Option<u32>,
         /// Draft model GPU layers (--spec-draft-ngl). MTP-only.
         #[serde(default, skip_serializing_if = "Option::is_none")]
         pub draft_ngl: Option<u32>,
     }
     ```
   - Add to `ModelConfig`: `#[serde(default)] pub spec_decoding: SpecDecodingConfig,`
   - Update `to_db_record()`: `spec_decoding: serde_json::to_string(&self.spec_decoding).ok(),`
   - Update `from_db_record()`: `spec_decoding: record.spec_decoding.as_ref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default(),`

2. In `crates/tama-core/src/db/queries/types.rs`:
   - Add to `ModelConfigRecord`: `pub spec_decoding: Option<String>,`

3. In `crates/tama-core/src/db/migrations.rs`:
   - Add migration v24: `ALTER TABLE model_configs ADD COLUMN spec_decoding TEXT;`
   - Bump `LATEST_VERSION` from 23 to 24
   - Add to the migrations list with version literal `24`

**Steps:**
- [ ] Write tests in `crates/tama-core/src/config/types.rs` (`#[cfg(test)]` module):
  - `test_spec_decoding_in_model_config_toml_roundtrip` — Create a `ModelConfig` with `spec_decoding` set, serialize to TOML, deserialize, verify all fields match
  - `test_spec_decoding_missing_in_toml_defaults` — TOML string *without* `spec_decoding` section deserializes to `SpecDecodingConfig::default()`
  - Do NOT test `SpecDecodingConfig` standalone — it's always embedded in `ModelConfig`
- [ ] Run `cargo test --package tama-core test_default_sampling_templates` (any existing test to verify baseline)
- [ ] Implement `SpecDecodingConfig` struct in `types.rs`
- [ ] Add `spec_decoding` field to `ModelConfig`
- [ ] Update `to_db_record()` and `from_db_record()` methods
- [ ] Add `spec_decoding` field to `ModelConfigRecord` in `db/queries/types.rs`
- [ ] Add migration v24 to `migrations.rs`, bump `LATEST_VERSION` to 24
- [ ] Write migration test in `migrations.rs`: `test_migration_v24_adds_spec_decoding_column()` that:
  - Runs migrations up to v23, inserts a row
  - Runs v24, verifies `spec_decoding` column exists
  - Verifies NULL defaults work (existing rows get NULL)
  - Inserts a row with JSON spec_decoding, verifies round-trip
- [ ] Run `cargo test --package tama-core -- migrations`
  - Did all migration tests pass? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: add SpecDecodingConfig to model config with DB migration v24"

**Acceptance criteria:**
- [ ] `SpecDecodingConfig` serializes/deserializes correctly through TOML
- [ ] `ModelConfig` round-trips through DB record (to_db_record + from_db_record)
- [ ] Migration v24 adds column, existing rows get NULL
- [ ] All tama-core tests pass, clippy clean

---

## Task 2: Args building — inject spec decoding flags

**Context:**
When a model has spec decoding configured, `build_full_args` must inject the appropriate CLI flags. This follows the same pattern as `--kv-unified`, `-ctk`, `-ctv` — llama.cpp-only, with `already_has` checks to avoid duplicates.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/mod.rs`

**What to implement:**

In `build_full_args()`, after the cache-type-v injection block and BEFORE the sampling merge block, add:

```rust
// Inject spec-decoding flags when configured (llama.cpp backends only).
if is_llama_cpp_backend && !server.spec_decoding.spec_types.is_empty() {
    let sd = &server.spec_decoding;

    let already_has_spec_type = grouped.iter().any(|e| {
        matches!(crate::config::flag_name(e), Some("--spec-type"))
    });
    if !already_has_spec_type {
        grouped.push(format!("--spec-type {}", sd.spec_types.join(",")));
    }

    if let Some(n) = sd.n_max {
        let already_has = grouped.iter().any(|e| {
            matches!(crate::config::flag_name(e), Some("--spec-draft-n-max"))
        });
        if !already_has {
            grouped.push(format!("--spec-draft-n-max {}", n));
        }
    }

    if let Some(n) = sd.n_min {
        let already_has = grouped.iter().any(|e| {
            matches!(crate::config::flag_name(e), Some("--spec-draft-n-min"))
        });
        if !already_has {
            grouped.push(format!("--spec-draft-n-min {}", n));
        }
    }

    if sd.spec_types.iter().any(|t| t == "draft-mtp") {
        if let Some(spec_ngl) = sd.draft_ngl {
            let already_has = grouped.iter().any(|e| {
                matches!(crate::config::flag_name(e), Some("--spec-draft-ngl"))
            });
            if !already_has {
                grouped.push(format!("--spec-draft-ngl {}", spec_ngl));
            }
        }
    }
}
```

**Steps:**
- [ ] Write failing test in `crates/tama-core/src/config/resolve/tests/args_building.rs`:
  - `test_spec_decoding_flags_injected` — model with spec_decoding configured should produce --spec-type, --spec-draft-n-max, --spec-draft-n-min flags
  - `test_spec_decoding_no_duplicate_when_in_args` — if user already has --spec-type in args, don't inject another
  - `test_spec_decoding_draft_ngl_only_for_mtp` — draft_ngl only injected when draft-mtp in spec_types
  - `test_spec_decoding_empty_types_no_flags` — empty spec_types produces no flags
- [ ] Run `cargo test --package tama-core -- args_building`
  - Did the new tests fail? If not, investigate why.
- [ ] Implement the spec decoding injection in `build_full_args()`
- [ ] Run `cargo test --package tama-core -- args_building`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-core -- -D warnings`
- [ ] Run `cargo build --package tama-core`
- [ ] Commit with message: "feat: inject spec decoding CLI flags in build_full_args"

**Acceptance criteria:**
- [ ] Spec decoding flags are correctly injected into the args list
- [ ] `already_has` checks prevent duplicate flags
- [ ] `draft_ngl` only injected when `draft-mtp` is in spec_types
- [ ] Empty spec_types produces no flags

---

## Task 3: Server-side pipeline — ModelBody, apply_model_body, model_entry_json, DB queries

**Context:**
Connect the data model to the API. The frontend sends `spec_decoding` in the request body, the server deserializes it into `ModelConfig`, persists it to DB, and returns it in API responses. This touches the CRUD layer, the info layer, and the DB queries.

**Files:**
- Modify: `crates/tama-web/src/api/models/crud/mod.rs`
- Modify: `crates/tama-web/src/api/models/info.rs`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs`

**What to implement:**

1. **`ModelBody`** (`crud/mod.rs`):
   - Add: `#[serde(default)] pub spec_decoding: Option<tama_core::config::SpecDecodingConfig>,`
   - (Follows the same pattern as `sampling: Option<SamplingParams>` — typed struct, not `serde_json::Value`)

2. **`apply_model_body()`** (`crud/mod.rs`):
   - In the `base` fallback `ModelConfig`: Add `spec_decoding: Default::default(),`
   - In the returned `ModelConfig`: Add:
     ```rust
     spec_decoding: body.spec_decoding
         .or_else(|| existing.map(|m| m.spec_decoding.clone()))
         .unwrap_or_default(),
     ```

3. **`model_entry_json()`** (`info.rs`):
   - Add to the JSON: `"spec_decoding": serde_json::to_value(&m.spec_decoding).unwrap_or_default(),`

4. **DB queries** (`model_config_queries.rs`):
   - `upsert_model_config()`: Add `spec_decoding` to INSERT columns (after `hf_last_modified`), add corresponding `?` parameter, add to `ON CONFLICT DO UPDATE SET`, add `record.spec_decoding` to params
   - `get_model_config()`: Add `spec_decoding` to SELECT, new index mapping:
     - `spec_decoding = row.get(30)?`
     - `created_at = row.get(31)?` (was 29)
     - `updated_at = row.get(32)?` (was 30)
   - `get_model_config_by_repo_id()`: Same SELECT + same index mapping
   - `get_all_model_configs()`: Same SELECT + same index mapping

**Steps:**
- [ ] Add `spec_decoding` field to `ModelBody` struct (typed as `Option<SpecDecodingConfig>`, NOT `serde_json::Value`)
- [ ] Update `apply_model_body()` to handle `spec_decoding` (typed field, preserve existing on None)
- [ ] Add `spec_decoding` to `model_entry_json()` output
- [ ] Update `upsert_model_config()` SQL — INSERT columns, params, ON CONFLICT UPDATE
- [ ] Update `get_model_config()` SQL — SELECT + row.get
- [ ] Update `get_model_config_by_repo_id()` SQL — SELECT + row.get
- [ ] Update `get_all_model_configs()` SQL — SELECT + row.get
- [ ] Update all existing tests in `crud/mod.rs` that construct `ModelBody` or `ModelConfig` — add `spec_decoding: None` / `spec_decoding: Default::default()`
- [ ] Run `cargo test --package tama-web`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Run `cargo build --package tama-web`
- [ ] Commit with message: "feat: wire spec_decoding through API pipeline and DB queries"

**Acceptance criteria:**
- [ ] `ModelBody` accepts `spec_decoding` as typed `Option<SpecDecodingConfig>` (NOT `serde_json::Value`)
- [ ] `apply_model_body()` preserves existing config on partial updates
- [ ] `model_entry_json()` includes `spec_decoding` in response
- [ ] DB queries correctly insert/select the `spec_decoding` column
- [ ] All existing tests pass (updated with new field)

---

## Task 4: Frontend — Spec Decoding section, form, types, and API

**Context:**
Add the "Spec Decoding" section to the model editor UI. Users see checkboxes for spec types and conditional number inputs. The form state flows through `ModelForm` → API → server.

**Files:**
- Create: `crates/tama-web/src/pages/model_editor/spec_decoding_form.rs`
- Modify: `crates/tama-web/src/pages/model_editor/sections.rs`
- Modify: `crates/tama-web/src/pages/model_editor/types.rs`
- Modify: `crates/tama-web/src/pages/model_editor/mod.rs`
- Modify: `crates/tama-web/src/pages/model_editor/api.rs`

**What to implement:**

1. **`sections.rs`**:
   - Add `SpecDecoding` variant to `Section` enum
   - `name()` → `"Spec Decoding"`
   - `icon()` → `"⚡"`

2. **`types.rs`**:
   - Add `SpecDecodingForm` struct:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize, Default)]
     #[serde(rename_all = "camelCase")]
     pub struct SpecDecodingForm {
         pub spec_types: Vec<String>,
         pub n_max: Option<u32>,
         pub n_min: Option<u32>,
         pub draft_ngl: Option<u32>,
     }
     ```
   - Add to `ModelForm`: `pub spec_decoding: SpecDecodingForm,`
   - Add to `ModelDetail`: `#[serde(default)] pub spec_decoding: Option<serde_json::Value>,`

3. **`spec_decoding_form.rs`** (new file):
   - Component `ModelEditorSpecDecodingForm(form: RwSignal<Option<ModelForm>>) -> impl IntoView`
   - Checkboxes for `draft-mtp` and `ngram-simple` (with short descriptions)
   - When any type checked: show `n_max` dropdown (1-8) and `n_min` dropdown (1-8)
   - When `draft-mtp` checked: additionally show `draft_ngl` number input (0-999, hint "99 = all layers")
   - Use `form-grid` layout matching other sections

4. **`mod.rs`**:
   - Import and render `ModelEditorSpecDecodingForm` between Sampling and Quants sections
   - Add nav button for `Section::SpecDecoding`
   - In the form populator `Effect`, parse `spec_decoding` from `ModelDetail`:
     ```rust
     let spec_decoding = if let Some(sd_json) = &d.spec_decoding {
         serde_json::from_value(sd_json.clone()).unwrap_or_default()
     } else {
         SpecDecodingForm::default()
     };
     ```
     And set `form.spec_decoding = spec_decoding;`
   - In the `save_action`, include `spec_decoding` in `form_data`

5. **`api.rs`**:
   - In `save_model()`, add to body JSON: `"spec_decoding": form.spec_decoding,`
   - In `fetch_model()` for "new" model, add: `spec_decoding: None,`

**Steps:**
- [ ] Add `SpecDecoding` to `Section` enum in `sections.rs`
- [ ] Add `SpecDecodingForm` struct and update `ModelForm`/`ModelDetail` in `types.rs`
- [ ] Create `spec_decoding_form.rs` with the form component
- [ ] Update `mod.rs` — import, render, nav button, form populator
- [ ] Update `api.rs` — add spec_decoding to save body and new model default
- [ ] Update `mod.rs`'s `mod` declaration: `mod spec_decoding_form;`
- [ ] Run `cargo build --package tama-web --target wasm32-unknown-unknown`
  - Did it compile? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --package tama-web -- -D warnings`
- [ ] Commit with message: "feat: add Spec Decoding section to model editor UI"

**Acceptance criteria:**
- [ ] New "Spec Decoding" nav button appears between Sampling and Quants
- [ ] Checkboxes for draft-mtp and ngram-simple render correctly
- [ ] n_max/n_min dropdowns show when any type is checked
- [ ] draft_ngl input shows only when draft-mtp is checked
- [ ] Form state saves and loads correctly through the API

---

## Task 5: Tests — args building, round-trip, and integration

**Context:**
Comprehensive test coverage for the new feature: args building edge cases, DB round-trip, and full config serialization.

**Files:**
- Modify: `crates/tama-core/src/config/resolve/tests/args_building.rs`
- Modify: `crates/tama-core/src/config/types.rs` (add to existing `#[cfg(test)]` module)

**Validation note:** `draft_ngl` validation is frontend-only (min=0, max=999). No server-side validation — consistent with `gpu_layers`, `context_length`, `num_parallel` pattern.

**What to implement:**

1. **Args building tests** (expand on Task 2):
   - `test_spec_decoding_multi_type_comma_separated` — multiple spec_types produce comma-separated --spec-type value
   - `test_spec_decoding_non_llama_backend_no_flags` — non-llama backend doesn't inject spec flags
   - `test_spec_decoding_all_already_has_checks` — each of the 4 flags (--spec-type, --spec-draft-n-max, --spec-draft-n-min, --spec-draft-ngl) has its own already_has guard
   - `test_spec_decoding_draft_ngl_value_99` — draft_ngl=99 is injected as-is (not truncated, not quoted)

2. **DB round-trip test** in `types.rs` (Task 1 already covers TOML round-trip):
   - `test_model_config_spec_decoding_db_roundtrip` — Create ModelConfig with spec_decoding, call to_db_record, call from_db_record, verify spec_decoding matches

3. **JSON camelCase test** in `types.rs`:
   - `test_spec_decoding_json_camel_case_roundtrip` — Verify `serde_json::to_string` of a populated `SpecDecodingConfig` produces camelCase keys (`specTypes`, `nMax`, etc.), and deserializes back correctly

**Steps:**
- [ ] Write all tests (failing first)
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "test: add comprehensive tests for spec decoding config"

**Acceptance criteria:**
- [ ] All args building tests pass
- [ ] TOML round-trip preserves spec_decoding
- [ ] DB round-trip preserves spec_decoding
- [ ] Full workspace test suite passes
- [ ] Clippy clean across workspace

---

## Task 6: Export and README updates

**Context:**
Ensure the new type is properly exported and documentation is updated.

**Files:**
- Modify: `crates/tama-core/src/config/mod.rs`
- Modify: `docs/plans/README.md`

**What to implement:**

1. In `config/mod.rs` — add `SpecDecodingConfig` to `pub use types::{...}` exports
2. In `docs/plans/README.md` — add this plan to "Recently Completed" with status 🚧 IN PROGRESS, increment Total Plans

**Steps:**
- [ ] Export `SpecDecodingConfig` from `config/mod.rs`
- [ ] Update `docs/plans/README.md` with new plan entry
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "chore: export SpecDecodingConfig and update plans README"

**Acceptance criteria:**
- [ ] `SpecDecodingConfig` is publicly exportable from `tama_core::config`
- [ ] Plans README updated with new plan
