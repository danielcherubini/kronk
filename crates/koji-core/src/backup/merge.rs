//! Config and database merge logic for backup/restore.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;

/// Statistics from merging config.
#[derive(Debug, Default)]
pub struct MergeStats {
    pub new_backends: Vec<String>,
    pub new_models: Vec<String>,
    pub new_sampling_templates: Vec<String>,
    pub skipped_backends: Vec<String>,
    pub skipped_models: Vec<String>,
}

/// Merge a backup config into a local config.
///
/// - New backends/models are added
/// - Existing local values are preserved (local wins)
/// - Sampling templates are merged (local wins)
///
/// Returns statistics about what was added vs skipped.
pub fn merge_config(local: &mut Config, backup: &Config) -> MergeStats {
    let mut stats = MergeStats::default();

    // Merge backends
    for (name, backend) in &backup.backends {
        if local.backends.contains_key(name) {
            stats.skipped_backends.push(name.clone());
        } else {
            local.backends.insert(name.clone(), backend.clone());
            stats.new_backends.push(name.clone());
        }
    }

    // Merge models
    for (name, model) in &backup.models {
        if local.models.contains_key(name) {
            stats.skipped_models.push(name.clone());
        } else {
            local.models.insert(name.clone(), model.clone());
            stats.new_models.push(name.clone());
        }
    }

    // Merge sampling templates (local wins)
    for (name, template) in &backup.sampling_templates {
        if !local.sampling_templates.contains_key(name) {
            local
                .sampling_templates
                .insert(name.clone(), template.clone());
            stats.new_sampling_templates.push(name.clone());
        }
    }

    stats
}

/// Merge model card TOML files from backup to local.
///
/// Copies any card that doesn't exist locally.
pub fn merge_model_cards(
    local_configs_dir: &Path,
    backup_configs_dir: &Path,
) -> Result<Vec<String>> {
    let mut copied = Vec::new();

    if !backup_configs_dir.exists() {
        return Ok(copied);
    }

    for entry in std::fs::read_dir(backup_configs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let filename = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let local_path = local_configs_dir.join(&filename);
            if !local_path.exists() {
                std::fs::copy(&path, &local_path)
                    .with_context(|| format!("Failed to copy card: {}", filename))?;
                copied.push(filename);
            }
        }
    }

    Ok(copied)
}

/// Statistics from merging database.
#[derive(Debug, Default)]
pub struct DbMergeStats {
    pub new_model_pulls: u32,
    pub new_model_files: u32,
    pub new_backend_installations: u32,
}

/// Merge database records from backup to local.
///
/// Uses `INSERT OR IGNORE` to skip existing records.
/// Only merges essential tables (model_pulls, model_files, backend_installations).
/// Ephemeral tables (active_models, download_log, system_metrics_history) are skipped.
pub fn merge_database(
    local_db: &rusqlite::Connection,
    backup_db_path: &Path,
) -> Result<DbMergeStats> {

    let mut stats = DbMergeStats::default();

    // Attach backup database
    let backup_db_path_str = backup_db_path.to_string_lossy().to_string();
    local_db
        .execute("ATTACH DATABASE ? AS backup_db", [&backup_db_path_str])
        .context("Failed to attach backup database")?;

    // Merge model_pulls (explicit column list, no id)
    let before = count_model_pulls(local_db)?;
    local_db
        .execute_batch(
            "INSERT OR IGNORE INTO model_pulls (repo_id, commit_sha, pulled_at) \
         SELECT repo_id, commit_sha, pulled_at FROM backup_db.model_pulls",
        )
        .context("Failed to merge model_pulls")?;
    let after = count_model_pulls(local_db)?;
    stats.new_model_pulls = after.saturating_sub(before);

    // Merge model_files (explicit column list, no id)
    let before = count_model_files(local_db)?;
    local_db
        .execute_batch(
            "INSERT OR IGNORE INTO model_files \
         (repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at, \
          last_verified_at, verified_ok, verify_error) \
         SELECT repo_id, filename, quant, lfs_oid, size_bytes, downloaded_at, \
                last_verified_at, verified_ok, verify_error \
         FROM backup_db.model_files",
        )
        .context("Failed to merge model_files")?;
    let after = count_model_files(local_db)?;
    stats.new_model_files = after.saturating_sub(before);

    // Merge backend_installations (explicit column list, no id)
    let before = count_backend_installations(local_db)?;
    local_db
        .execute_batch(
            "INSERT OR IGNORE INTO backend_installations \
         (name, backend_type, version, path, installed_at, gpu_type, source, is_active) \
         SELECT name, backend_type, version, path, installed_at, gpu_type, source, is_active \
         FROM backup_db.backend_installations",
        )
        .context("Failed to merge backend_installations")?;
    let after = count_backend_installations(local_db)?;
    stats.new_backend_installations = after.saturating_sub(before);

    // Detach backup database
    local_db
        .execute("DETACH DATABASE backup_db", [])
        .context("Failed to detach backup database")?;

    Ok(stats)
}

fn count_model_pulls(conn: &rusqlite::Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM model_pulls", [], |row| row.get(0))?)
}

fn count_model_files(conn: &rusqlite::Connection) -> Result<u32> {
    Ok(conn.query_row("SELECT COUNT(*) FROM model_files", [], |row| row.get(0))?)
}

fn count_backend_installations(conn: &rusqlite::Connection) -> Result<u32> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM backend_installations", [], |row| {
            row.get(0)
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_config_adds_new_backends() {
        let mut local = Config::default();
        let mut backup = Config::default();

        // Add a new backend to backup
        backup.backends.insert(
            "new_backend".to_string(),
            crate::config::BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: Some("http://test/health".to_string()),
                version: None,
            },
        );

        let stats = merge_config(&mut local, &backup);

        assert_eq!(stats.new_backends.len(), 1);
        assert_eq!(stats.new_backends[0], "new_backend");
        assert!(local.backends.contains_key("new_backend"));
    }

    #[test]
    fn test_merge_config_preserves_local() {
        let mut local = Config::default();
        let mut backup = Config::default();

        // Clear defaults to make test predictable
        local.backends.clear();
        backup.backends.clear();
        local.models.clear();
        backup.models.clear();

        // Add a backend to local
        local.backends.insert(
            "existing".to_string(),
            crate::config::BackendConfig {
                path: Some("/local/path".to_string()),
                default_args: vec!["--local".to_string()],
                health_check_url: None,
                version: None,
            },
        );

        // Try to add same backend to backup with different value
        backup.backends.insert(
            "existing".to_string(),
            crate::config::BackendConfig {
                path: Some("/backup/path".to_string()),
                default_args: vec!["--backup".to_string()],
                health_check_url: Some("http://backup/health".to_string()),
                version: None,
            },
        );

        let stats = merge_config(&mut local, &backup);

        assert_eq!(stats.skipped_backends.len(), 1);
        assert!(stats.skipped_backends.contains(&"existing".to_string()));
        // Local value should be preserved
        assert_eq!(
            local.backends["existing"].path,
            Some("/local/path".to_string())
        );
    }
}
