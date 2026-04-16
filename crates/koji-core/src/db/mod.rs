//! Database module for SQLite
//!
//! Provides connection helpers and automatic migration system.

pub mod backfill;
pub mod migrations;
pub mod queries;

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
pub use rusqlite::Connection;

use crate::config::ModelConfig;

/// Result of opening a database connection
pub struct OpenResult {
    pub conn: Connection,
    pub needs_backfill: bool,
}

/// Convert a config key (double-dash format, e.g. `unsloth--gemma-4-26b-a4b-it-gguf`)
/// back to the original repo_id stored in the DB (e.g. `unsloth/gemma-4-26b-a4b-it-gguf`).
///
/// All external IDs (URLs, JSON responses, CLI args) use the double-dash format.
/// The DB stores the original HF repo_id with a real slash.
pub fn config_key_to_repo_id(config_key: &str) -> String {
    if let Some(idx) = config_key.find("--") {
        let (prefix, suffix) = config_key.split_at(idx);
        format!("{}/{}", prefix, &suffix[2..])
    } else {
        config_key.to_string()
    }
}

/// Load all model_configs rows and return them as a HashMap<config_key, ModelConfig>
/// where config_key = repo_id.to_lowercase().replace('/', "--").
///
/// NOTE: this is only used internally by the proxy to build its in-memory registry.
/// All external API lookups should use the integer `id` column directly.
pub fn load_model_configs(conn: &Connection) -> anyhow::Result<HashMap<String, ModelConfig>> {
    let records = queries::get_all_model_configs(conn)?;
    let mut configs = HashMap::new();

    for record in records {
        let config_key = record.repo_id.to_lowercase().replace('/', "--");
        let mut config = ModelConfig::from_db_record(&record);
        config.db_id = Some(record.id);

        // Populate quants from model_files table to restore them after restart
        let files = queries::get_model_files(conn, record.id)?;
        for file in files {
            let quant_key = file.quant.clone().unwrap_or_else(|| file.filename.clone());
            config.quants.insert(
                quant_key,
                crate::config::QuantEntry {
                    file: file.filename.clone(),
                    kind: crate::config::QuantKind::from_filename(&file.filename),
                    size_bytes: file.size_bytes.map(|s| s as u64),
                    context_length: None,
                },
            );
        }

        configs.insert(config_key, config);
    }

    Ok(configs)
}

/// Persist a single ModelConfig entry.
/// `config_key` is the HashMap key (double-dash, lowercased). The DB's
/// `repo_id` preserves the original HF repo case — taken from `mc.model`
/// when present (carries the exact repo_id the user entered), and only
/// falling back to deriving from `config_key` when `mc.model` is unset.
/// Returns the integer model id from the database.
pub fn save_model_config(
    conn: &Connection,
    config_key: &str,
    mc: &ModelConfig,
) -> anyhow::Result<i64> {
    let repo_id = mc
        .model
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| config_key_to_repo_id(config_key));
    let mut record = mc.to_db_record(&repo_id);
    // Default api_name to repo_id at save time so the DB always stores a
    // concrete value. `from_db_record` used to backfill this on load, which
    // meant unsaved rows, JSON exports, and direct DB queries saw NULL even
    // though the in-memory ModelConfig had a value.
    if record.api_name.as_deref().is_none_or(str::is_empty) {
        record.api_name = Some(repo_id.clone());
    }
    queries::upsert_model_config(conn, &record)
}

/// Open (or create) the SQLite database at `config_dir/koji.db`
///
/// Sets up the database with:
/// - WAL mode enabled
/// - Foreign keys enabled
/// - Migrations applied
///
/// Returns a connection and whether backfill is needed (true if DB was freshly created).
pub fn open(config_dir: &Path) -> anyhow::Result<OpenResult> {
    // Ensure the config directory exists before SQLite tries to create the file.
    std::fs::create_dir_all(config_dir)?;
    let db_path = config_dir.join("koji.db");
    let conn = Connection::open(&db_path)?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // Check user_version BEFORE running migrations to detect fresh DB
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let needs_backfill = current_version == 0;

    migrations::run(&conn)?;

    Ok(OpenResult {
        conn,
        needs_backfill,
    })
}

/// Backup the SQLite database at `config_dir/koji.db` to a destination path.
///
/// Uses SQLite's `VACUUM INTO` command to create a clean, consistent copy of
/// the database. This avoids copying WAL/SHM files and guarantees a consistent
/// snapshot even if the database is in use.
///
/// # Arguments
/// * `config_dir` - The koji config directory containing `koji.db`
/// * `dest` - Where to write the backup database file
///
/// # Returns
/// Result<()> indicating success or failure
pub fn backup_db(config_dir: &Path, dest: &Path) -> anyhow::Result<()> {
    // Compute safe parent path - avoid creating directory named after the file
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(parent).context("Failed to create parent directory for backup")?;

    let db_path = config_dir.join("koji.db");
    let conn = Connection::open(&db_path)?;

    // VACUUM INTO creates a clean copy without WAL/SHM files
    // Convert Path to string for rusqlite parameter binding
    let dest_str = dest.to_string_lossy().to_string();
    conn.execute("VACUUM INTO ?", [&dest_str])
        .context("Failed to vacuum database into destination")?;

    Ok(())
}

/// Open an in-memory SQLite database for testing.
///
/// Applies `PRAGMA foreign_keys=ON` (same as `open()`) and runs migrations.
/// Note: `journal_mode=WAL` is not applied because it is not supported for
/// in-memory databases.
pub fn open_in_memory() -> anyhow::Result<OpenResult> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // In-memory DB starts at version 0, so it needs backfill
    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let needs_backfill = current_version == 0;

    migrations::run(&conn)?;

    Ok(OpenResult {
        conn,
        needs_backfill,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Verify tables exist by querying sqlite_master
        let pulls_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='model_pulls'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pulls_count, 1);

        let files_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='model_files'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(files_count, 1);

        let log_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='download_log'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(log_count, 1);

        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='idx_download_log_repo'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn test_migrations_idempotent() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Run migrations twice - should not error
        migrations::run(&conn).unwrap();
        migrations::run(&conn).unwrap();
    }

    #[test]
    fn test_user_version_updated() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrations::LATEST_VERSION);
    }

    #[test]
    fn test_migration_v3_creates_backend_installations() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='backend_installations'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "backend_installations table should exist after migration v3"
        );

        // Verify index was created
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_backend_installations_name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            idx_count, 1,
            "idx_backend_installations_name index should exist after migration v3"
        );
    }

    /// Test that loading model configs from an empty DB returns an empty HashMap.
    #[test]
    fn test_load_model_configs_empty() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let configs = load_model_configs(&conn).unwrap();
        assert!(configs.is_empty());
    }

    /// Test saving and then loading a model config.
    #[test]
    fn test_save_and_load_model_config() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let mc = ModelConfig {
            backend: "llama.cpp".to_string(),
            display_name: Some("Test Model".to_string()),
            ..Default::default()
        };
        let config_key = "owner--repo".to_string();

        save_model_config(&conn, &config_key, &mc).unwrap();

        let configs = load_model_configs(&conn).unwrap();
        assert!(configs.contains_key(&config_key));
        let loaded = configs.get(&config_key).unwrap();
        assert_eq!(loaded.backend, mc.backend);
        assert_eq!(loaded.display_name, mc.display_name);
    }
}
