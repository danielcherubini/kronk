# Backup & Restore Plan

**Goal:** Add backup and restore functionality that archives config + database (no model files), and restores by re-downloading models and reinstalling backends from remote sources.

**Architecture:** A new `backup` module in `koji-core` handles archive creation/extraction, manifest validation, and config/DB merging. The CLI exposes `koji backup` and `koji restore` commands. The web UI adds a "Backup & Restore" section tab in the Config page with download/upload/preview/progress UI.

**Tech Stack:** `tar` + `flate2` + `sha2` crates (already in koji-core deps) for archive and integrity; `inquire` (already in koji-cli deps) for CLI interactive model selection; Leptos + Axum SSE for web UI (matching existing patterns from backend install and self-update flows).

---

### Task 1: Core backup module — manifest types, archive creation/extraction

**Context:**
This is the foundation. All other tasks depend on having working archive create/extract functions and a well-defined manifest format. The backup archive is a `.tar.gz` containing `manifest.json`, `config.toml`, `configs/*.toml`, and `koji.db`. The manifest provides metadata for integrity checking and preview (so restore can show what's in the backup without extracting everything).

**SHA-256 contract:** The SHA-256 in the manifest covers all archive entries *except* `manifest.json` itself. This avoids the chicken-and-egg problem. On creation: stream config files + DB into both a hasher and the tar.gz, compute SHA-256, then write a second tar.gz with `manifest.json` first (containing the hash) followed by all other entries. On extraction: read all entries except `manifest.json` into a hasher, compare against the manifest's `sha256` field.

**Files:**
- Create: `crates/koji-core/src/backup/mod.rs`
- Create: `crates/koji-core/src/backup/manifest.rs`
- Create: `crates/koji-core/src/backup/archive.rs`
- Modify: `crates/koji-core/src/lib.rs` (add `pub mod backup;`)
- Modify: `crates/koji-core/src/db/mod.rs` (add `backup_db` function)

**What to implement:**

1. **Manifest types** (`manifest.rs`):
   - `BackupManifest` struct with fields: `version: u32`, `created_at: String` (ISO 8601), `koji_version: String`, `sha256: String`, `models: Vec<BackupModelEntry>`, `backends: Vec<BackendEntry>`
   - `BackupModelEntry` with: `repo_id: String`, `quants: Vec<String>`, `total_size_bytes: i64`
   - `BackendEntry` with: `name: String`, `version: String`, `backend_type: String`, `source: String`
   - All derive `Debug, Clone, Serialize, Deserialize`
   - `BACKUP_FORMAT_VERSION: u32 = 1`

2. **Archive creation** (`archive.rs`):
   - `create_backup(config_dir: &Path, output_path: &Path) -> Result<BackupManifest>` — builds the tar.gz archive
   - **Two-pass streaming approach** (avoids holding entire archive in memory):
     - **Pass 1 (hash only):** Stream `config.toml`, `configs/*.toml`, and `koji.db` (via `VACUUM INTO`) through a `sha2::Sha256` hasher. Do NOT write to disk yet. Compute the SHA-256 hash.
     - **Pass 2 (write archive):** Create the output `.tar.gz`. Write `manifest.json` as the first entry (containing the SHA-256 from pass 1). Then write `config.toml`, `configs/*.toml`, and `koji.db` as subsequent entries.
   - Build `BackupManifest` by querying the DB for model/backend lists (open the VACUUM'd copy or the original DB).
   - Doc comment on `create_backup` must state the SHA-256 contract explicitly.

3. **Archive extraction** (`archive.rs`):
   - `extract_manifest(archive_path: &Path) -> Result<BackupManifest>` — reads just `manifest.json` from the archive without full extraction (for preview). Use `tar::Archive` and iterate entries, find `manifest.json`, parse it.
   - `extract_backup(archive_path: &Path, target_dir: &Path) -> Result<ExtractResult>` — validates SHA-256, extracts all files to `target_dir`.
     - Iterate archive entries. For `manifest.json`: parse it to get the expected SHA-256, but do NOT feed its bytes into the hasher. For all other entries: feed bytes into a `sha2::Sha256` hasher AND write to `target_dir`. After all entries are processed, compare computed SHA-256 against `manifest.sha256`. If mismatch, delete extracted files and return an error.
   - `ExtractResult` struct with: `manifest: BackupManifest`, `config_path: PathBuf`, `db_path: PathBuf`, `card_paths: Vec<PathBuf>`

4. **DB backup helper** (`db/mod.rs`):
   - `backup_db(config_dir: &Path, dest: &Path) -> Result<()>` — uses SQLite `VACUUM INTO ?` to create a clean copy of the database at `dest`. This avoids copying WAL/SHM files and guarantees a consistent snapshot.
   - Implementation: open connection to `config_dir/koji.db`, execute `VACUUM INTO ?` with `dest` as parameter.

5. **Module re-export** (`mod.rs`):
   - Re-export `create_backup`, `extract_backup`, `extract_manifest`, `BackupManifest`, `ExtractResult`

**Steps:**
- [ ] Verify `tar`, `flate2`, `sha2` already exist in `koji-core/Cargo.toml` `[dependencies]` (they do)
- [ ] Write failing tests in `crates/koji-core/src/backup/archive.rs` for `create_backup` and `extract_backup` roundtrip
- [ ] Write failing test verifying SHA-256 contract: create backup, manually tamper with an entry, verify `extract_backup` fails integrity check
- [ ] Run `cargo test --package koji-core -- backup::archive` — verify tests fail
- [ ] Implement `BackupManifest` and `BackendEntry`/`BackupModelEntry` in `manifest.rs`
- [ ] Implement `backup_db()` in `db/mod.rs` using `VACUUM INTO`
- [ ] Implement `create_backup()` in `archive.rs` using two-pass streaming approach
- [ ] Implement `extract_manifest()` and `extract_backup()` in `archive.rs`
- [ ] Add `pub mod backup;` to `koji-core/src/lib.rs`
- [ ] Run `cargo test --package koji-core -- backup` — verify all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add backup module with archive creation, extraction, and manifest"

**Acceptance criteria:**
- [ ] `create_backup` produces a valid `.tar.gz` containing `manifest.json`, `config.toml`, `configs/*.toml`, and `koji.db`
- [ ] `manifest.json` includes SHA-256 of all archive entries except itself, model list, backend list, and metadata
- [ ] `extract_backup` validates SHA-256 (hashes all entries except manifest.json) and extracts files to a target directory
- [ ] `extract_backup` rejects tampered archives with a clear error message
- [ ] `extract_manifest` reads just the manifest without full extraction
- [ ] `backup_db` uses `VACUUM INTO` for consistent DB copy
- [ ] All tests pass

---

### Task 2: Config and DB merge logic

**Context:**
Restore uses "smart merge" — existing local data is preserved, new data from the backup is added. This task implements the merge functions that combine the backup's config and DB records with the local installation. The merge must be idempotent (safe to re-run).

**Files:**
- Create: `crates/koji-core/src/backup/merge.rs`
- Modify: `crates/koji-core/src/backup/mod.rs` (add `pub mod merge;`, re-exports)

**What to implement:**

1. **Config merge** (`merge_config`):
   - `merge_config(local: &mut Config, backup: &Config) -> MergeStats`
   - For `backends`: Insert any backend key from backup that doesn't exist locally. Existing local backends are kept unchanged (local wins).
   - For `models`: Insert any model key from backup that doesn't exist locally. Existing local models are kept unchanged.
   - For `general`, `supervisor`, `proxy`, `sampling_templates`: Keep local values (they're machine-specific). Only add sampling templates that don't exist locally.
   - `MergeStats` struct: `new_backends: Vec<String>`, `new_models: Vec<String>`, `new_sampling_templates: Vec<String>`, `skipped_backends: Vec<String>`, `skipped_models: Vec<String>`
   - After merge, save the config using `local.save()` or `local.save_to(config_dir)`

2. **Model card merge** (`merge_model_cards`):
   - `merge_model_cards(local_configs_dir: &Path, backup_configs_dir: &Path) -> Result<Vec<String>>`
   - Copy any `.toml` file from `backup_configs_dir` that doesn't exist in `local_configs_dir`
   - Returns list of newly copied card names

3. **DB merge** (`merge_database`):
   - `merge_database(local_db: &Connection, backup_db_path: &Path) -> Result<DbMergeStats>`
   - Attach the backup DB: `ATTACH DATABASE ? AS backup_db`
   - **Explicit column lists** (no `SELECT *`) to avoid autoincrement `id` conflicts and schema mismatch:
     - `INSERT OR IGNORE INTO model_pulls (repo_id, commit_sha, pulled_at) SELECT repo_id, commit_sha, pulled_at FROM backup_db.model_pulls`
     - `INSERT OR IGNORE INTO model_files (repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at, last_verified_at, verified_ok, verify_error) SELECT repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at, last_verified_at, verified_ok, verify_error FROM backup_db.model_files` — use `COALESCE(last_verified_at, '')` etc. for v4-era backups missing v5 columns. Actually, since the backup DB was created by `VACUUM INTO`, it will have the same schema as the local DB. But for safety, wrap the ATTACH + INSERT in a transaction and check column count first: `PRAGMA backup_db.table_info(model_files)` — if v5 columns are missing, use a fallback INSERT without those columns.
   - `INSERT OR IGNORE INTO backend_installations (name, backend_type, version, path, installed_at, gpu_type, source, is_active) SELECT name, backend_type, version, path, installed_at, gpu_type, source, is_active FROM backup_db.backend_installations`
   - Skip `active_models`, `download_log`, `system_metrics_history` — ephemeral, not restored
   - Detach backup DB: `DETACH DATABASE backup_db`
   - `DbMergeStats` struct: `new_model_pulls: u32`, `new_model_files: u32`, `new_backend_installations: u32`
   - Count new rows by comparing counts before and after each INSERT.

4. **Model card TOML files** from `configs/` dir:
   - The extracted backup has a `configs/` directory with model card TOMLs
   - Copy any that don't exist locally to the local `configs/` dir

**Steps:**
- [ ] Write failing tests in `merge.rs` for `merge_config`, `merge_model_cards`, `merge_database`
- [ ] Write test for `merge_database` using two in-memory DBs with explicit column INSERT (not SELECT *)
- [ ] Run `cargo test --package koji-core -- backup::merge` — verify tests fail
- [ ] Implement `merge_config` with `MergeStats`
- [ ] Implement `merge_model_cards`
- [ ] Implement `merge_database` with explicit column lists, `INSERT OR IGNORE`, and `DbMergeStats`
- [ ] Add re-exports to `mod.rs`
- [ ] Run `cargo test --package koji-core -- backup::merge` — verify all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add backup merge logic for config, model cards, and DB"

**Acceptance criteria:**
- [ ] `merge_config` adds new backends/models/templates, keeps existing local values
- [ ] `merge_model_cards` copies new cards, skips existing ones
- [ ] `merge_database` inserts missing rows using explicit column lists, skips existing via `INSERT OR IGNORE`
- [ ] DB merge does NOT use `SELECT *` — all column names are explicit
- [ ] Merge is idempotent — running twice produces same result
- [ ] All tests pass

---

### Task 3: CLI `koji backup` command

**Context:**
The backup command creates the archive. This is the simplest part — it calls `create_backup` from the core module and prints the result. We also add `--dry-run` and `-o` output path flags. The `config_dir` is obtained from `config.loaded_from.as_ref()` (same approach as `Config::save()`).

**Files:**
- Create: `crates/koji-cli/src/commands/backup.rs`
- Modify: `crates/koji-cli/src/cli.rs` (add `Backup` and `Restore` variants to `Commands`)
- Modify: `crates/koji-cli/src/commands/mod.rs` (add `pub mod backup;`)
- Modify: `crates/koji-cli/src/lib.rs` (wire up `Commands::Backup` and `Commands::Restore` dispatch)

**What to implement:**

1. **`Commands::Backup` variant** in `cli.rs`:
   ```
   Backup {
       /// Output path for the backup archive (default: koji-backup-YYYY-MM-DD.tar.gz in current dir)
       #[arg(short, long)]
       output: Option<PathBuf>,
       /// Show what would be backed up without creating the archive
       #[arg(long)]
       dry_run: bool,
   }
   ```

2. **`cmd_backup` function** in `commands/backup.rs`:
   - Get `config_dir` from `config.loaded_from.as_ref()` — this is the directory containing `config.toml` and `koji.db`
   - Resolve output path: use `-o` value, or `koji-backup-YYYY-MM-DD.tar.gz` in current directory
   - If `--dry-run`: list files that would be archived (config.toml, model cards, koji.db) and their sizes, then exit
   - Call `koji_core::backup::create_backup(config_dir, output_path)`
   - Print success: archive path, size, number of models and backends included
   - Error handling: config_dir missing, DB open failure, disk write failure

3. **Wire up in `lib.rs`**: Add `Commands::Backup { output, dry_run } => commands::backup::cmd_backup(&config, output, dry_run)`

**Steps:**
- [ ] Add `Backup` variant to `Commands` enum in `cli.rs`
- [ ] Create `commands/backup.rs` with `cmd_backup` stub
- [ ] Add `pub mod backup;` to `commands/mod.rs`
- [ ] Add dispatch in `lib.rs` for `Commands::Backup`
- [ ] Run `cargo build --package koji-cli` — verify it compiles
- [ ] Implement `cmd_backup` with `--dry-run` and `-o` support
- [ ] Write integration test in `tests/tests.rs`: create temp config dir with `Config::load_from`, run `cmd_backup`, verify archive exists and is valid
- [ ] Run `cargo test --package koji` — verify tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add koji backup CLI command"

**Acceptance criteria:**
- [ ] `koji backup` creates a `.tar.gz` in the current directory
- [ ] `koji backup -o /path/to/file.tar.gz` creates archive at specified path
- [ ] `koji backup --dry-run` prints files and sizes without creating archive
- [ ] Output shows archive path, size, model count, backend count
- [ ] Integration test verifies archive creation roundtrip

---

### Task 4: CLI `koji restore` command

**Context:**
The restore command validates the archive, merges config/DB, reinstalls backends, and re-downloads model GGUFs. It supports `--select` for interactive TUI model selection (using `inquire::MultiSelect`, consistent with existing CLI), `--dry-run`, `--skip-backends`, and `--skip-models`. The `--force` flag is deferred to a future iteration — merge-only behavior is sufficient for v1.

**Files:**
- Modify: `crates/koji-cli/src/commands/backup.rs` (add `cmd_restore`)
- Modify: `crates/koji-cli/src/cli.rs` (add `Restore` variant)
- Modify: `crates/koji-cli/src/lib.rs` (wire up `Commands::Restore`)

**What to implement:**

1. **`Commands::Restore` variant** in `cli.rs`:
   ```
   Restore {
       /// Path to backup archive
       archive: PathBuf,
       /// Interactively select which models to restore
       #[arg(long)]
       select: bool,
       /// Show what would be restored without making changes
       #[arg(long)]
       dry_run: bool,
       /// Skip backend re-installation
       #[arg(long)]
       skip_backends: bool,
       /// Skip model re-downloading
       #[arg(long)]
       skip_models: bool,
   }
   ```
   Note: `--force` is NOT included in v1. The merge logic is sufficient. Force/overwrite can be added later.

2. **`cmd_restore` function** in `commands/backup.rs`:
   
   Get `config_dir` from `config.loaded_from.as_ref()`.

   Phase 1 — Validate:
   - Call `extract_manifest(archive_path)` to read manifest
   - Print backup metadata (date, koji version, model count, backend count)
   - If `--dry-run`: print what would be restored, then exit
   
   Phase 2 — Extract & merge config:
   - Call `extract_backup(archive_path, temp_dir)` to extract to a temp directory
   - Load the backup config from the extracted `config.toml`
   - Call `merge_config(&mut local_config, &backup_config)` 
   - Call `merge_model_cards(local_configs_dir, backup_configs_dir)`
   - Print merge results (new backends, new models, etc.)
   
   Phase 3 — Merge DB:
   - Open local DB
   - Call `merge_database(&local_conn, &extracted_db_path)`
   - Print merge results
   
   Phase 4 — Install backends (if not `--skip-backends`):
   - Read `backend_installations` from the merged DB
   - For each active backend that isn't already installed locally (check if binary path exists):
     - Re-download and install using `install_backend_with_progress()`
     - Print progress: "Installing backend {name} {version}..."
   - Skip backends that already have a working binary at the recorded path
   
   Phase 5 — Pull models (if not `--skip-models`):
   - Read `model_pulls` and `model_files` from the merged DB
   - Group files by `repo_id`
   - If `--select`: use `inquire::MultiSelect` to let user pick which repos to restore
     - Items show: "repo_id (N quants, ~X GB)"
     - If not a TTY, fall back to restoring all models with a warning
   - For each selected repo:
     - Check if GGUF files already exist locally (using `models_dir/repo_id/filename`)
     - Skip files that exist with matching size (from `model_files.size_bytes`)
     - Download missing files using `download_gguf()` from `koji_core::models::pull`
     - Print progress: "Pulling {repo_id} {filename} ({size})..."
   
   Cleanup:
   - Remove temp extraction directory
   - Print final summary: models restored, backends installed, total time

3. **Wire up in `lib.rs`**: Add `Commands::Restore { archive, select, dry_run, skip_backends, skip_models } => commands::backup::cmd_restore(&config, archive, select, dry_run, skip_backends, skip_models).await`

**Steps:**
- [ ] Add `Restore` variant to `Commands` enum in `cli.rs`
- [ ] Add `cmd_restore` stub in `commands/backup.rs`
- [ ] Add dispatch in `lib.rs` for `Commands::Restore`
- [ ] Run `cargo build --package koji-cli` — verify it compiles
- [ ] Implement Phase 1 (validate) and Phase 2 (extract & merge config)
- [ ] Implement Phase 3 (merge DB)
- [ ] Implement Phase 4 (install backends)
- [ ] Implement Phase 5 (pull models with `--select` support using `inquire::MultiSelect`)
- [ ] Implement `--dry-run`, `--skip-backends`, `--skip-models`
- [ ] Write integration test: create backup with `cmd_backup`, restore with `cmd_restore --dry-run`, verify dry-run output
- [ ] Run `cargo test --package koji` — verify tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add koji restore CLI command with selective model restore"

**Acceptance criteria:**
- [ ] `koji restore <file>` validates, merges config/DB, installs backends, pulls models
- [ ] `koji restore <file> --dry-run` shows what would happen without making changes
- [ ] `koji restore <file> --select` shows interactive TUI for model selection using `inquire::MultiSelect` (falls back to all on non-TTY)
- [ ] `koji restore <file> --skip-backends` skips backend installation
- [ ] `koji restore <file> --skip-models` skips model downloads
- [ ] Existing models/backends are not re-downloaded
- [ ] Restore is idempotent — running twice produces same result
- [ ] Progress is printed for each phase
- [ ] Integration test covers dry-run roundtrip

---

### Task 5: Web API endpoints for backup and restore

**Context:**
The web UI needs API endpoints to create backups (download), preview restore archives (upload + read manifest), and execute restore with progress streaming. The restore endpoint follows the same async job + SSE pattern used by backend installs. The `Job.backend_type` field is made optional to support restore jobs that don't have a backend type.

**Files:**
- Create: `crates/koji-web/src/api/backup.rs`
- Modify: `crates/koji-web/src/api.rs` (add `pub mod backup;` — note: `api.rs` is the module root, NOT `api/mod.rs`)
- Modify: `crates/koji-web/src/server.rs` (add routes — restore POST routes go in `backend_routes` sub-router for CSRF protection)
- Modify: `crates/koji-web/src/jobs.rs` (add `JobKind::Restore`, make `Job.backend_type` optional)

**What to implement:**

1. **`JobKind::Restore`** in `jobs.rs`:
   - Add `Restore` variant to `JobKind` enum
   - Make `Job.backend_type` an `Option<koji_core::backends::BackendType>` (None for restore jobs)
   - Update `JobManager::submit()` to accept `backend_type: Option<BackendType>`
   - Update all `submit()` call sites in `api/backends.rs` to pass `Some(backend_type)` instead of bare `backend_type`
   - Update `job_events_sse` handler and `get_job` endpoint to handle `backend_type = None` gracefully (omit from JSON or set to null)

2. **`POST /api/backup`** — `create_backup`:
   - Calls `koji_core::backup::create_backup(config_dir, temp_path)` using `spawn_blocking`
   - Returns the `.tar.gz` as a file download with `Content-Type: application/gzip` and `Content-Disposition: attachment; filename="koji-backup-YYYY-MM-DD.tar.gz"`
   - This is a read-only endpoint, so it can stay on the main router (no CSRF concern)

3. **`POST /api/restore/preview`** — `preview_restore`:
   - Accepts `multipart/form-data` with the archive file (max 100 MB)
   - Saves upload to a temp file in `std::env::temp_dir()` with a UUID name
   - Calls `extract_manifest(temp_path)` using `spawn_blocking` to read manifest without full extraction
   - Stores the temp file path in `AppState.restore_temp_uploads` keyed by a UUID string
   - Returns JSON: `{ "upload_id": "uuid", "created_at": "...", "koji_version": "...", "models": [...], "backends": [...] }`
   - Does NOT make any changes to local state
   - This stores state, so it MUST be behind CSRF protection → goes in `backend_routes`

4. **`POST /api/restore`** — `start_restore`:
   - Accepts JSON body: `{ "upload_id": "uuid", "selected_models": Option<Vec<String>>, "skip_backends": bool, "skip_models": bool }`
   - Looks up temp file path from `AppState.restore_temp_uploads` using `upload_id`
   - Returns 404 if upload_id not found (expired or invalid)
   - Creates a restore job using `JobManager::submit(JobKind::Restore, None)`
   - Spawns a background task that runs restore phases (extract, merge config, merge DB, install backends, pull models) — calls the same core functions as CLI
   - Broadcasts progress via `job.log_tx` (same as backend install)
   - Returns job ID immediately
   - After restore completes or fails, cleans up the temp upload entry
   - MUST be behind CSRF protection → goes in `backend_routes`

5. **SSE progress**: Reuse the existing `GET /api/backends/jobs/:id/events` endpoint. The frontend subscribes using the job ID returned by `POST /api/restore`. No new SSE endpoint needed.

6. **Temp upload cleanup**:
   - `AppState` gets `restore_temp_uploads: Arc<Mutex<HashMap<String, TempUploadEntry>>>` where `TempUploadEntry { path: PathBuf, created_at: Instant }`
   - On server startup, no cleanup needed (orphaned temp files are in OS temp dir)
   - Before each preview/restore, evict entries older than 10 minutes and delete their temp files
   - After successful restore, remove the entry and delete the temp file

7. **Routes in `server.rs`**:
   - Add `POST /api/backup` to the main `Router` (read-only, no CSRF concern)
   - Add `POST /api/restore/preview` and `POST /api/restore` to the `backend_routes` sub-router (has `enforce_same_origin` middleware for CSRF protection)

**Steps:**
- [ ] Modify `JobKind` in `jobs.rs` to add `Restore` variant
- [ ] Change `Job.backend_type` to `Option<BackendType>` in `jobs.rs`
- [ ] Update `JobManager::submit()` signature to accept `backend_type: Option<BackendType>`
- [ ] Update all `submit()` call sites in `api/backends.rs` to pass `Some(backend_type)`
- [ ] Update `job_events_sse` and `get_job` handlers to handle `backend_type = None`
- [ ] Add `restore_temp_uploads` field to `AppState`
- [ ] Create `crates/koji-web/src/api/backup.rs` with handler stubs
- [ ] Add `pub mod backup;` to `crates/koji-web/src/api.rs` (the module root file, NOT `api/mod.rs`)
- [ ] Implement `POST /api/backup` handler
- [ ] Implement `POST /api/restore/preview` handler with temp upload storage
- [ ] Implement `POST /api/restore` handler with async job + background task
- [ ] Add routes in `server.rs`: backup on main router, restore routes in `backend_routes`
- [ ] Write `#[tokio::test]` for backup handler: create backup, verify response has correct content type
- [ ] Write `#[tokio::test]` for restore preview: upload archive, verify manifest returned
- [ ] Run `cargo test --workspace` — verify all tests pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add backup and restore API endpoints with async job progress"

**Acceptance criteria:**
- [ ] `POST /api/backup` returns a downloadable `.tar.gz` file with correct headers
- [ ] `POST /api/restore/preview` uploads archive, returns manifest data without modifying local state
- [ ] `POST /api/restore` starts async restore job, returns job ID
- [ ] Restore progress is streamed via existing `/api/backends/jobs/:id/events` SSE endpoint
- [ ] Temp uploads are cleaned up after use or after 10-minute timeout
- [ ] Restore endpoints are behind CSRF protection (in `backend_routes` sub-router)
- [ ] `Job.backend_type` is `Option<BackendType>`, existing backend jobs pass `Some(...)`, restore jobs pass `None`
- [ ] All tests pass

---

### Task 6: Web UI — Backup & Restore section in Config page

**Context:**
The Config page has a side nav with tabs: General, Proxy, Supervisor, Sampling Templates. We add "Backup & Restore" as a 5th tab. The backup UI is a simple one-click download. The restore UI has a file upload, preview with checkboxes, and SSE progress streaming — following the same patterns as backend install. The `BackupSection` component is defined in its own file but imported and rendered from `config_editor.rs`.

**Files:**
- Create: `crates/koji-web/src/components/backup_section.rs`
- Modify: `crates/koji-web/src/components/mod.rs` (add `pub mod backup_section;`)
- Modify: `crates/koji-web/src/pages/config_editor.rs` (add `Section::Backup` variant, import and render `BackupSection` component)

**What to implement:**

1. **Section enum update** in `config_editor.rs`:
   - Add `Backup` variant: icon `"💾"`, name `"Backup & Restore"`
   - Add to the section list in the side nav
   - Add `<div id="cfg-backup">` that renders `<BackupSection />`
   - `BackupSection` is imported via `use crate::components::backup_section::BackupSection;`

2. **`BackupSection` component** in `backup_section.rs`:

   **Backup card:**
   - Header: "Backup"
   - Description: "Download a backup of your configuration, model cards, and database. Model files are not included — they'll be re-downloaded on restore."
   - "Create Backup" button:
     - Use a hidden `<form method="POST" action="/api/backup">` that triggers the download natively on submit. This is the simplest and most reliable approach for file downloads in a browser.
     - Alternative: use `gloo_net` to POST, then create a blob URL from the response — use this if form POST doesn't work well with the CSRF-protected endpoints. But since `/api/backup` is NOT behind CSRF protection (it's on the main router), a form POST works fine.

   **Restore card:**
   - Header: "Restore"
   - Description: "Upload a backup archive to restore configuration and re-download models."
   - File upload area:
     - Dropzone with drag-and-drop + click to browse, accept only `.tar.gz`
     - On file select: `POST /api/restore/preview` with multipart upload
     - Show loading spinner while uploading
   - Preview section (shown after upload succeeds):
     - Backup metadata: date, koji version
     - Backends list with versions
     - Models list with checkboxes (default: all checked), showing repo_id, quant count, total size
     - "Restore Selected" button
   - Progress section (shown during restore):
     - Same pattern as backend install: spinner + status text
     - Subscribe to SSE at `/api/backends/jobs/{job_id}/events` (reuse existing endpoint)
     - Shows phase progress: "Merging config...", "Installing backends [1/2]...", "Pulling models [1/5]..."
   - Success/failure message when done

3. **Styling**: Use existing card/button/form CSS classes from `style.css`. Dropzone can use a dashed border card pattern similar to the pull model modal.

**Steps:**
- [ ] Create `crates/koji-web/src/components/backup_section.rs` with component stub
- [ ] Add `pub mod backup_section;` to `components/mod.rs`
- [ ] Add `Section::Backup` variant to `config_editor.rs` with icon `"💾"` and name `"Backup & Restore"`
- [ ] Import `BackupSection` in config_editor.rs and add render in the main form area
- [ ] Implement backup card with "Create Backup" button using form POST for download
- [ ] Implement restore card with file upload dropzone
- [ ] Implement preview display with model checkboxes
- [ ] Implement restore execution: POST `/api/restore`, subscribe to job SSE at `/api/backends/jobs/{id}/events`
- [ ] Run `cargo build --package koji-web` — verify it compiles
- [ ] Test manually in browser: create backup download, upload archive, preview, restore
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add Backup & Restore section to web UI Config page"

**Acceptance criteria:**
- [ ] "Backup & Restore" tab appears in Config page side nav with 💾 icon
- [ ] Backup card has "Create Backup" button that downloads a `.tar.gz` file
- [ ] Restore card accepts `.tar.gz` file upload via dropzone
- [ ] After upload, preview shows metadata, backends, and model checkboxes
- [ ] "Restore Selected" starts restore, streams progress via SSE (reusing `/api/backends/jobs/:id/events`)
- [ ] Progress shows current phase and step count
- [ ] Success/failure message shown when restore completes or fails
