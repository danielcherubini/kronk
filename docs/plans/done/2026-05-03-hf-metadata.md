# HF Metadata for Models Plan

**Goal:** Add 9 HuggingFace metadata columns to `model_configs`, populate them from the HF API + README parsing, display architecture type on model cards, and sort models by display name on the dashboard.

**Architecture:** New nullable columns on `model_configs` table (migration v19). A new `fetch_hf_metadata()` function in `tama-core/src/models/pull.rs` calls the HF API and parses README markdown for architecture details. The existing `refresh_metadata()` flow calls this function. `ModelStatus` gains an `hf_architecture_type` field so it flows through SSE → dashboard. The `ModelCard` component renders a new architecture badge pill.

**Tech Stack:** Rust (tama-core, tama-web), SQLite, hf-hub crate, Leptos

---

### Task 1: DB Migration v19 + Rust Type Updates

**Context:**
Add 9 nullable columns to the `model_configs` table and update all Rust types that mirror the DB schema. This is the foundation — all other tasks depend on these types existing. The migration follows the established pattern of simple `ALTER TABLE ADD COLUMN` statements (like v14-v18).

**Files:**
- Modify: `crates/tama-core/src/db/migrations.rs`
- Modify: `crates/tama-core/src/db/queries/types.rs`
- Modify: `crates/tama-core/src/db/queries/model_config_queries.rs`
- Modify: `crates/tama-core/src/config/types.rs`

**What to implement:**

1. **Migration v19** in `migrations.rs`:
   - Increment `LATEST_VERSION` from 18 to 19
   - Add migration tuple `(19, SQL)` with 9 `ALTER TABLE` statements:
     ```sql
     ALTER TABLE model_configs ADD COLUMN hf_format TEXT;
     ALTER TABLE model_configs ADD COLUMN hf_base_model TEXT;
     ALTER TABLE model_configs ADD COLUMN hf_pipeline_tag TEXT;
     ALTER TABLE model_configs ADD COLUMN hf_total_params TEXT;
     ALTER TABLE model_configs ADD COLUMN hf_active_params TEXT;
     ALTER TABLE model_configs ADD COLUMN hf_architecture_type TEXT;
     ALTER TABLE model_configs ADD COLUMN hf_context_length INTEGER;
     ALTER TABLE model_configs ADD COLUMN hf_num_layers INTEGER;
     ALTER TABLE model_configs ADD COLUMN hf_last_modified TEXT;
     ```
   - Add a regression test `test_migration_v19_adds_hf_metadata_columns` that verifies all 9 columns exist after migration.

2. **`ModelConfigRecord`** in `db/queries/types.rs`:
   Add 9 new optional fields **between `health_check` and `created_at`** (audit fields `created_at`/`updated_at` are always last):
   ```rust
   pub hf_format: Option<String>,
   pub hf_base_model: Option<String>,
   pub hf_pipeline_tag: Option<String>,
   pub hf_total_params: Option<String>,
   pub hf_active_params: Option<String>,
   pub hf_architecture_type: Option<String>,
   pub hf_context_length: Option<u32>,
   pub hf_num_layers: Option<u32>,
   pub hf_last_modified: Option<String>,
   ```

3. **`model_config_queries.rs`** — update all 4 functions:
   - `upsert_model_config`: Add the 9 new columns to INSERT (as `?22` through `?30`) and to ON CONFLICT DO UPDATE SET clause. The current INSERT has 21 params (`?1`-`?21`), so the new columns are `?22`-`?30`. Add matching entries to `params![]` macro call.
   - `get_model_config`: Add the 9 new columns to SELECT list (after `updated_at`, before the closing quote) and row mapping (`row.get(22)` through `row.get(30)`). Column order in SELECT must exactly match the INSERT VALUES order.
   - `get_model_config_by_repo_id`: Same column additions as `get_model_config`
   - `get_all_model_configs`: Same column additions as `get_model_config`

4. **`ModelConfig`** in `config/types.rs`:
   Add the same 9 optional fields to the struct. All must have `#[serde(default, skip_serializing_if = "Option::is_none")]` so they're never written to TOML config files (they're DB-only metadata). Update `to_db_record()` to include them and `from_db_record()` to read them back. In `from_db_record()`, treat empty strings as `None` (same pattern as `api_name`).
   
   Also update the existing `test_model_config_round_trip` test: set `hf_architecture_type: Some("MoE".to_string())` and `hf_total_params: Some("35B".to_string())` on the input `ModelConfig`, and verify they survive the round-trip through `to_db_record()` → `from_db_record()`.

**Steps:**
- [ ] Write a failing test in `migrations.rs` tests module that expects the 9 new columns to exist after migration v19
- [ ] Run `cargo test --package tama-core migrations::tests::test_migration_v19_adds_hf_metadata_columns`
  - Did it fail because LATEST_VERSION is still 18? If so, proceed.
- [ ] Implement migration v19 in `migrations.rs` (increment LATEST_VERSION to 19, add migration tuple)
- [ ] Run `cargo test --package tama-core migrations::tests::test_migration_v19_adds_hf_metadata_columns`
  - Did it pass? If not, fix and re-run.
- [ ] Add the 9 new fields to `ModelConfigRecord` in `types.rs` (between `health_check` and `created_at`)
- [ ] Update `upsert_model_config`, `get_model_config`, `get_model_config_by_repo_id`, `get_all_model_configs` in `model_config_queries.rs`
- [ ] Add the 9 new fields to `ModelConfig` in `config/types.rs` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
- [ ] Update `to_db_record()` and `from_db_record()` methods
- [ ] Update `test_model_config_round_trip` test to verify `hf_architecture_type` and `hf_total_params` survive round-trip
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix compilation errors and re-run.
- [ ] Run `cargo test --package tama-core`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add HF metadata columns to model_configs (migration v19)"

**Acceptance criteria:**
- [ ] Migration v19 adds all 9 nullable columns to `model_configs`
- [ ] `ModelConfigRecord` has all 9 new optional fields
- [ ] `ModelConfig` has all 9 new optional fields with round-trip through DB record
- [ ] All 4 query functions (upsert, get by id, get by repo_id, get all) include the new columns
- [ ] `cargo test --package tama-core` passes
- [ ] `cargo clippy --package tama-core -- -D warnings` passes

---

### Task 2: HF Metadata Fetcher

**Context:**
A new function that fetches model metadata from the HuggingFace API and parses the README for architecture details. This is called during pull, refresh, and backfill. It uses the existing `hf_api()` helper from `pull.rs` which provides the `hf-hub` crate's `Api` client.

**Files:**
- Modify: `crates/tama-core/src/models/pull.rs`
- Modify: `crates/tama-core/src/models/update.rs`

**What to implement:**

1. **New struct `HfModelMetadata`** in `pull.rs`:
   ```rust
   #[derive(Debug, Clone, Default)]
   pub struct HfModelMetadata {
       pub hf_format: Option<String>,
       pub hf_base_model: Option<String>,
       pub hf_pipeline_tag: Option<String>,
       pub hf_total_params: Option<String>,
       pub hf_active_params: Option<String>,
       pub hf_architecture_type: Option<String>,
       pub hf_context_length: Option<u32>,
       pub hf_num_layers: Option<u32>,
       pub hf_last_modified: Option<String>,
   }
   ```
   Note: No `Serialize`/`Deserialize` — this is an internal data-transfer type between the fetcher and the DB update helper, never serialized to JSON/TOML.

2. **New async function `fetch_hf_metadata(repo_id: &str) -> Result<HfModelMetadata>`** in `pull.rs`:
   - Internally calls `hf_api().await?` to get the `Api` client (same pattern as `list_gguf_files` and `fetch_blob_metadata`)
   - Call HF API for repo info: use `api.info(repo_id, ...).await` to get model metadata
   - Extract from API JSON response:
     - `hf_format`: Always `Some("gguf".to_string())` since tama only supports GGUF models (future-proofing for safetensors)
     - `hf_base_model`: From `tags[]` — find the tag starting with `"base_model:"` that does NOT start with `"base_model:quantized:"`. Strip the `"base_model:"` prefix.
     - `hf_pipeline_tag`: From `.pipeline_tag` or `.cardData.pipeline_tag` (prefer top-level)
     - `hf_last_modified`: From `.lastModified` ISO timestamp
   - Fetch README: use `api.client().get(&readme_url)` (same reqwest client as `fetch_blob_metadata`) to fetch `https://huggingface.co/{repo_id}/raw/main/README.md`
   - Parse README via a **pure helper function** `parse_readme_metadata(markdown: &str) -> HfModelMetadata` — extracted for testability (matches the codebase's pattern of separating I/O from logic, e.g., `parse_blob_siblings`):
     - `hf_total_params`: Extract from lines like "Number of Parameters: 35B" or table rows like "Total Parameters | 25.2B"
     - `hf_active_params`: Extract from lines like "3B activated" or "Active Parameters | 3.8B"
     - `hf_architecture_type`: Infer — if active_params is present → "MoE", else check for "Mamba" in text → "Mamba2-Transformer MoE", else → "Dense"
     - `hf_context_length`: Extract from lines like "Context Length: 262,144" or "128K tokens" (convert K to actual number, e.g., 128K → 131072)
     - `hf_num_layers`: Extract from lines like "Number of Layers: 40" or table rows like "Layers | 30"

   The parsing should be resilient — use regex or simple string matching. If a value can't be parsed, leave the field as `None`. Do NOT fail the entire fetch if README parsing is incomplete. The API call and README fetch are independent failures — if the API call succeeds but README fetch fails, still return the API-level metadata (format, base_model, pipeline_tag, last_modified).

3. **Update `refresh_metadata()`** in `update.rs`:
   - After the existing file upsert loop, call `fetch_hf_metadata(repo_id).await`
   - If successful, update the model config's HF metadata columns via a new helper function (see below)

4. **New helper `update_model_config_hf_metadata(conn: &Connection, model_id: i64, meta: &HfModelMetadata) -> Result<()>`** in `update.rs`:
   - Execute `UPDATE model_configs SET hf_format=?, hf_base_model=?, ... WHERE id=?`
   - Only update non-None fields (build dynamic SQL or use COALESCE pattern)

**Steps:**
- [ ] Add `HfModelMetadata` struct to `pull.rs` (with `#[derive(Debug, Clone, Default)]` only — no Serialize/Deserialize)
- [ ] Implement the pure function `parse_readme_metadata(markdown: &str) -> HfModelMetadata`
- [ ] Write unit tests for `parse_readme_metadata` with sample README markdown from Qwen3.6-35B-A3B, Gemma 4 26B A4B, and Nemotron 3 Nano (test MoE, Dense, and Mamba2 detection)
- [ ] Run `cargo test --package tama-core pull::tests`
  - Did the README parsing tests pass? If not, fix regex/string matching and re-run.
- [ ] Implement `fetch_hf_metadata()` — calls `hf_api().await?` for API info, then `api.client().get()` for README, then calls `parse_readme_metadata()`
- [ ] Add `update_model_config_hf_metadata()` helper in `update.rs`
- [ ] Update `refresh_metadata()` to call `fetch_hf_metadata()` and persist results
- [ ] Run `cargo build --package tama-core`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add HF metadata fetcher and integrate into refresh flow"

**Acceptance criteria:**
- [ ] `parse_readme_metadata` unit tests pass for MoE (Qwen3.6-35B-A3B), Dense (Gemma 4 31B), and Mamba2 (Nemotron) sample markdown (CI-verifiable)
- [ ] Manual test: `fetch_hf_metadata()` returns populated data for 3+ known models (not CI-verifiable)
- [ ] Missing/unparseable values default to `None` without failing the fetch
- [ ] `refresh_metadata()` persists HF metadata after fetching file blobs
- [ ] All existing tests still pass

---

### Task 3: Migration Backfill

**Context:**
After migration v19 runs, existing models have NULL for all 9 new columns. A one-time backfill task fetches HF metadata for all existing models and populates the columns. This runs on first startup after the migration, then is a no-op on subsequent startups.

**Files:**
- Modify: `crates/tama-core/src/db/backfill.rs` (or create a new module if backfill.rs doesn't exist)
- Modify: `crates/tama-core/src/proxy/lifecycle.rs` (or wherever the async startup sequence lives — find where migrations are run and DB is opened)

**What to implement:**

1. **New async function `backfill_hf_metadata(conn: &Connection) -> Result<()>`** in `backfill.rs`:
   - Query all model_configs rows where `hf_format IS NULL`
   - For each row, call `fetch_hf_metadata(&repo_id).await`
   - On success, call `update_model_config_hf_metadata(conn, id, &meta)`
   - Log warnings (not errors) for any model whose metadata fetch fails — the backfill should continue for remaining models even if some fail
   - Use a small delay between API calls (e.g., 200ms via `tokio::time::sleep`) to avoid rate limiting

2. **Trigger in async startup path:**
   - `migrations::run()` is synchronous and has no tokio runtime — the backfill CANNOT be triggered from there
   - Find the async startup code that calls `migrations::run()` (likely in `proxy/lifecycle.rs` or `handlers/run.rs`)
   - After migrations complete, check if any rows have `hf_format IS NULL`
   - If so, spawn a background task (`tokio::spawn`) that runs `backfill_hf_metadata`
   - The backfill must NOT block startup — it runs asynchronously
   - Note: The existing "Check all for updates" button already calls `/tama/v1/models/{id}/refresh` per model, which triggers `refresh_metadata()` (updated in Task 2) and thus updates HF metadata. This serves as a manual re-backfill path.

**Steps:**
- [ ] Implement `backfill_hf_metadata()` in `backfill.rs`
- [ ] Find the async startup code that runs migrations (search for `migrations::run` in `proxy/lifecycle.rs` or `handlers/run.rs`)
- [ ] Add the backfill trigger after migrations complete — spawn async task if NULL rows exist
- [ ] Write a test that verifies backfill populates NULL columns (use in-memory DB, skip actual HF API calls)
- [ ] Run `cargo build --package tama-core`
- [ ] Run `cargo test --package tama-core`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: add HF metadata backfill for existing models after migration v19"

**Acceptance criteria:**
- [ ] Backfill runs automatically after migration v19 on first startup
- [ ] Backfill is async and does not block proxy startup
- [ ] Failed fetches for individual models don't stop the backfill
- [ ] Subsequent startups skip the backfill (no NULL rows remain)

---

### Task 4: ModelStatus SSE Field + Dashboard Sorting

**Context:**
The `ModelStatus` struct in `gpu/system.rs` is serialized into the SSE metrics stream and consumed by the dashboard. We need to add `hf_architecture_type` so the model card can display it. We also need to change dashboard sorting from config key (`id`) to display name so models group by family.

**Files:**
- Modify: `crates/tama-core/src/gpu/system.rs`
- Modify: `crates/tama-core/src/proxy/status.rs`
- Modify: `crates/tama-web/src/pages/dashboard.rs`
- Modify: `crates/tama-web/src/components/model_card.rs`

**What to implement:**

1. **`ModelStatus`** in `gpu/system.rs`:
   Add one new field:
   ```rust
   /// Architecture type from HF metadata (e.g. "MoE", "Dense"). Display-only on dashboard.
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub hf_architecture_type: Option<String>,
   ```

2. **`collect_model_statuses()`** in `proxy/status.rs`:
   - When building each `ModelStatus`, include `hf_architecture_type: model_cfg.hf_architecture_type.clone()`
   - **Keep the sort by `a.id.cmp(&b.id)`** — the backend should maintain stable key-based ordering for SSE. Sorting by display name happens only in the frontend (see below).

3. **Dashboard frontend** in `dashboard.rs`:
   - Update the local `ModelStatus` struct (the frontend mirror) to include:
     ```rust
     #[serde(default)]
     hf_architecture_type: Option<String>,
     ```
     The `#[serde(default)]` is critical for backward compatibility — if the backend doesn't yet include this field in SSE payloads, deserialization won't fail.
   - In the Active Models and Inactive Models rendering sections, change sorting from `a.id.cmp(&b.id)` to `model_display_name(a).cmp(&model_display_name(b))`
   - Pass `hf_architecture_type` as a prop to `ModelCard`

**Note:** Task 4 depends on Task 1 being complete — `ModelConfig.hf_architecture_type` must exist before `collect_model_statuses()` can reference it. Do NOT attempt Task 4 before Task 1 is committed.

4. **`ModelCard` component** in `model_card.rs`:
   - Add new optional prop: `#[prop(default = None)] hf_architecture_type: Option<String>`
   - In line 2 (badge pills), after the backend badge, render the architecture badge:
     ```rust
     {if let Some(arch) = &hf_architecture_type {
         view! {
             <span class="badge-pill badge-pill--architecture">{arch}</span>
         }.into_any()
     } else {
         view! { <span/> }.into_any()
     }}
     ```

**Steps:**
- [ ] Add `hf_architecture_type` field to backend `ModelStatus` in `gpu/system.rs` (with `#[serde(default, skip_serializing_if = "Option::is_none")]`)
- [ ] Update `collect_model_statuses()` to populate the field from `model_cfg.hf_architecture_type.clone()` (keep sort by `id`)
- [ ] Update frontend `ModelStatus` struct in `dashboard.rs` to include `#[serde(default)] hf_architecture_type: Option<String>`
- [ ] Change dashboard sorting for both active and inactive models to use `model_display_name(a).cmp(&model_display_name(b))`
- [ ] Add `hf_architecture_type` prop to `ModelCard` component
- [ ] Render architecture badge pill in line 2 of the card
- [ ] Update all `ModelCard` call sites in `dashboard.rs` to pass the new prop
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: display HF architecture type on model cards and sort by display name"

**Acceptance criteria:**
- [ ] `ModelStatus` includes `hf_architecture_type` in SSE stream
- [ ] Dashboard sorts models by display name (Qwen models group together, Gemma models group together)
- [ ] ModelCard renders architecture badge pill when `hf_architecture_type` is present
- [ ] ModelCard renders nothing when `hf_architecture_type` is None
- [ ] All existing tests pass

---

### Task 5: CSS Styling + Verification

**Context:**
Add the CSS for the new `badge-pill--architecture` class and verify the complete feature end-to-end.

**Files:**
- Modify: `crates/tama-web/style.css` (the root-level CSS file in the web crate)

**What to implement:**

1. **CSS** — add `.badge-pill--architecture` class matching the existing badge pill pattern:
   ```css
   .badge-pill--architecture {
       /* Match existing badge-pill styling with a distinct color */
       background: var(--accent-purple, #8b5cf6);
       color: white;
   }
   ```

2. **End-to-end verification:**
   - Build the full workspace in release mode
   - Run clippy across all crates
   - Verify the migration applies cleanly on a fresh DB
   - Verify the backfill populates metadata for existing models

**Steps:**
- [ ] Add `.badge-pill--architecture` CSS class
- [ ] Run `cargo build --release --workspace`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo test --workspace`
  - Did all tests pass? If not, fix and re-run.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "style: add architecture badge pill CSS"

**Acceptance criteria:**
- [ ] `cargo build --release --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Architecture badge renders with distinct styling on the dashboard

---

## Rollout Order

1. Task 1 (DB + types) — foundation, must be first
2. Task 2 (HF fetcher) — depends on Task 1 types (HfModelMetadata uses the same field names)
3. Task 3 (Backfill) — depends on Task 2 fetcher
4. Task 4 (SSE + dashboard) — **depends on Task 1** (needs `ModelConfig.hf_architecture_type`), can run in parallel with Tasks 2-3 since it doesn't need the fetcher
5. Task 5 (CSS + verification) — final polish, depends on Task 4

Tasks 1 and 4 can share a branch since they both modify core types. Tasks 2 and 3 are sequential. Task 5 is the final integration.
