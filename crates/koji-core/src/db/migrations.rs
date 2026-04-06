//! Database migrations for SQLite
//!
//! Uses SQLite's `PRAGMA user_version` to track schema version.
//! Each migration runs in its own transaction.

use rusqlite::Connection;

/// Migration entry: (version number, SQL statement)
pub type Migration = (i32, &'static str);

/// Version number for the latest migration
pub const LATEST_VERSION: i32 = 4;

/// Run all applicable migrations on the database
///
/// Reads current `user_version`, applies any migrations with a higher version number.
/// Each individual migration runs in its own transaction. After each successful
/// migration, updates `user_version` to that migration's version.
pub fn run(conn: &Connection) -> anyhow::Result<()> {
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
    ];

    let current_version: i32 =
        conn.pragma_query_value::<i32, _>(None, "user_version", |row| row.get(0))?;

    for (version, sql) in migrations {
        if *version > current_version {
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
