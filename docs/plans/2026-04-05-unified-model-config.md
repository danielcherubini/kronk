# Unified Model Config Plan

**Status:** ✅ COMPLETED - See git commits `95c8e01` ("feat: Unified Model Config - merge model cards into ModelConfig with rename support (#31)"), `13bc2d3` ("feat: expand ModelConfig with unified fields (display_name, gpu_layers, quants, sampling)"), `0be825a` ("feat: auto-migrate model cards into unified ModelConfig on config load")

**Goal:** Consolidate model cards and model configs into a single unified `ModelConfig` in `config.toml`, add editable model IDs (rename), inline sampling params (removing profile-based indirection), and add per-param enable/disable UI checkboxes.

**Architecture:** The separate model card TOML files (`configs/<org>--<model>.toml`) are eliminated. All model metadata — quants table, GPU layers, context length defaults, display name, and sampling parameters — moves directly into `[models.<id>]` entries in `config.toml`. The `Profile` enum is kept only for preset lookup (quick-fill in the UI); it is no longer stored on `ModelConfig`. Model IDs (config keys) become editable with full rename support, updating config, DB, and running model state. Community card fetching is removed entirely. Auto-migration runs on config load (backs up `config.toml` first).

**Tech Stack:** Rust (koji-core, koji-cli, koji-web/Leptos), SQLite (rusqlite), TOML (serde)

**Key decisions:**
- `source` field dropped — `model` remains the canonical HF repo ID field
- `display_name` is the friendly name for the model (was `card.model.name`)
- Migration runs automatically on `Config::load_from()` (matches existing migration pattern), with `config.toml.pre-unified-migration` backup
- `quants` uses `BTreeMap` for stable TOML serialization order
- Sampling checkboxes: checked = `Some(value)`, unchecked = `None`; `Some(0.0)` is valid and distinct from `None`

---

### Task 1: Expand `ModelConfig` with unified fields and update `Config::default()`

**Context:**
Currently, model metadata is split between `ModelConfig` (in `config.toml`) and `ModelCard` (separate TOML files in `configs/`). This task merges all model card fields into `ModelConfig`, adds a `display_name` for the model's friendly name, and changes the `profile: Option<Profile>` field to `profile: Option<String>` (deprecated, kept only for migration deserialization). The `Profile` enum stays in the codebase as a preset mechanism but is no longer stored on `ModelConfig` as an enum variant. The `sampling_templates` remain in `Config` as presets the UI can use to quick-fill values.

This task must also update `Config::default()` in `loader.rs` because it currently constructs `ModelConfig` with `profile: Some(Profile::Coding)` — which would fail to compile after the type change from `Option<Profile>` to `Option<String>`.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs`
- Modify: `crates/koji-core/src/config/loader.rs`
- Test: `crates/koji-core/src/config/types.rs` (inline `#[cfg(test)]` module)

**What to implement:**

Add `QuantEntry` to `types.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct QuantEntry {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}
```

Expand `ModelConfig` struct definition (all new fields have `#[serde(default)]`):
```rust
pub struct ModelConfig {
    // Existing fields (unchanged):
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingParams>,     // (already existed)
    #[serde(default)]
    pub model: Option<String>,               // HF repo id (canonical, e.g. "bartowski/OmniCoder-8B-GGUF")
    #[serde(default)]
    pub quant: Option<String>,               // selected quant key (e.g. "Q4_K_M")
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub context_length: Option<u32>,

    // DEPRECATED — kept for migration deserialization only.
    // When present in an old config.toml, the migration reads this, resolves it to
    // concrete SamplingParams, writes those into `sampling`, and clears this field.
    // Must NOT be serialized back (skip_serializing).
    #[serde(default, skip_serializing)]
    pub profile: Option<String>,

    // NEW fields (from ModelCard):
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,        // friendly name (was card.model.name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,             // was card.model.default_gpu_layers
    #[serde(default, skip_serializing_if = "is_btreemap_empty")]
    pub quants: BTreeMap<String, QuantEntry>, // was card.quants — BTreeMap for stable TOML order
}
```

Add the helper for skip-serializing empty BTreeMap:
```rust
fn is_btreemap_empty<K, V>(map: &BTreeMap<K, V>) -> bool {
    map.is_empty()
}
```

Add `use std::collections::BTreeMap;` at the top of `types.rs`.

In `loader.rs`, update `Config::default()`:
- Change the default model entry's `profile: Some(Profile::Coding)` to `profile: None`
- Set `sampling: Some(coding_sampling_params)` instead (use the same values as the "coding" template: temperature 0.3, top_p 0.9, etc.)
- Add the new fields with defaults: `display_name: None`, `gpu_layers: None`, `quants: BTreeMap::new()`
- Remove `use crate::profiles::Profile;` import from `loader.rs` if it was only used for this

**Steps:**
- [ ] Write test `test_model_config_with_unified_fields` in `types.rs`: constructs a `ModelConfig` with `display_name: Some("My Model")`, `gpu_layers: Some(99)`, a `quants` BTreeMap with one entry, and `sampling: Some(SamplingParams { temperature: Some(0.3), ..Default::default() })`. No `profile` field set. Serialize to TOML and deserialize back, assert all fields round-trip correctly.
- [ ] Run `cargo test --package koji-core test_model_config_with_unified_fields`
  - Should fail because the new fields don't exist yet.
- [ ] Add `QuantEntry` struct, `is_btreemap_empty` helper, `BTreeMap` import, and new fields to `ModelConfig` in `types.rs`. Change `profile` from `Option<Profile>` to `Option<String>` with `#[serde(default, skip_serializing)]`.
- [ ] Update `Config::default()` in `loader.rs`: change the default model entry to use `profile: None`, `sampling: Some(...)` with coding preset values, `display_name: None`, `gpu_layers: None`, `quants: BTreeMap::new()`. Remove the `Profile` import if unused.
- [ ] Run `cargo test --package koji-core test_model_config_with_unified_fields` — should pass.
- [ ] Write test `test_model_config_reads_legacy_profile` that deserializes TOML containing `profile = "coding"` and verifies `config.profile == Some("coding".to_string())`. Then serialize back and verify `profile` is NOT in the output (because `skip_serializing`).
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo check --package koji-core` — expect compile errors in other files that reference `profile` as `Option<Profile>`. That's expected and will be fixed in later tasks. Verify `types.rs` and `loader.rs` themselves compile.
- [ ] Commit with message: "feat: expand ModelConfig with unified fields (display_name, gpu_layers, quants, sampling)"

**Acceptance criteria:**
- [ ] `ModelConfig` has `display_name`, `gpu_layers`, `quants` (BTreeMap), and `sampling` fields
- [ ] `profile` field is `Option<String>` with `skip_serializing` (reads old configs, never writes)
- [ ] `QuantEntry` struct exists in `types.rs`
- [ ] `Config::default()` builds without referencing `Profile` enum on `ModelConfig`
- [ ] TOML round-trip test passes for the new fields
- [ ] Legacy `profile = "coding"` deserializes correctly

---

### Task 2: Write config migration from model cards to unified ModelConfig

**Context:**
This task MUST come before rewriting `build_full_args` (Task 3). The migration populates the new `ModelConfig` fields (quants, gpu_layers, sampling, display_name) from existing model card TOML files. Without this migration running first, no model's `ModelConfig` would have quant data, and `build_full_args` wouldn't be able to find GGUF paths.

Existing users have model card TOML files in `configs/` and `ModelConfig` entries in `config.toml` that reference them via the `model` field. This migration:
1. Backs up `config.toml` to `config.toml.pre-unified-migration`
2. For each model config entry with a `model` field, finds the corresponding card file
3. Copies card data (display_name, gpu_layers, quants, context_length) into the `ModelConfig`
4. Handles the deprecated `profile` field by resolving it to concrete sampling params
5. Removes the card file after successful migration
6. Also removes community card fetching code (`fetch_community_card`, `MODELCARDS_BASE_URL`)

The migration runs automatically in `Config::load_from()` (same pattern as existing migrations) and is idempotent — if a model already has quants populated and no card file exists, it's a no-op.

**Files:**
- Modify: `crates/koji-core/src/config/migrate.rs`
- Modify: `crates/koji-core/src/config/loader.rs` (wire in the new migration call)
- Modify: `crates/koji-core/src/models/pull.rs` (remove `fetch_community_card`, `MODELCARDS_BASE_URL`)
- Test: `crates/koji-core/src/config/migrate.rs` (inline `#[cfg(test)]` module)

**What to implement:**

Add `pub fn migrate_cards_to_unified_config(config: &mut Config) -> anyhow::Result<()>`:

1. Check if `configs/` directory exists. If not, return early (nothing to migrate).
2. Back up `config.toml` to `config.toml.pre-unified-migration` (only if the backup doesn't already exist — don't overwrite a previous backup).
3. Read ALL card files into memory first (for atomicity — don't partially migrate).
4. For each `(key, model_config)` in `config.models` (iterate mutably):
   - If `model_config.model` is `Some(repo_id)`:
     - Derive card filename: `repo_id.replace('/', "--") + ".toml"`
     - Look up the pre-loaded card data for this filename
     - If found:
       - Set `model_config.display_name` from `card.model.name` if `display_name` is `None`
       - Set `model_config.gpu_layers` from `card.model.default_gpu_layers` if `gpu_layers` is `None`
       - If `model_config.context_length` is `None`, set from `card.model.default_context_length`
       - For each `(quant_name, quant_info)` in `card.quants`: insert into `model_config.quants` if key not already present. Map `card::QuantInfo` → `config::QuantEntry` (same fields: file, size_bytes, context_length).
       - If `model_config.sampling` is `None`:
         - If `model_config.profile` is `Some(profile_name)`: look up `card.sampling[profile_name]`, falling back to `config.sampling_templates[profile_name]`. Set result as `model_config.sampling`.
         - If `model_config.profile` is `None` but card has a "coding" sampling entry, use that.
       - Set `model_config.profile = None` (migration complete for this entry).
5. Save config.
6. Delete migrated card files (best-effort — log warnings, don't fail the migration).
7. If `configs/` directory is now empty, remove it.

Wire into `Config::load_from()` in `loader.rs`: call `migrate_cards_to_unified_config(&mut config)?` after the existing `migrate_profiles_to_model_cards` call (or replace it). The new migration supersedes both `migrate_model_cards_to_configs` and `migrate_profiles_to_model_cards`.

Do NOT remove `fetch_community_card` or `MODELCARDS_BASE_URL` from `pull.rs` in this task — their callers in `koji-cli` and `koji_handlers` would break workspace compilation. They will be removed in Task 6b along with their callers. This task only adds the new migration function and wires it into `Config::load_from()`.

**Migration edge cases to handle:**
- Model with card but no profile → quants/gpu_layers/display_name migrated, sampling stays None
- Model with profile but no card → resolve profile from `sampling_templates`, set as sampling, clear profile
- Model with neither → no-op
- Model with existing quants and a card → card quants fill in missing keys only (don't overwrite)
- Model with no `model` field (manual config, no HF repo) → skip entirely
- Card file exists but has no matching model config → leave card file alone (orphan)

**Steps:**
- [ ] Write test `test_migrate_cards_to_unified` in `migrate.rs`:
  - Create a temp dir structure with:
    - `config.toml` containing `[models.test-model]` with `model = "org/repo"`, `quant = "Q4_K_M"`, `profile = "coding"`, `sampling` not set, `quants` empty
    - `configs/org--repo.toml` with a model card: `model.name = "TestModel"`, `model.default_gpu_layers = 99`, `model.default_context_length = 8192`, `quants.Q4_K_M = { file = "model-Q4_K_M.gguf", size_bytes = 4000000000 }`, `sampling.coding = { temperature = 0.2, top_k = 40 }`
  - Load config, run `migrate_cards_to_unified_config`
  - Assert:
    - `config.models["test-model"].display_name == Some("TestModel")`
    - `config.models["test-model"].gpu_layers == Some(99)`
    - `config.models["test-model"].context_length == Some(8192)`
    - `config.models["test-model"].quants["Q4_K_M"].file == "model-Q4_K_M.gguf"`
    - `config.models["test-model"].sampling.as_ref().unwrap().temperature == Some(0.2)`
    - `config.models["test-model"].profile == None`
    - Card file `configs/org--repo.toml` no longer exists
    - Backup file `config.toml.pre-unified-migration` exists
- [ ] Run `cargo test --package koji-core test_migrate_cards_to_unified` — should fail.
- [ ] Implement `migrate_cards_to_unified_config`.
- [ ] Run the test — should pass.
- [ ] Write test `test_migrate_idempotent` — run migration twice, verify second run is no-op.
- [ ] Write test `test_migrate_no_card_with_profile` — model has `profile = "coding"`, no card file. Verify sampling is populated from `sampling_templates["coding"]`.
- [ ] Write test `test_migrate_preserves_existing_quants` — model already has quants populated. Verify card data doesn't overwrite them.
- [ ] Wire `migrate_cards_to_unified_config` into `Config::load_from()`.
- [ ] Run `cargo test --package koji-core` to verify existing tests still pass.
- [ ] Run `cargo fmt --all && cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat: auto-migrate model cards into unified ModelConfig on config load"

**Acceptance criteria:**
- [ ] Migration reads card files and populates ModelConfig fields
- [ ] Profile field is resolved to concrete sampling params during migration
- [ ] Card files are deleted after migration
- [ ] `config.toml.pre-unified-migration` backup is created
- [ ] Migration is idempotent
- [ ] All edge cases handled (no card, no profile, no model field, existing quants)
- [ ] `fetch_community_card` and `MODELCARDS_BASE_URL` left in `pull.rs` (removed in Task 6b with callers)

---

### Task 3: Update `build_full_args` to use unified ModelConfig

**Context:**
The `build_full_args` method in `resolve.rs` currently constructs a `ModelRegistry`, scans `configs/` for card files, and extracts `-m`, `-c`, `-ngl`, and sampling args from the loaded card. After Tasks 1-2, all this data lives directly on `ModelConfig`. This task rewrites the arg-building logic to read from the unified config, eliminating the `ModelRegistry` lookup. The `effective_sampling` and `effective_sampling_with_card` methods are removed — sampling comes directly from `server.sampling`.

The `ctx_override: Option<u32>` parameter is preserved on `build_full_args` for callers that need runtime context-length overrides.

**Files:**
- Modify: `crates/koji-core/src/config/resolve.rs`
- Test: `crates/koji-core/src/config/resolve.rs` (inline `#[cfg(test)]` module)

**What to implement:**

1. Rewrite `build_full_args(server, backend, ctx_override) -> Result<Vec<String>>`:
   - Start with `backend.default_args` + `server.args` (unchanged)
   - If `server.model` is `Some(model_id)` and `server.quant` is `Some(quant_name)`:
     - Look up `server.quants.get(quant_name)` to get the `QuantEntry`
     - If found, resolve GGUF path as `self.models_dir()? / model_id / quant_entry.file`
     - Inject `-m <path>` if not already in args
   - Context length priority: `ctx_override` > `server.context_length` > `server.quants[quant].context_length`
   - If a context length is resolved, inject `-c <value>` if not already in args
   - If `server.gpu_layers` is `Some(ngl)`, inject `-ngl <ngl>` if not already in args
   - If `server.sampling` is `Some(params)` and `!params.is_empty()`:
     - Call `params.to_args()`, deduplicate against existing args (same flag-stripping logic as before), and append
   - Return the final args vec

2. Remove `effective_sampling(server)` method entirely.
3. Remove `effective_sampling_with_card(server, card)` method entirely.
4. Simplify `build_args(server, backend)` to: `backend.default_args + server.args + server.sampling.to_args()` (same dedup logic).

5. Remove the `use crate::models::ModelRegistry` import and any `use crate::models::card::ModelCard` import from `resolve.rs`.
6. Keep `resolve_server`, `resolve_servers_for_model`, all health/URL resolution, and backend path resolution unchanged.

**Steps:**
- [ ] Write test `test_build_full_args_unified` in `resolve.rs`:
  - Create a `Config` with `models_dir` pointing to a temp dir
  - Add a `ModelConfig` with: `model: Some("org/repo")`, `quant: Some("Q4_K_M")`, `quants: {"Q4_K_M": QuantEntry { file: "model-Q4_K_M.gguf", .. }}`, `gpu_layers: Some(99)`, `context_length: Some(4096)`, `sampling: Some(SamplingParams { temperature: Some(0.3), ..Default::default() })`
  - Create the temp file at `<temp>/models/org/repo/model-Q4_K_M.gguf`
  - Call `config.build_full_args(server, backend, None)`
  - Assert args contain: `-m .../org/repo/model-Q4_K_M.gguf`, `-c 4096`, `-ngl 99`, `--temp 0.30`
- [ ] Write test `test_build_full_args_ctx_override` — pass `ctx_override: Some(2048)`, verify `-c 2048` (not 4096).
- [ ] Write test `test_build_full_args_no_sampling` — `sampling: None`, verify no `--temp`, `--top-k`, etc.
- [ ] Write test `test_build_full_args_no_quants` — `model` and `quant` set but `quants` map is empty (not yet migrated). Verify no `-m` arg emitted (graceful degradation, not a crash).
- [ ] Run tests — should fail.
- [ ] Implement the new `build_full_args` logic. Remove `effective_sampling` and `effective_sampling_with_card`.
- [ ] Run tests — should pass.
- [ ] Update/fix any existing tests in `resolve.rs` that constructed `ModelConfig` with `profile: Some(Profile::Coding)` — change to `profile: None` or remove.
- [ ] Run `cargo test --package koji-core -- config::resolve::tests`
- [ ] Run `cargo fmt --all && cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "refactor: build_full_args reads from unified ModelConfig instead of ModelCard"

**Acceptance criteria:**
- [ ] `build_full_args` no longer constructs `ModelRegistry` or loads card files
- [ ] `-m`, `-c`, `-ngl` args come from `ModelConfig` fields directly
- [ ] Context length priority: ctx_override > server.context_length > quant.context_length
- [ ] Sampling args come from `server.sampling` only (no profile/template merge)
- [ ] Empty/None sampling produces no sampling CLI args
- [ ] Missing quants entry doesn't crash (graceful no-op)
- [ ] All resolve tests updated and passing

---

### Task 4: Add `rename_active_model` DB function and `ProxyState::rename_model`

**Context:**
Model IDs (config keys like "my-coding-model") are used as the primary key `server_name` in the `active_models` DB table, and as keys in both `Config.models` HashMap and the in-memory `ProxyState.models` map. To support renaming, we need: (1) a DB function that updates the PK, (2) logic to rename the key in config and in-memory state. The rename must be atomic with rollback if config save fails. The API/UI endpoints for triggering rename come in Task 5.

**Files:**
- Modify: `crates/koji-core/src/db/queries.rs`
- Create: `crates/koji-core/src/proxy/rename.rs`
- Modify: `crates/koji-core/src/proxy/mod.rs` (add `mod rename;`)
- Test: `crates/koji-core/src/db/queries.rs` (inline tests)

**What to implement:**

1. Add `rename_active_model(conn, old_name, new_name) -> Result<()>` to `queries.rs`:
   ```rust
   pub fn rename_active_model(conn: &Connection, old_name: &str, new_name: &str) -> Result<()> {
       conn.execute(
           "UPDATE active_models SET server_name = ?2 WHERE server_name = ?1",
           [old_name, new_name],
       )?;
       Ok(())
   }
   ```

2. Add `rename_model` method on `ProxyState` in `proxy/rename.rs`:
   ```rust
   impl ProxyState {
       pub async fn rename_model(&self, old_name: &str, new_name: &str) -> Result<()>
   }
   ```
   Logic:
   - Validate: `new_name` is not empty, `old_name != new_name`
   - Take a write lock on `self.config`:
     - Check `config.models` contains `old_name`
     - Check `config.models` does NOT contain `new_name` (error: "name already taken")
     - Remove the entry at `old_name`, insert at `new_name`
     - Attempt `config.save()`
     - **If save fails: rollback** — remove `new_name`, re-insert at `old_name`, return error
   - Take a write lock on `self.models`:
     - If `old_name` exists in the map, remove and re-insert at `new_name`
   - DB update (best-effort): call `rename_active_model(conn, old_name, new_name)` if db is available

3. Add `mod rename;` to `crates/koji-core/src/proxy/mod.rs`.

**Steps:**
- [ ] Write test `test_rename_active_model` in `queries.rs`: insert an active model with name "old-name", call `rename_active_model("old-name", "new-name")`, verify `get_active_models` returns `server_name == "new-name"` and "old-name" is gone.
- [ ] Write test `test_rename_active_model_not_found` — rename a name that doesn't exist, verify no error (0 rows affected is OK).
- [ ] Run `cargo test --package koji-core test_rename_active_model` — should fail.
- [ ] Implement `rename_active_model` in `queries.rs`.
- [ ] Run tests — should pass.
- [ ] Create `proxy/rename.rs` with `ProxyState::rename_model`. Add `mod rename;` to `proxy/mod.rs`.
- [ ] Run `cargo fmt --all && cargo clippy --package koji-core -- -D warnings`
- [ ] Commit with message: "feat: add rename_active_model DB function and ProxyState::rename_model"

**Acceptance criteria:**
- [ ] `rename_active_model` function exists and is tested
- [ ] `ProxyState::rename_model` updates config, in-memory model map, and DB
- [ ] Rename fails with error if new name already exists
- [ ] Rename fails with error if old name doesn't exist
- [ ] Config save failure triggers rollback of the in-memory rename
- [ ] Rename works for models not currently loaded (config-only rename)

---

### Task 5: Update Web API and model editor UI

**Context:**
The web API (`koji-web/src/api.rs`) currently has separate endpoints for model config CRUD (`/api/models/:id`) and model card CRUD (`/api/models/:id/card`). After consolidation, the card endpoints are removed. The model config endpoints are updated to include the new unified fields. A new `POST /api/models/:id/rename` endpoint is added. The model editor UI (`model_editor.rs`) is redesigned from a two-panel layout (Model Config + Model Card) into a single unified form with sections. Sampling params get per-field enable/disable checkboxes.

The web crate (`koji-web`) is a separate process from the proxy. It has no direct access to `ProxyState` or the DB. The rename handler only updates `config.toml` — the DB is updated when the proxy next loads the model. This matches the existing architecture where the web crate only reads/writes config files.

**Files:**
- Modify: `crates/koji-web/src/api.rs`
- Modify: `crates/koji-web/src/server.rs` (router)
- Modify: `crates/koji-web/src/pages/model_editor.rs`
- Test: Manual testing via browser (Leptos WASM apps don't support unit tests)

**What to implement:**

**API changes (`api.rs`):**

1. Remove `get_model_card` and `save_model_card` handler functions.
2. Remove helper functions: `card_path`, `load_card`, `card_to_json`.
3. Remove `CardBody` and `CardQuantBody` structs.
4. Update `ModelBody` request struct:
   ```rust
   #[derive(serde::Deserialize)]
   pub struct ModelBody {
       pub backend: String,
       #[serde(default)]
       pub model: Option<String>,
       #[serde(default)]
       pub quant: Option<String>,
       #[serde(default)]
       pub args: Vec<String>,
       // REMOVED: pub profile: Option<String>,
       #[serde(default)]
       pub sampling: Option<koji_core::profiles::SamplingParams>,
       #[serde(default)]
       pub enabled: Option<bool>,
       #[serde(default)]
       pub context_length: Option<u32>,
       #[serde(default)]
       pub port: Option<u16>,
       // NEW:
       #[serde(default)]
       pub display_name: Option<String>,
       #[serde(default)]
       pub gpu_layers: Option<u32>,
       #[serde(default)]
       pub quants: Option<BTreeMap<String, koji_core::config::QuantEntry>>,
   }
   ```
5. Update `apply_model_body`: remove `Profile` parsing/import. Map all body fields to `ModelConfig`:
   - `sampling: body.sampling` (replaces the old `sampling: base.sampling` — the API now accepts sampling from the client)
   - `display_name: body.display_name`
   - `gpu_layers: body.gpu_layers`
   - `quants: body.quants.unwrap_or_default()`
   - `profile: None` always
6. Update `model_entry_json`: remove card loading (`load_card` call). Serialize all fields directly from `ModelConfig`:
   ```rust
   serde_json::json!({
       "id": id,
       "backend": m.backend,
       "model": m.model,
       "quant": m.quant,
       "args": m.args,
       "sampling": m.sampling,
       "enabled": m.enabled,
       "context_length": m.context_length,
       "port": m.port,
       "display_name": m.display_name,
       "gpu_layers": m.gpu_layers,
       "quants": m.quants,
   })
   ```
7. Add `rename_model` handler:
   ```rust
   #[derive(serde::Deserialize)]
   pub struct RenameBody { pub new_id: String }

   pub async fn rename_model(
       State(state): State<Arc<AppState>>,
       Path(id): Path<String>,
       Json(body): Json<RenameBody>,
   ) -> impl IntoResponse
   ```
   This handler loads config, checks `id` exists and `body.new_id` doesn't, renames the key in `config.models`, saves config, returns `200 { "id": new_id }`.

8. Update router in `server.rs`:
   - Remove: `GET /api/models/:id/card`, `PUT /api/models/:id/card`
   - Add: `POST /api/models/:id/rename`

**UI changes (`model_editor.rs`):**

1. Remove the entire "Model Card" panel (the second `form-card--wide card` div containing card name, source, GPU layers, and quants table).
2. Merge into the main form these sections:

   **Section: Identity**
   - "ID" — text input, editable always (not disabled for existing models). Store `original_id` signal to detect renames.
   - "Display Name" — text input (maps to `display_name`)
   - "Backend" — select (unchanged)
   - "Model (HF repo)" — text input (maps to `model`, unchanged)
   - "Quant" — text input (maps to `quant`, the selected quant key)
   - "Enabled" — checkbox (unchanged)

   **Section: Hardware**
   - "Context Length" — number input (maps to `context_length`)
   - "GPU Layers" — number input (maps to `gpu_layers`)
   - "Port Override" — number input (maps to `port`)

   **Section: Sampling Parameters**
   - "Load Preset" dropdown: options are "" (none), "coding", "chat", "analysis", "creative". Selecting one fetches the `sampling_templates` values from a new `GET /api/sampling-templates` endpoint (or hardcoded in the frontend — simpler). Populates the fields below and checks all relevant checkboxes.
   - For each of the 7 sampling params (temperature, top_k, top_p, min_p, presence_penalty, frequency_penalty, repeat_penalty):
     - A checkbox + label (e.g. "Temperature")
     - A number input next to it
     - Checkbox is checked if the value is `Some(_)` when loaded
     - When checkbox is unchecked: number input is disabled and grayed out, value becomes `None`
     - When checkbox is checked with no value entered: treat as `Some(0.0)` or leave the field focused for input
     - `Some(0.0)` is a valid distinct value (greedy decoding for temperature, for example)

   **Section: Quants**
   - Same table as before (moved from card panel): Name, File, Size (bytes), Context length, delete button, "+ Add Quant" button

   **Section: Extra Args**
   - Textarea for extra args (unchanged)

3. Save logic: single "Save" button that:
   - If `form_id != original_id` (user renamed): call `POST /api/models/:original_id/rename` with `{ "new_id": form_id }`. On success, update `original_id` signal.
   - Then call `PUT /api/models/:form_id` with all fields (or `POST /api/models` if new).
   - Collect sampling params: for each param, if checkbox is checked and value is non-empty, include as `Some(value)`. If checkbox is unchecked, send `null`/omit.
   - Collect quants from the table rows into the `quants` BTreeMap.

4. Remove all `card_*` signals, `save_card_action`, `has_card` signal, and the separate card status message.

5. Frontend types to update:
   - `ModelForm`: add `display_name: String`, `gpu_layers: String`, remove reliance on card
   - `ModelDetail`: add `display_name`, `gpu_layers`, `quants`, `sampling` (remove `card: Option<CardData>`)
   - Remove `CardData`, keep local `QuantRow` struct
   - Add sampling field signals: for each of 7 params, two signals: `RwSignal<bool>` (enabled checkbox) and `RwSignal<String>` (value)

**Steps:**
- [ ] Update `ModelBody`, `apply_model_body`, and `model_entry_json` in `api.rs`.
- [ ] Remove card-related endpoints, helpers, and request types from `api.rs`.
- [ ] Add `rename_model` handler and `RenameBody` struct in `api.rs`.
- [ ] Update router in `server.rs` — remove card routes, add rename route.
- [ ] Rewrite `model_editor.rs`:
  - Remove card panel and all card-related signals/actions
  - Add display_name, gpu_layers fields to main form
  - Move quants table into main form
  - Make ID field editable with rename-on-save logic
  - Add sampling params section with 7 checkbox+input pairs
  - Add "Load Preset" dropdown
  - Update `ModelDetail`, `ModelForm` types
  - Single save button
- [ ] Run `cargo build --package koji-web` to verify compilation.
- [ ] Run `cargo fmt --all && cargo clippy --package koji-web -- -D warnings`
- [ ] Commit with message: "feat: unified model editor UI with sampling checkboxes and rename support"

**Acceptance criteria:**
- [ ] No separate card endpoints exist (`/api/models/:id/card` routes removed)
- [ ] Model editor is a single unified form (no two-panel layout)
- [ ] Each sampling param has a checkbox to enable/disable
- [ ] "Load Preset" dropdown populates sampling fields from templates
- [ ] Model ID is editable; renaming calls the rename endpoint before saving
- [ ] Quants table is in the main form
- [ ] Single save button saves everything
- [ ] Compilation succeeds for `koji-web`

---

### Task 6: Update CLI, proxy handlers, and core modules

**Context:**
After Tasks 1-5, many files across all 3 crates still reference `ModelCard`, `Profile` (as an enum on `ModelConfig`), `ModelRegistry` (for card lookups), and community card fetching. This task updates all remaining consumers. It's broken into sub-steps by area to keep changes manageable.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers.rs`
- Modify: `crates/koji-core/src/proxy/state.rs`
- Modify: `crates/koji-core/src/proxy/lifecycle.rs`
- Modify: `crates/koji-core/src/proxy/status.rs`
- Modify: `crates/koji-core/src/models/registry.rs`
- Modify: `crates/koji-core/src/models/mod.rs`
- Delete: `crates/koji-core/src/models/card.rs`
- Modify: `crates/koji-core/src/db/backfill.rs`
- Modify: `crates/koji-cli/src/commands/model.rs`
- Modify: `crates/koji-cli/src/handlers/profile.rs`
- Modify: `crates/koji-cli/src/handlers/status.rs`
- Modify: `crates/koji-cli/src/handlers/server/add.rs`
- Modify: `crates/koji-cli/src/handlers/server/edit.rs`
- Modify: `crates/koji-cli/src/handlers/server/ls.rs`
- Modify: `crates/koji-cli/src/cli.rs`
- Modify: `crates/koji-cli/src/lib.rs`
- Modify: `crates/koji-cli/tests/tests.rs`

**What to implement:**

**Step 6a: Core library — models module and DB backfill**

1. Delete `crates/koji-core/src/models/card.rs`.
2. Update `crates/koji-core/src/models/mod.rs`: remove `pub mod card;` and re-exports of `ModelCard`, `ModelMeta`, `QuantInfo`. Keep `pub mod registry;` and `pub mod pull;`.
3. Simplify `crates/koji-core/src/models/registry.rs`:
   - Remove `configs_dir` field (no more card scanning).
   - `ModelRegistry::new` takes only `models_dir: PathBuf`.
   - Remove `InstalledModel.card` and `InstalledModel.card_path` fields. The struct only needs `dir: PathBuf` and `id: String`.
   - `scan()` now just scans `models_dir` for `<org>/<model>/` subdirectories that contain `.gguf` files. It doesn't load any TOML.
   - `find()` looks up by `id` (same as before, but no card loading).
   - `gguf_path()` — takes `id`, `quant_filename` (not quant name). Returns `models_dir/id/filename`.
   - `untracked_ggufs()` — takes `model_dir: &Path` and `tracked_files: &HashSet<&str>` (instead of `&ModelCard`).
   - Update all tests in `registry.rs` to not create card files.
4. Update `crates/koji-core/src/db/backfill.rs`: change `ModelRegistry::new` call to single arg. Don't load cards. Backfill by scanning `models/` for GGUF files and matching against `config.models` entries.

After this step: `cargo build --package koji-core` should compile (though `koji-cli` and `koji-web` may still have errors).

**Step 6b: Core library — proxy handlers**

1. `proxy/koji_handlers.rs`:
   - In `_setup_model_after_pull_with_config` (and its callers): remove all card file creation. Instead, create/update the `ModelConfig` entry directly in `config.models` with `display_name`, `quants`, `gpu_layers`, etc. Remove `ModelCard`/`ModelMeta`/`QuantInfo` imports. Rename the `QuantEntry` struct in this file (used for HF API response parsing) to `HfQuantEntry` to avoid collision with `config::QuantEntry`.
   - Remove community card merging logic — remove the caller of `fetch_community_card`.
   - Also in this step: remove `fetch_community_card()` function and `MODELCARDS_BASE_URL` constant from `crates/koji-core/src/models/pull.rs` (deferred from Task 2 to keep workspace compilable). Remove the `use crate::models::card::ModelCard` import from `pull.rs`.
2. `proxy/state.rs`: Remove `get_model_card` method (it loaded card TOML files). If anything calls it, replace with reading from `config.models[key]` directly.
3. `proxy/lifecycle.rs`: Remove `_model_card: Option<&crate::models::card::ModelCard>` parameter from `load_model`. Update the signature to just `pub async fn load_model(&self, model_name: &str) -> Result<String>`. Update all callers (search for `load_model(` across the codebase).
4. `proxy/status.rs`: Replace `m.profile` serialization with `m.sampling` serialization. Where it currently outputs `"profile": model_config.profile.as_ref().map(|p| p.to_string())`, change to `"sampling": model_config.sampling`.

After this step: `cargo build --package koji-core` should compile cleanly.

**Step 6c: CLI commands and handlers**

1. `commands/model.rs`:
   - `cmd_pull`: Remove `fetch_community_card` call (already deleted in Task 2, just remove the caller). After downloading GGUF files, create/update `ModelConfig` in config.toml directly with `display_name`, `quants` (with the downloaded file info), `gpu_layers`, etc. No card file creation. No card file loading.
   - `cmd_info`: Read all info from the `ModelConfig` in config instead of loading a separate card file.
   - `cmd_rm`: Don't try to delete a card file. Just remove the config entry.
   - `cmd_sync` (called `cmd_scan` in some contexts): Build `ModelConfig` entries directly instead of creating card files. Update `ModelRegistry::new` calls to single arg.
   - All `ModelRegistry::new(models_dir, configs_dir)` calls → `ModelRegistry::new(models_dir)`.
2. `handlers/profile.rs`:
   - `profile list`: Keep as-is — shows available presets from `sampling_templates`.
   - `profile set <model> <preset>`: Instead of setting `model_config.profile = Some(Profile::...)`, look up `config.sampling_templates[preset]`, set `model_config.sampling = Some(template_params)`, set `model_config.profile = None`, save config.
   - `profile clear <model>`: Set `model_config.sampling = None`, save config.
   - Remove `use koji_core::profiles::Profile;` import if only used for matching.
3. `handlers/server/add.rs`: Remove `Profile` parsing. When constructing a new `ModelConfig`, set `profile: None`. Update `ModelRegistry::new` call to single arg. Quant validation now checks against the `ModelConfig.quants` map (loaded from config) instead of scanning card files.
4. `handlers/server/edit.rs`: Remove `Profile` parsing. If the user is editing sampling, update `model_config.sampling` directly.
5. `handlers/server/ls.rs`: Display `sampling` summary (e.g. "temp=0.3 top_k=50") instead of profile name.
6. `handlers/status.rs`: Display `sampling` params instead of `profile` name.
7. `cli.rs`: The `koji profile` subcommand stays (it's reused as "koji sampling preset" essentially). No structural changes needed.
8. `lib.rs`: Update doc comments about model cards.
9. `tests/tests.rs`: Update any test that constructs `ModelConfig` with `profile: Some("chat")` or references `Profile` enum.

After this step: `cargo build --workspace` should compile cleanly.

**Steps:**
- [ ] **6a**: Delete `card.rs`, update `mod.rs`, simplify `registry.rs`, update `backfill.rs`. Run `cargo build --package koji-core`.
- [ ] **6b**: Update `koji_handlers.rs` (rename `QuantEntry` → `HfQuantEntry`, remove card creation), `state.rs` (remove `get_model_card`), `lifecycle.rs` (remove card parameter from `load_model`), `status.rs` (sampling instead of profile). Run `cargo build --package koji-core`.
- [ ] **6c**: Update all CLI files. Run `cargo build --package koji-cli`.
- [ ] Run `cargo build --workspace` — should compile.
- [ ] Run `cargo test --workspace` — fix any broken tests.
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "refactor: remove ModelCard, update all CLI/proxy code to use unified ModelConfig"

**Acceptance criteria:**
- [ ] `cargo build --workspace` compiles cleanly
- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `models/card.rs` deleted
- [ ] No remaining imports of `ModelCard`, `ModelMeta`, or `card::QuantInfo`
- [ ] `load_model` no longer takes a model card parameter
- [ ] `ModelRegistry` no longer has `configs_dir` or card-loading logic
- [ ] All profile references in CLI handlers replaced with sampling
- [ ] `HfQuantEntry` used in `koji_handlers.rs` (no naming collision)

---

### Task 7: Cleanup, remove dead code, and final verification

**Context:**
After all changes, clean up dead code, remove the deprecated `profile` string field from `ModelConfig` (it was `skip_serializing` but still deserializable), remove the `modelcards/` directory from the repo, remove now-unnecessary migration functions, update doc comments, and verify the full workflow.

**Files:**
- Modify: `crates/koji-core/src/config/types.rs` (remove `profile` field)
- Modify: `crates/koji-core/src/config/loader.rs` (clean up default config)
- Modify: `crates/koji-core/src/config/migrate.rs` (remove legacy migration functions)
- Delete: `modelcards/` directory (community card templates in the repo)
- Modify: `crates/koji-core/src/lib.rs` (update doc comments)
- Modify: `crates/koji-cli/src/lib.rs` (update doc comments)
- Modify: `crates/koji-core/src/config/defaults.rs` (update/remove comments about profiles)

**What to implement:**

1. Remove the `profile: Option<String>` field from `ModelConfig` entirely. Any old `config.toml` with `profile = "coding"` that hasn't been migrated will now fail to deserialize — this is acceptable because Task 2's migration runs first on every load. If paranoid, keep the field but add `#[serde(skip)]` (skip both ser and de) so it's completely ignored.
   - **Decision:** Keep `#[serde(default, skip)]` for one release cycle, then remove in a future version. This way old configs don't hard-fail.

2. Remove `Config::configs_dir()` method from `types.rs` if no callers remain. If the migration still needs it, keep it but mark as `#[deprecated]`.

3. Remove legacy migration functions from `migrate.rs`:
   - `migrate_model_cards_to_configs` — fully superseded
   - `migrate_profiles_to_model_cards` — fully superseded
   - Keep `rename_legacy_directories` (still useful) and `migrate_cards_to_unified_config` (the new one)

4. Update `Config::load_from()` in `loader.rs`: remove calls to the deleted migration functions. Keep only `rename_legacy_directories` and `migrate_cards_to_unified_config`.

5. Delete the `modelcards/` directory from the repo root.

6. Update doc comments:
   - `koji-core/src/lib.rs`: remove references to "model cards in configs/" and "profile-based sampling"
   - `koji-cli/src/lib.rs`: same
   - `config/defaults.rs`: remove comment about "Profile resolution is now handled via Config.sampling_templates" (outdated)

7. Run `make check` (fmt + clippy + test).

**Steps:**
- [ ] Change `profile` field on `ModelConfig` to `#[serde(default, skip)]`.
- [ ] Remove or deprecate `Config::configs_dir()`.
- [ ] Remove `migrate_model_cards_to_configs` and `migrate_profiles_to_model_cards` from `migrate.rs`.
- [ ] Update `Config::load_from()` — remove calls to deleted migrations.
- [ ] Delete `modelcards/` directory.
- [ ] Update doc comments in `lib.rs` files and `defaults.rs`.
- [ ] Run `make check` (or `cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace`).
- [ ] Fix any remaining issues.
- [ ] Commit with message: "chore: cleanup deprecated profile field, remove modelcards/, remove legacy migrations"

**Acceptance criteria:**
- [ ] `make check` passes (fmt + clippy + test)
- [ ] No dead code warnings from clippy
- [ ] `modelcards/` directory removed
- [ ] Legacy migration functions removed
- [ ] `profile` field on `ModelConfig` is `#[serde(default, skip)]` (ignored)
- [ ] Doc comments updated
- [ ] End-to-end: a model can be created, configured with sampling params, renamed, loaded, queried via API with the friendly ID, and unloaded — all using the unified config
