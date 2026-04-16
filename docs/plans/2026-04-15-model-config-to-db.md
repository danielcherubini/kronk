# Model Config to Database Plan

**Goal:** Move all model configuration out of `koji.toml` into SQLite so the filesystem and DB are the single source of truth and config files can never become stale.

**Architecture:** A new `model_configs` DB table holds every field currently in `koji.toml`'s `[models]` section. The in-memory `config.models: HashMap<String, ModelConfig>` is kept as the runtime registry (to avoid touching ~20 call sites) but is now populated from the DB on startup and written back to the DB on every mutation. `koji.toml` retains only non-model settings (general, backends, proxy, supervisor). On first startup after migration, any existing `[models]` entries in `koji.toml` are automatically imported to the DB and then stripped from the file.

**Tech Stack:** Rust, rusqlite, serde_json (for JSON columns), existing migration infrastructure in `crates/koji-core/src/db/migrations.rs`

---

### Task 1: DB migration — add `model_configs` table and `kind` column

**Context:**
All model user-preferences (enabled, selected quant, backend, args, context length, etc.) need a home in SQLite. This task adds the schema. It also adds a `kind` column to the existing `model_files` table so GGUF model files and mmproj vision-projector files can be told apart without inferring from the filename. No application logic changes in this task — schema only.

**Files:**
- Modify: `crates/koji-core/src/db/migrations.rs`
- Modify: `crates/koji-core/src/db/queries/types.rs`
- Modify: `crates/koji-core/src/db/queries/mod.rs`

**What to implement:**

Add migration v7 (increment `LATEST_VERSION` to `7`) with this SQL:

```sql
-- Per-repo user configuration (replaces [models] in koji.toml)
CREATE TABLE IF NOT EXISTS model_configs (
    repo_id       TEXT PRIMARY KEY,
    display_name  TEXT,
    backend       TEXT NOT NULL DEFAULT 'llama_cpp',
    enabled       INTEGER NOT NULL DEFAULT 1,
    selected_quant  TEXT,        -- quant key (e.g. "Q4_K_M"), references model_files.quant
    selected_mmproj TEXT,        -- mmproj filename (e.g. "mmproj-F16.gguf")
    context_length  INTEGER,
    gpu_layers      INTEGER,
    port            INTEGER,
    args            TEXT,        -- JSON array of strings, e.g. '["--flash-attn"]'
    sampling        TEXT,        -- JSON object (serialised SamplingParams), nullable
    modalities      TEXT,        -- JSON object {input:[],output:[]}, nullable
    profile         TEXT,
    api_name        TEXT,
    health_check    TEXT,        -- JSON object (serialised HealthCheck), nullable
    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Add file kind so model files and mmproj files are distinguishable
ALTER TABLE model_files ADD COLUMN kind TEXT NOT NULL DEFAULT 'model';
```

Add a `ModelConfigRecord` struct to `types.rs`:

```rust
pub struct ModelConfigRecord {
    pub repo_id: String,
    pub display_name: Option<String>,
    pub backend: String,
    pub enabled: bool,
    pub selected_quant: Option<String>,
    pub selected_mmproj: Option<String>,
    pub context_length: Option<u32>,
    pub gpu_layers: Option<u32>,
    pub port: Option<u16>,
    pub args: Option<String>,        // raw JSON string
    pub sampling: Option<String>,    // raw JSON string
    pub modalities: Option<String>,  // raw JSON string
    pub profile: Option<String>,
    pub api_name: Option<String>,
    pub health_check: Option<String>, // raw JSON string
    pub created_at: String,
    pub updated_at: String,
}
```

Add to `queries/mod.rs` (new file `queries/model_config_queries.rs` is fine too):

```rust
pub fn upsert_model_config(conn: &Connection, record: &ModelConfigRecord) -> Result<()>
pub fn get_model_config(conn: &Connection, repo_id: &str) -> Result<Option<ModelConfigRecord>>
pub fn get_all_model_configs(conn: &Connection) -> Result<Vec<ModelConfigRecord>>
pub fn delete_model_config(conn: &Connection, repo_id: &str) -> Result<()>
```

`upsert_model_config` must set `updated_at = strftime(...)` on conflict. `delete_model_config` should also call `delete_model_records` (which removes model_files and model_pulls rows) — both in a single transaction.

**Steps:**
- [ ] Add migration v7 SQL to `migrations.rs`, set `LATEST_VERSION = 7`
- [ ] Add `ModelConfigRecord` to `types.rs`
- [ ] Add the four query functions
- [ ] Write tests in `queries/tests.rs`:
  - `test_upsert_and_get_model_config` — upsert, retrieve, verify all fields round-trip
  - `test_get_all_model_configs` — insert two records, assert both returned
  - `test_delete_model_config` — upsert then delete, assert None on get
  - `test_migration_v7_creates_model_configs_table` in `migrations.rs`
- [ ] Run `cargo test --package koji-core -- db` — all must pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit: `feat(db): migration v7 — model_configs table and model_files.kind column`

**Acceptance criteria:**
- [ ] `LATEST_VERSION` is `7`
- [ ] `model_configs` table exists after `db::open()`
- [ ] All four CRUD functions work and are tested
- [ ] `model_files` table has a `kind` column defaulting to `'model'`
- [ ] No existing tests broken

---

### Task 2: Persist and load model config via DB

**Context:**
With the schema in place, we need two things: (a) a way to convert between `ModelConfigRecord` (flat DB row with JSON strings) and `ModelConfig` (the rich in-memory struct used everywhere), and (b) functions to load all model configs from DB into the `HashMap<String, ModelConfig>` that `ProxyState` / `Config` uses, and to save a single `ModelConfig` back. This task adds those conversion and persistence helpers — no call-site changes yet.

The config key (the HashMap key, e.g. `"unsloth--gemma-4-31b-it-gguf"`) is derived from `repo_id` by lowercasing and replacing `/` with `--`. Reverse is also needed (split on `--`, rejoin with `/`).

**Files:**
- Create: `crates/koji-core/src/db/queries/model_config_queries.rs` (or extend `model_queries.rs`)
- Modify: `crates/koji-core/src/db/mod.rs` (re-export new helpers)
- Modify: `crates/koji-core/src/config/types.rs` (add conversion methods)

**What to implement:**

In `config/types.rs` on `ModelConfig`:

```rust
impl ModelConfig {
    /// Serialise to a ModelConfigRecord for DB storage.
    /// `repo_id` is the HF repo id (e.g. "unsloth/gemma-4-31B-it-GGUF").
    pub fn to_db_record(&self, repo_id: &str) -> crate::db::queries::ModelConfigRecord

    /// Deserialise from a DB record. JSON fields are parsed; parse errors
    /// fall back to None / default so a bad JSON column never hard-fails.
    pub fn from_db_record(record: &crate::db::queries::ModelConfigRecord) -> Self
}
```

JSON serialisation: use `serde_json::to_string` / `from_str`. Fields that fail to deserialise should log a warning and use `None` / default.

Add to `db/mod.rs` (or a new `db/model_config.rs`):

```rust
/// Load all model_configs rows and return them as a HashMap<config_key, ModelConfig>
/// where config_key = repo_id.to_lowercase().replace('/', "--").
pub fn load_model_configs(conn: &Connection) -> Result<HashMap<String, ModelConfig>>

/// Persist a single ModelConfig entry.
/// `config_key` is the HashMap key; `repo_id` is derived by reversing the key convention.
pub fn save_model_config(conn: &Connection, config_key: &str, mc: &ModelConfig) -> Result<()>
```

**Steps:**
- [ ] Add `to_db_record` and `from_db_record` on `ModelConfig`
- [ ] Add `load_model_configs` and `save_model_config` in db layer
- [ ] Write unit tests:
  - `test_model_config_round_trip` — create a `ModelConfig` with all fields populated (args, sampling, modalities, health_check), call `to_db_record` then `from_db_record`, assert equality
  - `test_load_model_configs_empty` — empty DB returns empty HashMap
  - `test_save_and_load_model_config` — save one, load all, assert present
- [ ] Run `cargo test --package koji-core` — all pass
- [ ] Run `cargo fmt --all && cargo build --workspace`
- [ ] Commit: `feat(db): ModelConfig ↔ DB record conversion and load/save helpers`

**Acceptance criteria:**
- [ ] All fields of `ModelConfig` survive a round-trip through `to_db_record` → `from_db_record`
- [ ] `load_model_configs` returns a correctly-keyed HashMap
- [ ] No existing tests broken

---

### Task 3: On-startup migration — import koji.toml models to DB, strip from file

**Context:**
Existing installations have models in `koji.toml`. We must import them to DB on first startup (when `model_configs` is empty but `config.models` is not), then remove them from the TOML file so they are never read from TOML again. This is a one-time, automatic migration — no user action required. After this, `koji.toml` only contains general/backends/proxy/supervisor config.

**Files:**
- Create: `crates/koji-core/src/config/migrate/model_to_db.rs`
- Modify: `crates/koji-core/src/config/migrate/mod.rs`
- Modify: `crates/koji-core/src/proxy/server/mod.rs` (call migration on startup)
- Modify: `crates/koji-core/src/config/types.rs` (make `models` field `#[serde(skip)]` or default-empty after migration)

**What to implement:**

```rust
/// If the DB has no model_configs rows but Config has non-empty models,
/// import all Config models into DB, then clear Config.models and save
/// the config file (removing the [models] section from koji.toml).
/// Returns the number of models migrated (0 = nothing to do).
pub fn migrate_models_to_db(
    conn: &Connection,
    config: &mut Config,
) -> anyhow::Result<usize>
```

Steps inside the function:
1. `get_all_model_configs(conn)` — if non-empty, return 0 (already migrated)
2. If `config.models` is empty, return 0
3. For each `(key, mc)` in `config.models`, call `save_model_config(conn, key, mc)`
4. Log how many were migrated
5. `config.models.clear()`
6. `config.save()` — this writes koji.toml without the `[models]` section

Call `migrate_models_to_db` in `proxy/server/mod.rs` during startup, right after `db::open()` and before the proxy begins serving.

Also add `koji model migrate` CLI subcommand as an explicit trigger for users who want to run it manually or verify it. The subcommand should print each model it migrates.

**Steps:**
- [ ] Implement `migrate_models_to_db` in `config/migrate/model_to_db.rs`
- [ ] Write tests:
  - `test_migrate_models_to_db_imports_all` — start with populated config.models and empty DB, run migration, assert DB has rows and config.models is empty
  - `test_migrate_models_to_db_skips_if_db_has_rows` — DB already has a row, assert no-op (returns 0)
  - `test_migrate_models_to_db_skips_if_config_empty` — config.models empty, assert 0
- [ ] Wire call into proxy startup
- [ ] Add `koji model migrate` CLI subcommand
- [ ] Run `cargo test --package koji-core -- migrate` — all pass
- [ ] Run `cargo fmt --all && cargo build --workspace`
- [ ] Commit: `feat: auto-migrate koji.toml model entries to DB on first startup`

**Acceptance criteria:**
- [ ] After one startup with models in koji.toml: DB has rows, koji.toml `[models]` is gone
- [ ] Second startup: migration is a no-op
- [ ] `koji model migrate` prints what it migrated (or "nothing to migrate")
- [ ] All existing tests pass

---

### Task 4: Proxy loads and saves models via DB

**Context:**
Now that DB has the data and helpers exist, wire the proxy to use them. On startup, populate `config.models` from DB (after the migration in Task 3 has run). On every write that currently calls `config.save()` for model changes, replace with `save_model_config(conn, key, mc)`. The in-memory HashMap stays — we are not changing the 20+ read call-sites, only the load and write paths.

Write sites to update (grep confirms these call `config.save()` after mutating `config.models`):
- `crates/koji-core/src/proxy/koji_handlers/pull.rs` — `setup_model_after_pull`
- `crates/koji-core/src/proxy/koji_handlers/pull.rs` — `_setup_model_after_pull_with_config`
- `crates/koji-core/src/proxy/handlers.rs` — model enable/disable/update handlers
- `crates/koji-core/src/proxy/server/mod.rs` — startup and restart paths

**Files:**
- Modify: `crates/koji-core/src/proxy/server/mod.rs`
- Modify: `crates/koji-core/src/proxy/koji_handlers/pull.rs`
- Modify: `crates/koji-core/src/proxy/handlers.rs`
- Modify: `crates/koji-core/src/proxy/types.rs` (ensure `ProxyState` exposes db connection for handlers)

**What to implement:**

In `proxy/server/mod.rs` startup sequence (after migration):
```rust
// Populate in-memory model registry from DB
let db_models = db::load_model_configs(&conn)?;
if !db_models.is_empty() {
    config.models = db_models;
}
```

In every handler that currently does `config.save()` after touching `config.models`:
- Replace `config.save()?` with `db::save_model_config(&conn, &key, &mc)?`
- Do NOT call `config.save()` for model changes (it writes the whole TOML, and models are no longer in TOML)
- `config.save()` may still be called for non-model config changes (general settings, backends, etc.)

`ProxyState` already has `db_dir: Option<PathBuf>` — open the DB connection where needed, or add a `db_conn` to `ProxyState` if connection reuse is preferred (a `Mutex<Connection>` or open fresh per write — either is fine, prefer simplicity).

**Steps:**
- [ ] Add DB load of model_configs to proxy startup in `server/mod.rs`
- [ ] Update `_setup_model_after_pull_with_config` to write to DB not TOML
- [ ] Update model mutation handlers in `handlers.rs` to write to DB
- [ ] Run `cargo test --package koji-core` — all pass
- [ ] Manual smoke-test: start proxy, pull a model, restart — model should still be there (loaded from DB)
- [ ] Run `cargo fmt --all && cargo build --workspace`
- [ ] Commit: `feat: proxy loads and saves model config via DB instead of koji.toml`

**Acceptance criteria:**
- [ ] After `koji model scan` or pull, model appears in DB (`model_configs` table)
- [ ] After proxy restart, model is loaded from DB — not from koji.toml
- [ ] No model-related writes touch `koji.toml`
- [ ] All existing proxy tests pass

---

### Task 5: Update `model scan` and remove models from koji.toml schema

**Context:**
With the DB as source of truth, `model scan` can be dramatically simplified. It no longer needs to parse model cards or config files — just walk the filesystem and reconcile with the DB. This task rewrites scan to use the DB-first approach, removes the debug verbose output added temporarily, and removes `ModelConfig` / `models` from the `Config` struct (completing the migration).

**Files:**
- Modify: `crates/koji-cli/src/commands/model.rs` (`cmd_scan`)
- Modify: `crates/koji-core/src/config/types.rs` (remove/deprecate `models` field)
- Modify: `crates/koji-core/src/config/mod.rs` (remove model-related config helpers if any)
- Delete or empty: model card loading code no longer needed for scan

**What to implement:**

New `cmd_scan` logic:
1. Open DB
2. Walk `models_dir` recursively for `*.gguf` files — for each, check if a `model_files` row exists
   - File exists on disk, no DB row → insert into `model_files` (and `model_configs` if new repo)
   - File missing on disk, DB row exists → `delete_model_file` from DB; if repo has no surviving files → `delete_model_config`
3. Walk `model_configs` DB rows — if the model dir doesn't exist at all, delete the config row
4. Print a summary of what was added and removed
5. Remove the temporary verbose debug output (the `println!` blocks added in previous commits)

Remove `pub models: HashMap<String, ModelConfig>` from `Config` struct (or mark `#[serde(skip, default)]` if removing breaks too much at once — prefer full removal). Remove all `config.models` access from non-proxy code (CLI, scan, etc.) and replace with DB queries.

**Steps:**
- [ ] Rewrite `cmd_scan` to use DB instead of model cards / config.models
- [ ] Remove debug `println!` blocks from scan
- [ ] Remove `models` from `Config` struct (or `#[serde(skip, default)]` as interim)
- [ ] Fix all compile errors caused by removing `config.models` outside proxy
- [ ] Run `cargo test --workspace` — all pass
- [ ] Run `cargo fmt --all && cargo build --workspace`
- [ ] Commit: `feat: rewrite model scan to use DB as source of truth, remove models from Config`

**Acceptance criteria:**
- [ ] `koji model scan` with a completely empty models dir removes all ghost DB/config entries
- [ ] `koji model scan` with files on disk but no DB entries adds them
- [ ] No references to `config.models` outside `crates/koji-core/src/proxy/`
- [ ] `koji.toml` no longer has a `[models]` section after migration
- [ ] All tests pass

---

## Migration safety notes

- Tasks 1–3 are additive (new table, new helpers, import). Safe to ship independently.
- Task 4 is the behaviour change. The proxy must open DB and the migration (Task 3) must have run before Task 4 is deployed.
- Task 5 is cleanup. Can be done as a follow-up after Task 4 is stable.
- The `model_configs` table uses `repo_id` as primary key. The config HashMap key is `repo_id.to_lowercase().replace('/', "--")`. This convention must be consistent across all tasks.
