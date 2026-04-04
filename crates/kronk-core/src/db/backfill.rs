use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::config::Config;
use crate::models::registry::ModelRegistry;

/// Run the initial DB backfill for all installed models.
///
/// Scans model cards from the config/models directories, then fetches
/// commit SHAs and LFS hashes from HuggingFace for each model.
/// Prints progress to stdout.
///
/// This function is async because it makes network calls to HuggingFace.
pub async fn run_initial_backfill(conn: &Connection, config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir, configs_dir);

    let models = registry.scan()?;

    if models.is_empty() {
        println!("  No installed models found.");
        return Ok(());
    }

    let total = models.len();
    println!("  Backfilling database for {} installed model(s)...", total);

    for (i, model) in models.iter().enumerate() {
        let repo_id = &model.card.model.source;
        println!("  [{}/{}] {}...", i + 1, total, repo_id);

        // Fetch commit SHA from HuggingFace
        let listing = match crate::models::pull::list_gguf_files(repo_id).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Failed to fetch listing for {}: {}", repo_id, e);
                println!("    Failed to fetch metadata — skipping.");
                continue;
            }
        };

        // Upsert pull record with commit SHA
        if let Err(e) = crate::db::queries::upsert_model_pull(conn, repo_id, &listing.commit_sha) {
            tracing::warn!("Failed to upsert pull record for {}: {}", repo_id, e);
        }

        // Fetch blob metadata for LFS hashes (best-effort; proceed even on failure)
        let blobs = match crate::models::pull::fetch_blob_metadata(repo_id).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Failed to fetch blob metadata for {}: {}", repo_id, e);
                println!("    Failed to fetch blob metadata — continuing without LFS hashes.");
                std::collections::HashMap::new()
            }
        };

        // Upsert file records with LFS hashes (empty map means hashes will be None)
        for (filename, blob_info) in blobs {
            if let Err(e) = crate::db::queries::upsert_model_file(
                conn,
                repo_id,
                &filename,
                None,
                blob_info.lfs_sha256.as_deref(),
                blob_info.size,
            ) {
                tracing::warn!("Failed to upsert file record for {}: {}", filename, e);
            }
        }

        // Skip logging download for backfill - it's a batch operation
    }

    println!("  Database backfill complete.");
    Ok(())
}

/// Migrate existing `backend_registry.toml` into the `backend_installations` SQLite table.
///
/// If the file does not exist, returns `Ok(())` immediately.
/// After migrating all entries, renames the file to `backend_registry.toml.migrated`
/// so it is not re-imported on subsequent startups.
///
/// Duplicate `(name, version)` entries are handled by `INSERT OR REPLACE` — the old row
/// is deleted and re-inserted with a new `id`.
pub fn migrate_backend_registry_toml(
    conn: &Connection,
    config_dir: &std::path::Path,
) -> Result<()> {
    use crate::db::queries::{insert_backend_installation, BackendInstallationRecord};

    let registry_path = config_dir.join("backend_registry.toml");

    if !registry_path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(&registry_path)
        .with_context(|| format!("Failed to read {}", registry_path.display()))?;

    let registry_data: LegacyRegistryData = toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", registry_path.display()))?;

    let mut count = 0usize;

    for (name, info) in registry_data.backends {
        let gpu_type_json: Option<String> =
            match &info.gpu_type {
                Some(g) => Some(serde_json::to_string(g).with_context(|| {
                    format!("Failed to serialize gpu_type for backend '{}'", name)
                })?),
                None => None,
            };

        let source_json: Option<String> =
            match &info.source {
                Some(s) => Some(serde_json::to_string(s).with_context(|| {
                    format!("Failed to serialize source for backend '{}'", name)
                })?),
                None => None,
            };

        let record = BackendInstallationRecord {
            id: 0,
            name: name.clone(),
            backend_type: info.backend_type.to_string(),
            version: info.version.clone(),
            path: info.path.to_string_lossy().to_string(),
            installed_at: info.installed_at,
            gpu_type: gpu_type_json,
            source: source_json,
            is_active: true,
        };

        // INSERT OR REPLACE handles duplicate (name, version) by replacing the row
        insert_backend_installation(conn, &record)
            .with_context(|| format!("Failed to insert backend '{}' during migration", name))?;
        count += 1;
    }

    let migrated_path = config_dir.join("backend_registry.toml.migrated");
    std::fs::rename(&registry_path, &migrated_path).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            registry_path.display(),
            migrated_path.display()
        )
    })?;

    tracing::info!("Migrated {} backends from backend_registry.toml", count);

    Ok(())
}

// ---------------------------------------------------------------------------
// Private legacy deserialization structs (for one-time TOML migration only)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct LegacyRegistryData {
    #[serde(default)]
    backends: std::collections::HashMap<String, LegacyBackendInfo>,
}

#[derive(serde::Deserialize)]
struct LegacyBackendInfo {
    backend_type: crate::backends::BackendType,
    version: String,
    path: std::path::PathBuf,
    installed_at: i64,
    gpu_type: Option<crate::gpu::GpuType>,
    source: Option<crate::backends::BackendSource>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::open_in_memory;
    use crate::db::OpenResult;

    /// Test backfill with no models — should return Ok without error.
    #[tokio::test]
    async fn test_backfill_with_no_models() {
        let (_tmp, config) = setup_test_config();

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let result = run_initial_backfill(&conn, &config).await;

        assert!(result.is_ok());
    }

    fn setup_test_config() -> (tempfile::TempDir, Config) {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let configs = tmp.path().join("configs");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::create_dir_all(&configs).unwrap();

        let config = Config {
            loaded_from: Some(tmp.path().to_path_buf()),
            ..Default::default()
        };

        (tmp, config)
    }

    /// Test that migrate_backend_registry_toml correctly migrates a TOML file into the DB.
    #[test]
    fn test_migrate_backend_registry_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let registry_path = tmp.path().join("backend_registry.toml");

        // Write a minimal backend_registry.toml with one backend entry
        let toml_content = r#"
[backends.llama_cpp]
backend_type = "LlamaCpp"
version = "b3456"
path = "/opt/backends/llama_cpp/llama-server"
installed_at = 1700000000
"#;
        std::fs::write(&registry_path, toml_content).unwrap();

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Run the migration
        migrate_backend_registry_toml(&conn, tmp.path()).unwrap();

        // Assert that the backend was inserted correctly
        let record = crate::db::queries::get_active_backend(&conn, "llama_cpp")
            .unwrap()
            .expect("llama_cpp should exist in DB after migration");
        assert_eq!(record.version, "b3456");
        assert_eq!(record.name, "llama_cpp");

        // Assert the migrated file exists
        assert!(
            tmp.path().join("backend_registry.toml.migrated").exists(),
            "backend_registry.toml.migrated should exist"
        );

        // Assert the original file no longer exists
        assert!(
            !tmp.path().join("backend_registry.toml").exists(),
            "backend_registry.toml should have been renamed"
        );
    }

    /// Test that migrate_backend_registry_toml returns Ok when the file does not exist.
    #[test]
    fn test_migrate_backend_registry_toml_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Should return Ok without any error
        let result = migrate_backend_registry_toml(&conn, tmp.path());
        assert!(result.is_ok());
    }

    /// Test that a duplicate entry is skipped (not an error).
    #[test]
    fn test_migrate_backend_registry_toml_duplicate_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let registry_path = tmp.path().join("backend_registry.toml");

        let toml_content = r#"
[backends.llama_cpp]
backend_type = "LlamaCpp"
version = "b3456"
path = "/opt/backends/llama_cpp/llama-server"
installed_at = 1700000000
"#;
        std::fs::write(&registry_path, toml_content).unwrap();

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Pre-insert the same record (same name + version)
        crate::db::queries::insert_backend_installation(
            &conn,
            &crate::db::queries::BackendInstallationRecord {
                id: 0,
                name: "llama_cpp".to_string(),
                backend_type: "llama_cpp".to_string(),
                version: "b3456".to_string(),
                path: "/opt/backends/llama_cpp/llama-server".to_string(),
                installed_at: 1700000000,
                gpu_type: None,
                source: None,
                is_active: true,
            },
        )
        .unwrap();

        // Migration should succeed (duplicate is skipped, not an error)
        let result = migrate_backend_registry_toml(&conn, tmp.path());
        assert!(result.is_ok());

        // File should still be renamed
        assert!(tmp.path().join("backend_registry.toml.migrated").exists());
        assert!(!tmp.path().join("backend_registry.toml").exists());
    }
}
