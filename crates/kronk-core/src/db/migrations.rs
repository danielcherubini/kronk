//! Database migrations for SQLite
//!
//! Uses SQLite's `PRAGMA user_version` to track schema version.
//! Each migration runs in its own transaction.

use rusqlite::Connection;

/// Migration entry: (version number, SQL statement)
pub type Migration = (i32, &'static str);

/// Version number for the latest migration
pub const LATEST_VERSION: i32 = 1;

/// Run all applicable migrations on the database
///
/// Reads current `user_version`, applies any migrations with a higher version number.
/// Each individual migration runs in its own transaction. After each successful
/// migration, updates `user_version` to that migration's version.
pub fn run(conn: &Connection) -> anyhow::Result<()> {
    // Define all migrations in order.
    // Each tuple uses an explicit version literal (not the LATEST_VERSION constant)
    // so that adding a new migration never accidentally changes an existing version number.
    let migrations: &[Migration] = &[(
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
    )];

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
