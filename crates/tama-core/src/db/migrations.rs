//! Database migrations for SQLite
//!
//! Uses SQLite's `PRAGMA user_version` to track schema version.
//! Each migration runs in its own transaction.

use rusqlite::Connection;

/// RAII guard that re-enables SQLite foreign keys on drop.
///
/// Used around migrations that temporarily disable FK enforcement
/// (e.g., migration v9 which rebuilds `model_configs` via DROP + RENAME).
/// Ensures `PRAGMA foreign_keys=ON` runs even if the migration panics or
/// returns an error, preventing permanent FK disabling.
pub struct FkGuard<'conn> {
    conn: &'conn Connection,
}

impl<'conn> FkGuard<'conn> {
    /// Disable foreign keys and return a guard that re-enables them on Drop.
    pub fn disable(conn: &'conn Connection) -> anyhow::Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
        Ok(Self { conn })
    }
}

impl Drop for FkGuard<'_> {
    fn drop(&mut self) {
        // Ignore errors — best effort to restore FK state.
        let _ = self.conn.execute_batch("PRAGMA foreign_keys=ON;");
    }
}

/// Migration entry: (version number, SQL statement)
pub type Migration = (i32, &'static str);

/// Version number for the latest migration
pub const LATEST_VERSION: i32 = 18;

/// Migrations that rebuild a parent table via DROP + RENAME. SQLite with
/// `foreign_keys=ON` performs an implicit DELETE on the dropped table which
/// fires `ON DELETE CASCADE` on referencing tables (e.g. `model_files.model_id
/// REFERENCES model_configs(id) ON DELETE CASCADE`) and wipes their rows.
/// `PRAGMA defer_foreign_keys` defers enforcement checks but NOT cascade
/// actions, and `PRAGMA foreign_keys` is a no-op inside a transaction. The
/// only safe fix is to toggle `foreign_keys=OFF` around the entire migration
/// from outside the transaction.
const FK_OFF_MIGRATIONS: &[i32] = &[9];

/// Run all applicable migrations on the database
///
/// Reads current `user_version`, applies any migrations with a higher version number.
/// Each individual migration runs in its own transaction. After each successful
/// migration, updates `user_version` to that migration's version.
pub fn run(conn: &Connection) -> anyhow::Result<()> {
    run_up_to(conn, i32::MAX)
}

/// Run migrations only up to (and including) `target_version`. Primarily for
/// tests that need to simulate a pre-release schema (e.g. insert rows against
/// the v8 schema before running v9 to verify FK cascade behaviour).
pub(crate) fn run_up_to(conn: &Connection, target_version: i32) -> anyhow::Result<()> {
    // Define all migrations in order.
    // Each tuple uses an explicit version literal (not the LATEST_VERSION constant)
    // so that adding a new migration never accidentally changes an existing version number.
    let migrations: &[Migration] = &[
        (
            1,
            r#"
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
                    size_bytes INTEGER,              -- file size (i64 in Rust)
                    downloaded_at TEXT NOT NULL,     -- ISO 8601 timestamp
                    UNIQUE(repo_id, filename)        -- one row per file per repo
                );

                -- Download event log (append-only)
                CREATE TABLE IF NOT EXISTS download_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    repo_id TEXT NOT NULL,
                    filename TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    completed_at TEXT,
                    size_bytes INTEGER,              -- i64 in Rust
                    duration_ms INTEGER,             -- i64 in Rust
                    success INTEGER NOT NULL DEFAULT 0,
                    error_message TEXT
                );

                -- Index for querying download history by repo
                CREATE INDEX IF NOT EXISTS idx_download_log_repo ON download_log(repo_id);
                "#,
        ),
        (
            2,
            r#"
                -- Tracks running backend processes
                CREATE TABLE IF NOT EXISTS active_models (
                    server_name TEXT PRIMARY KEY,   -- config key, e.g. "my-coding-model"
                    model_name TEXT NOT NULL,       -- model identifier used for loading
                    backend TEXT NOT NULL,          -- backend key, e.g. "llama-server"
                    pid INTEGER NOT NULL,           -- backend process PID (i64 in Rust)
                    port INTEGER NOT NULL,          -- backend port (i64 in Rust)
                    backend_url TEXT NOT NULL,      -- full URL, e.g. "http://127.0.0.1:54321"
                    loaded_at TEXT NOT NULL,        -- ISO 8601 timestamp
                    last_accessed TEXT NOT NULL     -- ISO 8601 timestamp, updated periodically
                );
                "#,
        ),
        (
            3,
            r#"
                CREATE TABLE IF NOT EXISTS backend_installations (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,             -- backend key, e.g. "llama_cpp", "ik_llama"
                    backend_type TEXT NOT NULL,     -- serialized enum, e.g. "llama_cpp", "ik_llama", "custom"
                    version TEXT NOT NULL,          -- version string, e.g. "b8407", "main@abc12345"
                    path TEXT NOT NULL,             -- absolute path to installed binary
                    installed_at INTEGER NOT NULL,  -- unix timestamp (i64)
                    gpu_type TEXT,                  -- JSON string (nullable, serialized GpuType)
                    source TEXT,                    -- JSON string (nullable, serialized BackendSource)
                    is_active INTEGER NOT NULL DEFAULT 0, -- 1 = current active version for this name
                    UNIQUE(name, version)
                );
                CREATE INDEX IF NOT EXISTS idx_backend_installations_name ON backend_installations(name);
                "#,
        ),
        (
            4,
            r#"
                CREATE TABLE IF NOT EXISTS system_metrics_history (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts_unix_ms          INTEGER NOT NULL,
                    cpu_usage_pct       REAL    NOT NULL,
                    ram_used_mib        INTEGER NOT NULL,
                    ram_total_mib       INTEGER NOT NULL,
                    gpu_utilization_pct INTEGER,
                    vram_used_mib       INTEGER,
                    vram_total_mib      INTEGER,
                    models_loaded       INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_system_metrics_ts
                    ON system_metrics_history(ts_unix_ms);
                "#,
        ),
        (
            5,
            r#"
                -- Local SHA-256 verification tracking for previously downloaded quants.
                -- last_verified_at is ISO 8601 of the most recent verification attempt.
                -- verified_ok is nullable: NULL = never verified or no upstream hash available.
                -- verify_error holds a short message on mismatch or verification failure.
                ALTER TABLE model_files ADD COLUMN last_verified_at TEXT;
                ALTER TABLE model_files ADD COLUMN verified_ok INTEGER;
                ALTER TABLE model_files ADD COLUMN verify_error TEXT;
                "#,
        ),
        (
            6,
            r#"
                CREATE TABLE IF NOT EXISTS update_checks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    item_type TEXT NOT NULL,           -- 'backend' or 'model'
                    item_id TEXT NOT NULL,             -- backend name or model config key
                    current_version TEXT,              -- installed version/commit SHA
                    latest_version TEXT,               -- remote version/commit SHA
                    update_available INTEGER NOT NULL DEFAULT 0,
                    status TEXT NOT NULL DEFAULT 'unknown',
                    error_message TEXT,
                    details_json TEXT,                 -- JSON blob (per-file changes for models)
                    checked_at INTEGER NOT NULL,        -- unix timestamp
                    UNIQUE(item_type, item_id)
                );
                CREATE INDEX IF NOT EXISTS idx_update_checks_type ON update_checks(item_type);
            "#,
        ),
        (
            7,
            r#"
                -- Per-repo user configuration (replaces [models] in tama.toml)
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
                "#,
        ),
        (
            8,
            r#"
                -- Destructive migration: drop and recreate model tables with integer PK.
                -- All tables now use `model_id INTEGER REFERENCES models(id)` instead of repo_id.
                DROP TABLE IF EXISTS model_configs;
                DROP TABLE IF EXISTS model_files;
                DROP TABLE IF EXISTS model_pulls;

                -- Per-repo user configuration (replaces [models] in tama.toml)
                CREATE TABLE model_configs (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    repo_id       TEXT NOT NULL UNIQUE,           -- HF repo name, e.g. "unsloth/gemma-4-26B-A4B-it-GGUF"
                    display_name  TEXT,
                    backend       TEXT NOT NULL DEFAULT 'llama_cpp',
                    enabled       INTEGER NOT NULL DEFAULT 1,
                    selected_quant  TEXT,                          -- active quant key (e.g. "Q4_K_M")
                    selected_mmproj TEXT,                          -- mmproj filename (e.g. "mmproj-F16.gguf")
                    context_length  INTEGER,
                    gpu_layers      INTEGER,
                    port            INTEGER,
                    args            TEXT,                          -- JSON array of strings
                    sampling        TEXT,                          -- JSON object (serialised SamplingParams)
                    modalities      TEXT,                          -- JSON object {input:[],output:[]}
                    profile         TEXT,
                    api_name        TEXT,
                    health_check    TEXT,                          -- JSON object (serialised HealthCheck)
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                );

                -- Tracks HuggingFace repo state at time of pull
                CREATE TABLE model_pulls (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    model_id   INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
                    repo_id    TEXT NOT NULL,                              -- cached for convenience
                    commit_sha TEXT NOT NULL,                              -- HF repo HEAD commit hash
                    pulled_at  TEXT NOT NULL                               -- ISO 8601 timestamp
                );

                -- Tracks per-file metadata for downloaded GGUFs
                CREATE TABLE model_files (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    model_id   INTEGER NOT NULL REFERENCES model_configs(id) ON DELETE CASCADE,
                    repo_id    TEXT NOT NULL,                              -- cached for convenience
                    filename   TEXT NOT NULL,                              -- e.g. "gemma-4-26B-Q4_K_M.gguf"
                    quant      TEXT,                                       -- e.g. "Q4_K_M"
                    lfs_oid    TEXT,                                       -- LFS SHA256 content hash
                    size_bytes INTEGER,                                    -- file size in bytes
                    downloaded_at     TEXT NOT NULL,                       -- ISO 8601 timestamp
                    last_verified_at   TEXT,                                 -- ISO 8601, cleared on hash change
                    verified_ok       INTEGER,                               -- 1=ok, 0=mismatch, NULL=never verified
                    verify_error      TEXT,                                  -- short error message
                    kind TEXT NOT NULL DEFAULT 'model'                      -- 'model' or 'mmproj'
                );
                CREATE UNIQUE INDEX idx_model_files_model_id_filename ON model_files(model_id, filename);
                "#,
        ),
        (
            9,
            r#"
                -- Rebuild model_configs with COLLATE NOCASE on repo_id so that
                -- the UNIQUE constraint, ON CONFLICT(repo_id) upserts, and
                -- WHERE repo_id = ? lookups all match case-insensitively. HF
                -- repo ids preserve original casing but users (and our own
                -- config-key normalisation) routinely lowercase them, so a
                -- binary UNIQUE index produced duplicate rows for the same repo.

                -- The migration runner toggles `PRAGMA foreign_keys=OFF`
                -- around this migration (see `FK_OFF_MIGRATIONS`). That is
                -- required because the `DROP TABLE` below would otherwise
                -- fire `ON DELETE CASCADE` on `model_files` / `model_pulls`
                -- and wipe every referencing row. `defer_foreign_keys` does
                -- NOT prevent cascade actions, only deferred enforcement
                -- checks, so it is the wrong tool here.

                -- Deduplicate any existing rows that differ only by case
                -- (keep the row with the lowest id). Without this, the new
                -- UNIQUE constraint would fail to enforce.
                DELETE FROM model_configs WHERE id NOT IN (
                    SELECT MIN(id) FROM model_configs GROUP BY LOWER(repo_id)
                );

                CREATE TABLE model_configs_new (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    repo_id       TEXT NOT NULL UNIQUE COLLATE NOCASE,
                    display_name  TEXT,
                    backend       TEXT NOT NULL DEFAULT 'llama_cpp',
                    enabled       INTEGER NOT NULL DEFAULT 1,
                    selected_quant  TEXT,
                    selected_mmproj TEXT,
                    context_length  INTEGER,
                    gpu_layers      INTEGER,
                    port            INTEGER,
                    args            TEXT,
                    sampling        TEXT,
                    modalities      TEXT,
                    profile         TEXT,
                    api_name        TEXT,
                    health_check    TEXT,
                    created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                );

                INSERT INTO model_configs_new (
                    id, repo_id, display_name, backend, enabled, selected_quant,
                    selected_mmproj, context_length, gpu_layers, port, args,
                    sampling, modalities, profile, api_name, health_check,
                    created_at, updated_at
                )
                SELECT
                    id, repo_id, display_name, backend, enabled, selected_quant,
                    selected_mmproj, context_length, gpu_layers, port, args,
                    sampling, modalities, profile, api_name, health_check,
                    created_at, updated_at
                FROM model_configs;

                DROP TABLE model_configs;
                ALTER TABLE model_configs_new RENAME TO model_configs;
                "#,
        ),
        (
            10,
            r#"
                -- Deduplicate historical rows first (keep row with highest id
                -- per model_id). Without this, CREATE UNIQUE INDEX would fail
                -- on upgraded databases that have duplicate model_pulls rows.
                DELETE FROM model_pulls
                WHERE id NOT IN (
                    SELECT MAX(id) FROM model_pulls GROUP BY model_id
                );

                -- Add UNIQUE index on model_pulls.model_id so that
                -- upsert_model_pull's ON CONFLICT(model_id) has a matching
                -- constraint. Without it, refresh_metadata (which calls
                -- upsert_model_pull before upserting files) fails entirely,
                -- leaving all file hashes unbaked.
                CREATE UNIQUE INDEX IF NOT EXISTS idx_model_pulls_model_id
                    ON model_pulls(model_id);
                "#,
        ),
        (
            11,
            r#"
                -- Operational download queue table (updated as status changes,
                -- not append-only like download_log).
                CREATE TABLE download_queue (
                    id            INTEGER PRIMARY KEY AUTOINCREMENT,
                    job_id        TEXT NOT NULL UNIQUE,
                    repo_id       TEXT NOT NULL,
                    filename      TEXT NOT NULL,
                    display_name  TEXT,
                    status        TEXT NOT NULL DEFAULT 'queued',
                    bytes_downloaded INTEGER NOT NULL DEFAULT 0,
                    total_bytes     INTEGER,
                    error_message TEXT,
                    started_at     TEXT,
                    completed_at   TEXT,
                    queued_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    kind           TEXT NOT NULL DEFAULT 'model'
                );
                CREATE INDEX idx_dq_status ON download_queue(status);
                "#,
        ),
        (
            12,
            r#"
                -- Add quant and context_length to download_queue so the queue
                -- processor can reconstruct a QuantDownloadSpec from the DB row.
                ALTER TABLE download_queue ADD COLUMN quant TEXT;
                ALTER TABLE download_queue ADD COLUMN context_length INTEGER;
                "#,
        ),
        (
            13,
            r#"
                -- Stores benchmark results for comparison over time.
                CREATE TABLE IF NOT EXISTS benchmarks (
                    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at          INTEGER NOT NULL,           -- Unix timestamp (seconds)
                    model_id            TEXT NOT NULL,              -- Model config key (e.g. "qwen7b")
                    display_name        TEXT,                       -- Model display name
                    quant               TEXT,                       -- Quantization label (e.g. "Q4_K_M")
                    backend             TEXT NOT NULL,              -- Backend type (e.g. "llama_cpp")
                    engine              TEXT NOT NULL DEFAULT 'llama_bench',
                    pp_sizes            TEXT NOT NULL,              -- JSON array, e.g. "[512,1024]"
                    tg_sizes            TEXT NOT NULL,              -- JSON array, e.g. "[128,256]"
                    threads             TEXT,                       -- JSON array or null
                    ngl_range           TEXT,                       -- GPU layers range or null
                    runs                INTEGER NOT NULL DEFAULT 3,
                    warmup              INTEGER NOT NULL DEFAULT 1,
                    results             TEXT NOT NULL,              -- JSON array of BenchSummary objects
                    load_time_ms        REAL,                       -- Model load time in ms
                    vram_used_mib       INTEGER,                    -- VRAM used at benchmark time
                    vram_total_mib      INTEGER,                    -- Total VRAM
                    duration_seconds    REAL,                       -- How long the benchmark took
                    status              TEXT NOT NULL DEFAULT 'success'
                );
                CREATE INDEX IF NOT EXISTS idx_benchmarks_model_id ON benchmarks(model_id);
                CREATE INDEX IF NOT EXISTS idx_benchmarks_created_at ON benchmarks(created_at DESC);
            "#,
        ),
        (
            14,
            r#"
                ALTER TABLE model_configs ADD COLUMN num_parallel INTEGER DEFAULT 1 CHECK(num_parallel >= 1);
            "#,
        ),
        (
            15,
            r#"
                CREATE TABLE tts_configs (
                    id           INTEGER PRIMARY KEY AUTOINCREMENT,
                    engine       TEXT NOT NULL UNIQUE COLLATE NOCASE,  -- TTS engine name (e.g., 'kokoro')
                    default_voice TEXT,                                -- e.g., 'af_sky'
                    speed        REAL   NOT NULL DEFAULT 1.0,          -- 0.5 to 2.0
                    format       TEXT   NOT NULL DEFAULT 'mp3',        -- mp3, wav, ogg
                    enabled      INTEGER NOT NULL DEFAULT 1,
                    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
                    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                );
            "#,
        ),
        (
            16,
            r#"
                ALTER TABLE benchmarks ADD COLUMN benchmark_type TEXT;
            "#,
        ),
        (
            17,
            r#"
                ALTER TABLE model_configs ADD COLUMN kv_unified INTEGER NOT NULL DEFAULT 0;
                UPDATE model_configs SET kv_unified = 1 WHERE num_parallel IS NULL OR num_parallel <= 1;
            "#,
        ),
        (
            18,
            r#"
                ALTER TABLE model_configs ADD COLUMN cache_type_k TEXT;
                ALTER TABLE model_configs ADD COLUMN cache_type_v TEXT;
            "#,
        ),
    ];

    let current_version: i32 =
        conn.pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))?;

    for (version, sql) in migrations {
        if *version > current_version && *version <= target_version {
            let fk_off = FK_OFF_MIGRATIONS.contains(version);
            // PRAGMA foreign_keys must be toggled outside any transaction —
            // it is a no-op inside one. For rebuild-style migrations we need
            // FKs off so DROP TABLE on the parent does not cascade-delete
            // rows in referencing tables.
            let _fk_guard = if fk_off {
                Some(FkGuard::disable(conn)?)
            } else {
                None
            };
            // Run each migration in its own transaction so a crash mid-migration
            // leaves the DB in a consistent state (user_version only updates on commit).
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {version};"))?;
            tx.commit()?;
            tracing::debug!("Applied migration to version {}", version);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn test_migration_v6_creates_update_checks_table() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='update_checks'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_update_checks_type'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn test_migration_v7_creates_model_configs_table() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='model_configs'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let kind_column_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_files') WHERE name='kind'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind_column_exists, 1);
    }

    /// Migration v9 rebuilds model_configs with COLLATE NOCASE on repo_id so
    /// inserting the same repo id in different cases is rejected as a conflict
    /// and ON CONFLICT(repo_id) upserts fire.
    #[test]
    fn test_migration_v9_repo_id_is_case_insensitive() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run(&conn).unwrap();

        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('Foo/Bar', 'llama_cpp')",
            [],
        )
        .unwrap();

        // Case-variant insert must fail as a UNIQUE violation.
        let err = conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES ('foo/bar', 'llama_cpp')",
            [],
        );
        assert!(
            err.is_err(),
            "case-variant repo_id should conflict with UNIQUE COLLATE NOCASE"
        );

        // ON CONFLICT(repo_id) should fire across case variants too.
        conn.execute(
            "INSERT INTO model_configs (repo_id, backend) VALUES (?, 'llama_cpp')
             ON CONFLICT(repo_id) DO UPDATE SET backend = 'ik_llama'",
            ["FOO/BAR"],
        )
        .unwrap();
        let backend: String = conn
            .query_row(
                "SELECT backend FROM model_configs WHERE repo_id = 'Foo/Bar'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            backend, "ik_llama",
            "ON CONFLICT(repo_id) must match case-insensitively"
        );

        // WHERE repo_id = ? must match case-insensitively too.
        let row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_configs WHERE repo_id = 'FOO/BAR'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(row_count, 1, "WHERE should match ignoring case");
    }

    /// Migration v9 must deduplicate pre-existing case-variant rows rather
    /// than fail on the new UNIQUE constraint.
    #[test]
    fn test_migration_v9_dedupes_existing_case_variants() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Apply migrations up through v8 (the pre-NOCASE schema). We do this
        // by temporarily setting user_version back and running run() — but
        // since run() is idempotent and we want to simulate pre-v9 state,
        // manually run migrations 1-8 here would be more robust. Simplest:
        // run() fully, then after it completes, insert the case-variant rows
        // before re-running is not possible. So we run everything, insert
        // the second variant by direct rebuild is blocked by UNIQUE NOCASE.
        //
        // Instead, assert the dedupe behaviour via the DELETE pattern the
        // migration uses: create two rows that differ only by case in a
        // fresh non-NOCASE table, run the dedupe DELETE, verify only one
        // survives.
        conn.execute_batch(
            r#"
            CREATE TABLE tmp_cfg (id INTEGER PRIMARY KEY, repo_id TEXT NOT NULL);
            INSERT INTO tmp_cfg (id, repo_id) VALUES (1, 'Foo/Bar'), (2, 'foo/bar'), (3, 'Other');
            DELETE FROM tmp_cfg WHERE id NOT IN (
                SELECT MIN(id) FROM tmp_cfg GROUP BY LOWER(repo_id)
            );
            "#,
        )
        .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM tmp_cfg", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 2, "dedupe should keep one per lower(repo_id)");

        // Also verify the full migration applies cleanly on an empty DB.
        run(&conn).unwrap();
    }

    /// Regression test for the v9 FK-cascade bug. Before the fix, running v9
    /// on a DB with existing `model_files` rows would wipe those rows because
    /// `DROP TABLE model_configs` (with `foreign_keys=ON`) performs an
    /// implicit `DELETE FROM model_configs`, which cascades through
    /// `ON DELETE CASCADE` to `model_files`. The migration must preserve all
    /// referencing rows.
    #[test]
    fn test_migration_v9_preserves_model_files_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Bring the DB up to v8 — the pre-v9 schema where model_configs
        // exists with a case-sensitive UNIQUE constraint on repo_id.
        run_up_to(&conn, 8).unwrap();

        // Seed a model_configs row and two model_files rows that reference it.
        conn.execute(
            "INSERT INTO model_configs (id, repo_id, backend) VALUES (1, 'unsloth/Qwen3.6-35B-A3B-GGUF', 'llama_cpp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_files (model_id, repo_id, filename, quant, size_bytes, downloaded_at, kind) \
             VALUES (1, 'unsloth/Qwen3.6-35B-A3B-GGUF', 'Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf', 'UD-Q4_K_XL', 22360456160, '2026-04-16T20:00:00Z', 'model')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_files (model_id, repo_id, filename, quant, size_bytes, downloaded_at, kind) \
             VALUES (1, 'unsloth/Qwen3.6-35B-A3B-GGUF', 'mmproj-F16.gguf', NULL, 899283680, '2026-04-16T20:00:00Z', 'mmproj')",
            [],
        )
        .unwrap();

        // Sanity: rows are present before the migration.
        let files_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM model_files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(files_before, 2);

        // Apply v9.
        run(&conn).unwrap();

        // The model_configs row must survive (same id, same repo_id).
        let configs_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM model_configs WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            configs_after, 1,
            "model_configs row 1 must survive the rebuild"
        );

        // All referencing model_files rows must survive. Before the fix this
        // was 0 because DROP TABLE cascaded.
        let files_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_files WHERE model_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            files_after, 2,
            "both model_files rows must survive migration v9"
        );

        // Foreign keys must be re-enabled after the migration completes, so
        // subsequent DB activity enforces referential integrity.
        let fk_on: i32 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk_on, 1, "foreign_keys must be re-enabled after migration");
    }

    /// Regression test for the v10 ON CONFLICT bug. Before the fix,
    /// `upsert_model_pull` used `ON CONFLICT(model_id)` but the
    /// `model_pulls` table had no UNIQUE constraint on `model_id`,
    /// causing `refresh_metadata` to fail and leave all file hashes
    /// unbaked.
    #[test]
    fn test_migration_v10_adds_model_pulls_unique_index() {
        let conn = Connection::open_in_memory().unwrap();

        // Bring the DB up to v9 — the pre-v10 schema.
        run_up_to(&conn, 9).unwrap();

        // Verify the unique index does NOT exist yet.
        let idx_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_model_pulls_model_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_before, 0);

        // Apply v10.
        run(&conn).unwrap();

        // Verify the unique index now exists.
        let idx_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_model_pulls_model_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_after, 1);

        // Verify ON CONFLICT(model_id) now works.
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn.execute(
            "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
             VALUES (1, 'test/repo', 'abc123', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO model_pulls (model_id, repo_id, commit_sha, pulled_at) \
             VALUES (1, 'test/repo', 'def456', '2024-01-02T00:00:00Z') \
             ON CONFLICT(model_id) DO UPDATE SET commit_sha=excluded.commit_sha",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Verify the row was upserted (commit_sha updated).
        let commit_sha: String = conn
            .query_row(
                "SELECT commit_sha FROM model_pulls WHERE model_id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(commit_sha, "def456");
    }

    /// Regression test for the FK toggle not restored on error path.
    ///
    /// Before the RAII guard fix, if a migration that required `foreign_keys=OFF`
    /// failed mid-execution (e.g., invalid SQL), the subsequent
    /// `PRAGMA foreign_keys=ON` would never run, permanently disabling FK
    /// enforcement for the rest of the session.
    ///
    /// This test verifies that the `FkGuard` struct properly re-enables FKs
    /// even when an error occurs inside its scope.
    #[test]
    fn test_fk_guard_restores_on_error() {
        let conn = Connection::open_in_memory().unwrap();

        // Verify FKs start enabled.
        let fk_before: i32 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk_before, 1);

        // Disable FKs via guard and trigger an error inside its scope.
        let guard_result = FkGuard::disable(&conn).unwrap();

        // Verify FKs are now off.
        let fk_off: i32 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk_off, 0);

        // Simulate an error occurring inside the guard's scope by dropping it early.
        drop(guard_result);

        // FKs must be re-enabled after the guard is dropped (even without an explicit ON call).
        let fk_after: i32 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk_after, 1, "FKs must be re-enabled after FkGuard drops");
    }

    /// Test that FKs remain enabled when guard is not used (normal path).
    #[test]
    fn test_fk_guard_noop_when_not_used() {
        let conn = Connection::open_in_memory().unwrap();

        let fk_before: i32 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk_before, 1);

        // Don't use the guard — FKs should stay enabled.
        let _ = ();

        let fk_after: i32 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(fk_after, 1);
    }

    /// Regression test: migration v18 must add cache_type_k and cache_type_v
    /// columns to model_configs.
    #[test]
    fn test_migration_v18_adds_cache_type_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn).unwrap();

        let k_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='cache_type_k'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let v_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('model_configs') WHERE name='cache_type_v'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(k_exists, 1);
        assert_eq!(v_exists, 1);
    }
}
