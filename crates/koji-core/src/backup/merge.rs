//! Config and database merge logic for backup/restore.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;

/// RAII guard to ensure database is always detached.
struct DetachGuard<'a> {
    conn: &'a rusqlite::Connection,
    attached: bool,
}

impl<'a> DetachGuard<'a> {
    fn new(conn: &'a rusqlite::Connection) -> Self {
        Self {
            conn,
            attached: false,
        }
    }
    fn attach(&mut self, path: &str) -> Result<()> {
        self.conn
            .execute("ATTACH DATABASE ? AS backup_db", [path])
            .context("Failed to attach backup database")?;
        self.attached = true;
        Ok(())
    }
}

impl Drop for DetachGuard<'_> {
    fn drop(&mut self) {
        if self.attached {
            // Best effort detach - ignore errors since we're in Drop
            let _ = self.conn.execute("DETACH DATABASE backup_db", []);
        }
    }
}

/// Statistics from merging config.
#[derive(Debug, Default)]
pub struct MergeStats {
    pub new_backends: Vec<String>,
    pub new_sampling_templates: Vec<String>,
    pub skipped_backends: Vec<String>,
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

    // Ensure local directory exists
    std::fs::create_dir_all(local_configs_dir).with_context(|| {
        format!(
            "Failed to create local configs directory: {}",
            local_configs_dir.display()
        )
    })?;

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

    // Attach backup database with RAII guard to ensure cleanup
    let mut guard = DetachGuard::new(local_db);
    let backup_db_path_str = backup_db_path.to_string_lossy().to_string();
    guard.attach(&backup_db_path_str)?;

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

    // Guard will detach on drop
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
        // Models are now stored in DB, so we don't clear them from Config
        // local.models.clear();
        // backup.models.clear();

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

    #[test]
    fn test_merge_config_empty_backup() {
        let mut local = Config::default();
        local.backends.clear(); // Clear defaults for predictable test
        let mut backup = Config::default();
        backup.backends.clear(); // Also clear backup defaults

        let stats = merge_config(&mut local, &backup);

        assert!(stats.new_backends.is_empty());
        assert!(stats.skipped_backends.is_empty());
    }

    #[test]
    fn test_merge_config_empty_local() {
        let _local = Config::default();
        let mut backup = Config::default();
        backup.backends.insert(
            "new".to_string(),
            crate::config::BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );

        // This should work — local gets the backup's backends
        let mut local_mut = Config::default();
        let stats = merge_config(&mut local_mut, &backup);
        assert_eq!(stats.new_backends.len(), 1);
    }

    #[test]
    fn test_merge_config_multiple_new_backends() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.backends.clear();
        backup.backends.clear();

        for i in 1..=5 {
            backup.backends.insert(
                format!("backend{}", i),
                crate::config::BackendConfig {
                    path: None,
                    default_args: vec![],
                    health_check_url: None,
                    version: None,
                },
            );
        }

        let stats = merge_config(&mut local, &backup);
        assert_eq!(stats.new_backends.len(), 5);
        assert_eq!(local.backends.len(), 5);
    }

    #[test]
    fn test_merge_config_mixed_new_and_existing() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.backends.clear();
        backup.backends.clear();

        // Add some to local
        for i in 1..=3 {
            local.backends.insert(
                format!("local{}", i),
                crate::config::BackendConfig {
                    path: None,
                    default_args: vec![],
                    health_check_url: None,
                    version: None,
                },
            );
        }
        // Add some to backup (some overlapping with local)
        for i in 1..=5 {
            backup.backends.insert(
                format!("backend{}", i),
                crate::config::BackendConfig {
                    path: None,
                    default_args: vec![],
                    health_check_url: None,
                    version: None,
                },
            );
        }
        // Overlap: local1, local2 are in both (backup overrides)
        backup.backends.insert(
            "local1".to_string(),
            crate::config::BackendConfig {
                path: Some("/backup/path".to_string()),
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );
        backup.backends.insert(
            "local2".to_string(),
            crate::config::BackendConfig {
                path: Some("/backup/path".to_string()),
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );

        let stats = merge_config(&mut local, &backup);
        // New: backend1, backend2, backend3, backend4, backend5 (5 new)
        // Skipped: local1, local2 (2 skipped)
        assert_eq!(stats.new_backends.len(), 5);
        assert_eq!(stats.skipped_backends.len(), 2);
    }

    #[test]
    fn test_merge_config_sampling_templates() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.sampling_templates.clear();
        backup.sampling_templates.clear();

        let params = crate::profiles::SamplingParams {
            temperature: Some(0.7),
            top_k: Some(50),
            top_p: None,
            min_p: None,
            presence_penalty: None,
            frequency_penalty: None,
            repeat_penalty: None,
        };
        backup
            .sampling_templates
            .insert("coding".to_string(), params);

        let stats = merge_config(&mut local, &backup);
        assert_eq!(stats.new_sampling_templates.len(), 1);
        assert!(local.sampling_templates.contains_key("coding"));
    }

    #[test]
    fn test_merge_config_local_wins_for_sampling_templates() {
        let mut local = Config::default();
        let mut backup = Config::default();
        local.sampling_templates.clear();
        backup.sampling_templates.clear();

        // Local has a template with temperature 0.5
        local.sampling_templates.insert(
            "coding".to_string(),
            crate::profiles::SamplingParams {
                temperature: Some(0.5),
                top_k: None,
                top_p: None,
                min_p: None,
                presence_penalty: None,
                frequency_penalty: None,
                repeat_penalty: None,
            },
        );

        // Backup has a different template with same name (temperature 0.9)
        backup.sampling_templates.insert(
            "coding".to_string(),
            crate::profiles::SamplingParams {
                temperature: Some(0.9),
                top_k: None,
                top_p: None,
                min_p: None,
                presence_penalty: None,
                frequency_penalty: None,
                repeat_penalty: None,
            },
        );

        let stats = merge_config(&mut local, &backup);
        // Local should win — no new templates added
        assert!(stats.new_sampling_templates.is_empty());
        assert_eq!(local.sampling_templates["coding"].temperature, Some(0.5));
    }

    #[test]
    fn test_merge_stats_default() {
        let stats = MergeStats::default();
        assert!(stats.new_backends.is_empty());
        assert!(stats.new_sampling_templates.is_empty());
        assert!(stats.skipped_backends.is_empty());
    }

    #[test]
    fn test_merge_stats_debug() {
        let stats = MergeStats {
            new_backends: vec!["a".to_string()],
            new_sampling_templates: vec!["b".to_string()],
            skipped_backends: vec!["c".to_string()],
        };
        let debug_str = format!("{:?}", stats);
        assert!(debug_str.contains("new_backends"));
        assert!(debug_str.contains("skipped_backends"));
    }
}
