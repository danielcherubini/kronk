# SQLite Database + `koji model update` Plan

**Goal:** Introduce a SQLite database (`koji.db`) in the config directory to store internal metadata, and use it to power a new `koji model update` command that detects and downloads updated GGUF files from HuggingFace.
**Status:** ✅ COMPLETED - See git commits `e7e73e0` ("feat: SQLite database + kronk model update command (#23)"), `8d01ccb` ("feat: add SQLite database foundation with migration system")

**Architecture:** A new `db` module in `koji-core` owns the database connection, schema migrations, and typed query functions. The DB stores internal metadata (pull timestamps, commit SHAs, file hashes, download logs) while TOML model cards remain the user-facing config for sampling presets, context lengths, and GPU layers. The `koji model update` command compares stored commit SHAs and per-file LFS OIDs against HuggingFace to detect changes.

**Tech Stack:** `rusqlite` with `bundled` feature (bundles SQLite, no system dependency). Synchronous API — never held across `.await` points (do all DB I/O before/after async network calls).

**Key design decisions:**
- `rusqlite::Connection` is `!Send` — all async functions that touch both DB and network must be structured as: sync DB read → async network → sync DB write. Never hold `&Connection` across `.await`.
- All integer fields stored in SQLite use `i64` in Rust (SQLite INTEGER is signed 64-bit; `rusqlite` doesn't impl `ToSql` for `u64`).
- Timestamps use SQLite's `strftime('%Y-%m-%dT%H:%M:%fZ', 'now')` for ISO 8601 format, except `download_log` which accepts caller-provided timestamps for start/end tracking.
- Per-file LFS metadata is fetched via `hf_hub`'s `info_request().query(&[("blobs", "true")])` which returns `blobId`, `size`, and `lfs.sha256` per sibling — this reuses the existing HF auth, no separate `reqwest` client needed.
- The DB path is resolved via `Config::config_dir()?` (static method returning `~/.config/koji/`).

---

### Task 1: Add rusqlite dependency and create DB module with migrations

**Context:**
Koji currently has no database — all state lives in TOML files and the filesystem. We need a SQLite database at `~/.config/koji/koji.db` to store internal metadata that doesn't belong in user-editable TOML cards (commit hashes, file hashes, timestamps, download logs). This task sets up the foundation: the dependency, the module structure, a connection helper, and an automatic migration system. The migration system uses SQLite's `PRAGMA user_version` to track schema version.

**Files:**
- Modify: `Cargo.toml` (workspace root — add rusqlite to workspace dependencies)
- Modify: `crates/koji-core/Cargo.toml` (add rusqlite dependency)
- Create: `crates/koji-core/src/db/mod.rs` (connection helper, migration runner)
- Create: `crates/koji-core/src/db/migrations.rs` (versioned SQL migrations)
- Modify: `crates/koji-core/src/lib.rs` (add `pub mod db;`)

**What to implement:**

1. **Workspace dependency** in root `Cargo.toml`:
   ```toml
   rusqlite = { version = "0.34", features = ["bundled"] }
   ```

2. **`db/mod.rs`** — Public API:
   - `pub mod migrations;`
   - `pub fn open(config_dir: &Path) -> Result<Connection>` — Opens (or creates) `config_dir/koji.db`, enables WAL mode (`PRAGMA journal_mode=WAL`), enables foreign keys (`PRAGMA foreign_keys=ON`), runs migrations, returns the connection.
   - `pub fn open_in_memory() -> Result<Connection>` — For tests. Runs migrations on an in-memory DB.
   - Both functions call `migrations::run(&conn)?` before returning.

3. **`db/migrations.rs`** — Migration system:
   - Use SQLite's `PRAGMA user_version` to track schema version.
   - `pub fn run(conn: &Connection) -> Result<()>` — Reads current `user_version`, applies any migrations with a higher version number. Each individual migration runs in its own transaction. After each successful migration, updates `user_version` to that migration's version.
   - The migrations array is a `&[(i32, &str)]` of `(version, sql)` tuples.
   - Migration v1 creates the initial tables:

   ```sql
   -- Tracks HuggingFace repo state at time of pull
   CREATE TABLE IF NOT EXISTS model_pulls (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       repo_id TEXT NOT NULL,           -- e.g. "bartowski/OmniCoder-8B-GGUF"
       commit_sha TEXT NOT NULL,        -- HF repo HEAD commit hash
       pulled_at TEXT NOT NULL,         -- ISO 8601 timestamp
       UNIQUE(repo_id)                 -- one row per repo, updated on re-pull
   );

   -- Tracks per-file metadata for downloaded GGUFs
   CREATE TABLE IF NOT EXISTS model_files (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       repo_id TEXT NOT NULL,           -- FK-like reference to model_pulls.repo_id
       filename TEXT NOT NULL,          -- e.g. "OmniCoder-8B-Q4_K_M.gguf"
       quant TEXT,                      -- e.g. "Q4_K_M"
       lfs_oid TEXT,                    -- LFS SHA256 content hash
       size_bytes INTEGER,             -- file size (i64 in Rust)
       downloaded_at TEXT NOT NULL,     -- ISO 8601 timestamp
       UNIQUE(repo_id, filename)       -- one row per file per repo
   );

   -- Download event log (append-only)
   CREATE TABLE IF NOT EXISTS download_log (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       repo_id TEXT NOT NULL,
       filename TEXT NOT NULL,
       started_at TEXT NOT NULL,
       completed_at TEXT,
       size_bytes INTEGER,             -- i64 in Rust
       duration_ms INTEGER,            -- i64 in Rust
       success INTEGER NOT NULL DEFAULT 0,
       error_message TEXT
   );

   -- Index for querying download history by repo
   CREATE INDEX IF NOT EXISTS idx_download_log_repo ON download_log(repo_id);
   ```

   **Note on UNIQUE(repo_id) in model_pulls:** The old `commit_sha` and `pulled_at` are overwritten on upsert. This is intentional — the `download_log` table provides historical audit trail. The `model_pulls` table only tracks current state for update comparison.

**Steps:**
- [ ] Add `rusqlite = { version = "0.34", features = ["bundled"] }` to `[workspace.dependencies]` in root `Cargo.toml`
- [ ] Add `rusqlite.workspace = true` to `crates/koji-core/Cargo.toml` under `[dependencies]`
- [ ] Create `crates/koji-core/src/db/mod.rs` with `open()`, `open_in_memory()` functions
- [ ] Create `crates/koji-core/src/db/migrations.rs` with the migration runner and v1 schema
- [ ] Add `pub mod db;` to `crates/koji-core/src/lib.rs`
- [ ] Write tests in `db/mod.rs`:
  - `test_open_in_memory` — opens DB, verifies tables exist by querying `sqlite_master`
  - `test_migrations_idempotent` — runs `migrations::run()` twice, verifies no error
  - `test_user_version_updated` — verifies `PRAGMA user_version` equals latest migration version
- [ ] Run `cargo test --package koji-core -- db::tests`, confirm tests pass
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo build --workspace`, confirm it succeeds
- [ ] Commit with message: "feat: add SQLite database foundation with migration system"

**Acceptance criteria:**
- [ ] `koji.db` is created in the config directory when `db::open()` is called
- [ ] Tables `model_pulls`, `model_files`, and `download_log` exist after migration
- [ ] Index `idx_download_log_repo` exists
- [ ] Calling `open()` on an already-migrated DB is a no-op (idempotent)
- [ ] All tests pass, clippy clean, builds on workspace

---

### Task 2: Add DB query functions for model pull metadata

**Context:**
With the DB schema in place from Task 1, we need typed Rust functions to insert/update/query the `model_pulls` and `model_files` tables. These functions will be called by `cmd_pull()` (to record metadata at pull time) and by the new `cmd_update()` (to compare stored vs remote state). We also need functions for the `download_log` table to record download events, and a cleanup function for when models are deleted. All functions take a `&Connection` parameter — the caller owns the connection. All are synchronous (no async).

**Files:**
- Create: `crates/koji-core/src/db/queries.rs`
- Modify: `crates/koji-core/src/db/mod.rs` (add `pub mod queries;`)

**What to implement:**

`db/queries.rs` — all functions return `Result<T>`:

```rust
/// Insert or update the pull record for a repo.
/// Uses INSERT ... ON CONFLICT(repo_id) DO UPDATE (upsert).
/// Timestamp generated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now').
pub fn upsert_model_pull(
    conn: &Connection,
    repo_id: &str,
    commit_sha: &str,
) -> Result<()>

/// Get the stored pull record for a repo. Returns None if never pulled.
pub fn get_model_pull(
    conn: &Connection,
    repo_id: &str,
) -> Result<Option<ModelPullRecord>>

/// Insert or update a file record for a downloaded GGUF.
/// Uses INSERT ... ON CONFLICT(repo_id, filename) DO UPDATE.
/// Timestamp generated via SQLite's strftime('%Y-%m-%dT%H:%M:%fZ', 'now').
pub fn upsert_model_file(
    conn: &Connection,
    repo_id: &str,
    filename: &str,
    quant: Option<&str>,
    lfs_oid: Option<&str>,
    size_bytes: Option<i64>,
) -> Result<()>

/// Get all stored file records for a repo.
pub fn get_model_files(
    conn: &Connection,
    repo_id: &str,
) -> Result<Vec<ModelFileRecord>>

/// Log a download event (append-only).
pub fn log_download(
    conn: &Connection,
    entry: &DownloadLogEntry,
) -> Result<()>

/// Delete all records for a repo (model_pulls, model_files).
/// Does NOT delete download_log entries (they're historical).
pub fn delete_model_records(
    conn: &Connection,
    repo_id: &str,
) -> Result<()>
```

Record structs (defined in the same file):
```rust
#[derive(Debug, Clone)]
pub struct ModelPullRecord {
    pub repo_id: String,
    pub commit_sha: String,
    pub pulled_at: String,  // ISO 8601 from SQLite
}

#[derive(Debug, Clone)]
pub struct ModelFileRecord {
    pub repo_id: String,
    pub filename: String,
    pub quant: Option<String>,
    pub lfs_oid: Option<String>,
    pub size_bytes: Option<i64>,
    pub downloaded_at: String,
}

#[derive(Debug, Clone)]
pub struct DownloadLogEntry {
    pub repo_id: String,
    pub filename: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub size_bytes: Option<i64>,
    pub duration_ms: Option<i64>,
    pub success: bool,
    pub error_message: Option<String>,
}
```

**Important:** All integer fields that map to SQLite `INTEGER` use `i64` in Rust, not `u64`. This is because `rusqlite` doesn't implement `ToSql` for `u64`. Callers should cast `u64` values with `size as i64` before passing them in. For GGUF files this is safe (files are always < 9.2 EB).

**Steps:**
- [ ] Create `crates/koji-core/src/db/queries.rs` with all functions and structs above
- [ ] Add `pub mod queries;` to `crates/koji-core/src/db/mod.rs`
- [ ] Write tests in `db/queries.rs`:
  - `test_upsert_and_get_model_pull` — insert, read back, verify fields. Update with new SHA, verify it changed.
  - `test_upsert_and_get_model_files` — insert 2 files for same repo, read back, verify count and fields. Update one file's lfs_oid, verify it changed.
  - `test_log_download` — insert a log entry, query it back from the table directly
  - `test_get_model_pull_not_found` — returns None for unknown repo
  - `test_get_model_files_empty` — returns empty vec for unknown repo
  - `test_delete_model_records` — insert records, delete them, verify they're gone. Verify download_log entries are preserved.
- [ ] Run `cargo test --package koji-core -- db`, confirm all pass
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add DB query functions for model pull metadata"

**Acceptance criteria:**
- [ ] All CRUD operations work correctly (insert, upsert, query, delete)
- [ ] Upsert correctly updates existing rows (commit_sha, lfs_oid, timestamps)
- [ ] `delete_model_records` removes pulls and files but preserves download_log
- [ ] All integer fields use `i64` (not `u64`)
- [ ] All tests pass, clippy clean

---

### Task 3: Record metadata in DB during `koji model pull`

**Context:**
Currently `cmd_pull()` downloads GGUF files and saves a TOML model card, but records no commit SHA or file hashes. We need to: (1) capture the repo's `commit_sha` from the `RepoInfo` returned by `hf_hub`, (2) fetch per-file LFS OIDs using `hf_hub`'s `info_request().query(&[("blobs", "true")])`, and (3) write both to the DB after each successful download. This means modifying `list_gguf_files()` to also return the commit SHA and blob metadata, and updating `cmd_pull()` to open the DB and record everything.

**Files:**
- Modify: `crates/koji-core/src/models/pull.rs` (return commit SHA, add blob metadata fetching)
- Modify: `crates/koji-cli/src/commands/model.rs` (open DB in cmd_pull, record metadata)

**What to implement:**

1. **`pull.rs` changes:**

   Change `list_gguf_files()` return type. Currently returns `Result<(String, Vec<RemoteGguf>)>`. Change to return a struct:
   ```rust
   pub struct RepoGgufListing {
       pub repo_id: String,
       pub commit_sha: String,
       pub files: Vec<RemoteGguf>,
   }
   ```
   The `commit_sha` comes from `info.sha` on the `RepoInfo` returned by `repo.info().await`.

   **Update the single call site:** `cmd_pull()` in `model.rs` line 67 destructures the tuple `let (resolved_repo, ggufs) = ...`. Change to destructure the struct: `let listing = pull::list_gguf_files(repo_id).await?;` then use `listing.repo_id`, `listing.commit_sha`, `listing.files`.

   Add a new function to fetch per-file blob metadata using `hf_hub`'s built-in auth:
   ```rust
   /// Fetch per-file blob metadata from HuggingFace using the blobs API.
   /// Uses `hf_hub`'s `info_request().query(&[("blobs", "true")])` which
   /// returns `blobId`, `size`, and `lfs.sha256` per sibling.
   /// Returns a map of filename → BlobInfo for GGUF files.
   pub async fn fetch_blob_metadata(
       repo_id: &str,
   ) -> Result<HashMap<String, BlobInfo>>
   ```
   Where:
   ```rust
   #[derive(Debug, Clone)]
   pub struct BlobInfo {
       pub filename: String,
       pub blob_id: Option<String>,
       pub size: Option<i64>,
       pub lfs_sha256: Option<String>,
   }
   ```

   The implementation:
   - Gets the `hf_api()` singleton
   - Calls `api.model(repo_id).info_request().query(&[("blobs", "true")]).send().await`
   - Deserializes response as `serde_json::Value`
   - Iterates `siblings` array, filters for `.gguf` files
   - Extracts `rfilename`, `blobId`, `size`, and `lfs.sha256` from each sibling
   - Returns `HashMap<filename, BlobInfo>`

   This approach reuses `hf_hub`'s authentication (HF tokens for private repos) rather than making a raw `reqwest` call that would bypass auth.

2. **`cmd_pull()` changes in `model.rs`:**

   After the existing download loop completes and before saving the TOML card:
   - Open the DB: `let db_dir = koji_core::config::Config::config_dir()?;`
     `let conn = koji_core::db::open(&db_dir)?;`
   - Fetch blob metadata: `let blobs = pull::fetch_blob_metadata(repo_id).await?;`
     (This is a separate API call with `blobs=true` — we already have the file list from the initial `list_gguf_files` call but that one doesn't include blob data)
   - For each downloaded quant, call `db::queries::upsert_model_file()` with the filename, quant, `lfs_sha256` from the blobs map, and `size_bytes as i64`
   - Call `db::queries::upsert_model_pull()` with `repo_id` and `commit_sha` from the listing
   - For each downloaded file, call `db::queries::log_download()` with a success entry
   - If a download fails, still log it with `success: false` and the error message

   **Important:** All DB calls are synchronous (`rusqlite`). They happen after all async downloads are complete, so there's no `&Connection` held across `.await` points. This is correct and intentional.

**Steps:**
- [ ] Create `RepoGgufListing` struct in `pull.rs`, update `list_gguf_files()` to return it, capturing `info.sha` as `commit_sha`
- [ ] Update `cmd_pull()` in `model.rs` to destructure `RepoGgufListing` instead of the tuple
- [ ] Add `BlobInfo` struct and `fetch_blob_metadata()` function to `pull.rs`
- [ ] Write a unit test for blob metadata JSON deserialization using a mock JSON string (no network). Test the parsing logic by extracting it into a helper function `parse_blob_siblings(value: &serde_json::Value) -> HashMap<String, BlobInfo>` that can be tested with fixture data.
- [ ] Update `cmd_pull()` to open DB and record metadata after all downloads complete
- [ ] Run `cargo test --package koji-core -- pull`, confirm tests pass
- [ ] Run `cargo test --workspace`, confirm nothing broke
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: record commit SHA and LFS OIDs in DB during model pull"

**Acceptance criteria:**
- [ ] `list_gguf_files()` returns commit SHA alongside file list via `RepoGgufListing`
- [ ] `fetch_blob_metadata()` uses `hf_hub`'s `info_request()` with auth (not a separate `reqwest` client)
- [ ] After `koji model pull`, the DB contains the repo's commit SHA and per-file LFS SHA256 hashes
- [ ] Existing pull functionality (download, TOML card creation, context size selection) is unchanged
- [ ] All tests pass, clippy clean

---

### Task 4: Implement `check_for_updates()` in core

**Context:**
This is the core logic for detecting model updates. It compares locally stored DB metadata against what HuggingFace currently reports. The strategy is two-tier: (1) quick check — compare stored `commit_sha` against remote `RepoInfo.sha`; if identical, the model is up-to-date (single lightweight API call). (2) Per-file check — if the commit SHA differs, fetch blob metadata to compare individual file LFS SHA256 hashes. This avoids re-downloading when only non-GGUF files changed (e.g., README updates).

**Critical design constraint:** `rusqlite::Connection` is `!Send` and cannot be held across `.await` points in tokio. All functions must be structured as: sync DB reads first → async network calls (no `&Connection` reference) → sync DB writes. The comparison logic itself is a pure function with no DB or network access, making it fully testable.

**Files:**
- Create: `crates/koji-core/src/models/update.rs`
- Modify: `crates/koji-core/src/models/mod.rs` (add `pub mod update;`)

**What to implement:**

Type definitions:
```rust
/// Result of checking a single model for updates.
#[derive(Debug)]
pub struct UpdateCheckResult {
    pub repo_id: String,
    pub status: UpdateStatus,
    pub file_updates: Vec<FileUpdateInfo>,
}

#[derive(Debug)]
pub enum UpdateStatus {
    /// No stored metadata — model was pulled before DB existed
    NoPriorRecord,
    /// Commit SHA matches — repo hasn't changed at all
    UpToDate,
    /// Commit SHA differs but all tracked GGUF files are unchanged
    RepoChangedFilesUnchanged,
    /// One or more tracked GGUF files have changed
    UpdatesAvailable,
    /// Error checking (network, API, etc.)
    CheckFailed(String),
}

#[derive(Debug)]
pub struct FileUpdateInfo {
    pub filename: String,
    pub quant: Option<String>,
    pub status: FileStatus,
    pub local_size: Option<i64>,
    pub remote_size: Option<i64>,
}

#[derive(Debug)]
pub enum FileStatus {
    Unchanged,
    /// LFS SHA256 changed
    Changed { old_oid: String, new_oid: String },
    /// New remote file not locally downloaded
    NewRemote,
    /// No stored hash to compare (legacy pull without DB)
    Unknown,
    /// File was removed from remote
    RemovedFromRemote,
}
```

Main function — structured to avoid holding `&Connection` across `.await`:
```rust
/// Check a single model for updates against HuggingFace.
pub async fn check_for_updates(
    conn: &Connection,
    repo_id: &str,
) -> UpdateCheckResult
```

**Note the return type is `UpdateCheckResult` not `Result<UpdateCheckResult>`**. Network errors are captured as `UpdateStatus::CheckFailed(message)` so that one model's failure doesn't abort checking all others. Only internal errors (DB failures) propagate as `Err`.

Actually, to keep it simpler: return `Result<UpdateCheckResult>` but catch network errors within the function and return `Ok(UpdateCheckResult { status: CheckFailed(...) })`. Let DB errors propagate.

Implementation logic (structured for `!Send` safety):
```rust
pub async fn check_for_updates(
    conn: &Connection,
    repo_id: &str,
) -> Result<UpdateCheckResult> {
    // Step 1: SYNC — read from DB (no .await)
    let pull_record = db::queries::get_model_pull(conn, repo_id)?;
    let file_records = db::queries::get_model_files(conn, repo_id)?;

    // Step 2: handle no prior record
    if pull_record.is_none() {
        return Ok(UpdateCheckResult {
            repo_id: repo_id.to_string(),
            status: UpdateStatus::NoPriorRecord,
            file_updates: vec![],
        });
    }
    let pull_record = pull_record.unwrap();

    // Step 3: ASYNC — fetch remote state (conn not referenced)
    let remote_listing = match pull::list_gguf_files(repo_id).await {
        Ok(listing) => listing,
        Err(e) => return Ok(UpdateCheckResult {
            repo_id: repo_id.to_string(),
            status: UpdateStatus::CheckFailed(e.to_string()),
            file_updates: vec![],
        }),
    };

    // Step 4: quick check — commit SHA match?
    if remote_listing.commit_sha == pull_record.commit_sha {
        return Ok(UpdateCheckResult {
            repo_id: repo_id.to_string(),
            status: UpdateStatus::UpToDate,
            file_updates: vec![],
        });
    }

    // Step 5: ASYNC — fetch per-file blob metadata
    let remote_blobs = match pull::fetch_blob_metadata(repo_id).await {
        Ok(blobs) => blobs,
        Err(e) => return Ok(UpdateCheckResult {
            repo_id: repo_id.to_string(),
            status: UpdateStatus::CheckFailed(
                format!("Commit changed but failed to fetch file details: {}", e)
            ),
            file_updates: vec![],
        }),
    };

    // Step 6: PURE — compare local vs remote (testable, no I/O)
    let file_updates = compare_files(&file_records, &remote_blobs);

    // Step 7: determine overall status
    let has_changes = file_updates.iter().any(|f| matches!(
        f.status,
        FileStatus::Changed { .. } | FileStatus::NewRemote
    ));

    let status = if has_changes {
        UpdateStatus::UpdatesAvailable
    } else {
        UpdateStatus::RepoChangedFilesUnchanged
    };

    Ok(UpdateCheckResult {
        repo_id: repo_id.to_string(),
        status,
        file_updates,
    })
}
```

Extract the pure comparison logic into a separate testable function:
```rust
/// Compare local file records against remote blob metadata.
/// This is a pure function with no I/O — fully unit-testable.
pub fn compare_files(
    local_files: &[ModelFileRecord],
    remote_blobs: &HashMap<String, BlobInfo>,
) -> Vec<FileUpdateInfo>
```

Also add:
```rust
/// Refresh DB metadata for a model without re-downloading.
/// Fetches current commit SHA and file LFS OIDs from HF and writes to DB.
/// Used to establish a baseline for models pulled before the DB existed.
pub async fn refresh_metadata(
    conn: &Connection,
    repo_id: &str,
) -> Result<()>
```

This function also follows the sync-async-sync pattern:
1. ASYNC: fetch `list_gguf_files()` for commit SHA, `fetch_blob_metadata()` for file hashes
2. SYNC: write to DB via `upsert_model_pull()` and `upsert_model_file()` for each GGUF

**Steps:**
- [ ] Create `crates/koji-core/src/models/update.rs` with all types and functions above
- [ ] Add `pub mod update;` to `crates/koji-core/src/models/mod.rs`
- [ ] Write unit tests:
  - `test_compare_files_unchanged` — all local files match remote OIDs → all Unchanged
  - `test_compare_files_changed` — one file has different OID → Changed with old/new OIDs
  - `test_compare_files_new_remote` — remote has GGUF not in local → NewRemote
  - `test_compare_files_removed` — local has file not in remote → RemovedFromRemote
  - `test_compare_files_unknown` — local file has no lfs_oid → Unknown
  - `test_compare_files_mixed` — combination of all statuses
  - `test_check_no_prior_record` — uses in-memory DB with no data, verifies NoPriorRecord
- [ ] Run `cargo test --package koji-core -- models::update`, confirm tests pass
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit with message: "feat: add check_for_updates core logic for model update detection"

**Acceptance criteria:**
- [ ] `compare_files()` is a pure function with no I/O dependencies — fully testable
- [ ] `check_for_updates()` never holds `&Connection` across `.await` points
- [ ] Network errors are captured as `CheckFailed`, not propagated as `Err`
- [ ] DB errors (connection issues) propagate as `Err`
- [ ] `refresh_metadata()` stamps the DB without downloading files
- [ ] All tests pass, clippy clean

---

### Task 5: Add `koji model update` CLI command

**Context:**
This is the user-facing command that ties everything together. It supports three modes: (1) `koji model update` — check all installed models for updates, (2) `koji model update <model>` — check a specific model, (3) `koji model update --check` — dry-run that only reports status without downloading. There's also `koji model update --refresh` to stamp existing models with metadata without re-downloading. The `--check` and `--refresh` flags are mutually exclusive. The command opens the DB, iterates models, calls `check_for_updates()`, displays results, and optionally re-downloads changed files.

**Files:**
- Modify: `crates/koji-cli/src/cli.rs` (add `Update` variant to `ModelCommands`)
- Modify: `crates/koji-cli/src/commands/model.rs` (add `cmd_update()` function, add match arm)

**What to implement:**

1. **CLI definition** in `cli.rs` — add to `ModelCommands` enum:
   ```rust
   /// Check for and download model updates from HuggingFace
   Update {
       /// Model ID to update (e.g. "bartowski/OmniCoder-8B-GGUF"). Checks all if omitted.
       model: Option<String>,
       /// Only check for updates, don't download
       #[arg(long, conflicts_with = "refresh")]
       check: bool,
       /// Refresh stored metadata without re-downloading (establishes baseline for future checks)
       #[arg(long, conflicts_with = "check")]
       refresh: bool,
       /// Skip confirmation prompt (for scripting/CI)
       #[arg(long, short = 'y')]
       yes: bool,
   },
   ```

2. **Match arm in `run()`:**
   ```rust
   ModelCommands::Update { model, check, refresh, yes } =>
       cmd_update(config, model, check, refresh, yes).await,
   ```

3. **`cmd_update()` in `model.rs`:**

   ```rust
   async fn cmd_update(
       config: &Config,
       model_filter: Option<String>,
       check_only: bool,
       refresh: bool,
       yes: bool,
   ) -> Result<()>
   ```

   Logic:
   1. Open DB: `let db_dir = Config::config_dir()?;` then `let conn = koji_core::db::open(&db_dir)?;`
   2. Build list of models to check:
      - `let models_dir = config.models_dir()?;`
      - `let configs_dir = config.configs_dir()?;`
      - `let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());`
      - If `model_filter` is `Some(id)` → `registry.find(&id)?` to get single model
      - If `None` → `registry.scan()?` for all installed models
   3. If `refresh` flag:
      - For each model, call `update::refresh_metadata(&conn, &model.card.model.source).await?`
      - Print "  Metadata refreshed for {model_id}"
      - At end print "Metadata refreshed."
      - Return
   4. For each model, call `update::check_for_updates(&conn, &model.card.model.source).await?`
   5. Display results:
      ```
      Checking for updates...

      bartowski/OmniCoder-8B-GGUF
        Status: Updates available
        Q4_K_M  OmniCoder-8B-Q4_K_M.gguf  changed (4.2 GiB → 4.3 GiB)
        Q8_0    OmniCoder-8B-Q8_0.gguf     unchanged

      bartowski/Llama-3-8B-GGUF
        Status: Up to date

      someone/NewModel-GGUF
        Status: No prior record (run with --refresh to enable tracking)
      ```
   6. If `check_only` → stop after display
   7. Otherwise, collect models with `UpdatesAvailable` status
      - If none → print "All models up to date." and return
      - Show confirmation prompt (unless `--yes`): "Download updates for N file(s)? (Y/n)"
      - For each changed/new file:
        - Delete the existing local file (if `Changed`)
        - Call `pull::download_gguf()` to re-download
        - Update DB: `upsert_model_file()` and `log_download()`
      - After all downloads: `upsert_model_pull()` with new commit SHA
      - Update TOML card's `size_bytes` if file size changed, save card
   8. Print "Models updated."

   Also add `koji_core::db` to the `use` imports at the top of the file. Add `use koji_core::models::update;` for the update module.

4. **Update `cmd_rm()`** to also clean up DB records:
   After removing files and the model card (around line 498 in current code), add:
   ```rust
   // Clean up DB metadata
   if let Ok(db_dir) = Config::config_dir() {
       if let Ok(conn) = koji_core::db::open(&db_dir) {
           let _ = koji_core::db::queries::delete_model_records(&conn, &model.id);
       }
   }
   ```
   Use `let _` / `if let Ok` to make DB cleanup best-effort — model deletion should succeed even if DB is unavailable.

**Steps:**
- [ ] Add `Update` variant to `ModelCommands` in `cli.rs` with `conflicts_with` annotations
- [ ] Add match arm in `run()` function in `model.rs`
- [ ] Implement `cmd_update()` in `model.rs`
- [ ] Update `cmd_rm()` to call `delete_model_records()` (best-effort)
- [ ] Run `cargo build --workspace`, confirm it compiles
- [ ] Test manually:
  - `koji model update --check` (dry-run on installed models)
  - `koji model update --refresh` (stamp metadata for existing models)
  - `koji model update <specific-model>` (single model check)
  - `koji model update --check --refresh` (should error due to conflicts_with)
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`, confirm all tests pass
- [ ] Commit with message: "feat: add koji model update command for checking and downloading model updates"

**Acceptance criteria:**
- [ ] `koji model update` checks all installed models and displays status
- [ ] `koji model update <model>` checks a single model
- [ ] `--check` flag prevents downloads (dry-run)
- [ ] `--refresh` flag stamps metadata without downloading
- [ ] `--check` and `--refresh` are mutually exclusive (clap enforces)
- [ ] `--yes` / `-y` skips confirmation prompt
- [ ] Changed files are re-downloaded and DB + TOML card updated
- [ ] Models with no prior DB record show a helpful message pointing to `--refresh`
- [ ] `koji model rm` cleans up DB records (best-effort)
- [ ] All tests pass, clippy clean, builds on workspace
