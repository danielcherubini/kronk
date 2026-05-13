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

        // Get or create the model_config entry to get the integer id
        let model_record = match crate::db::queries::get_model_config_by_repo_id(conn, repo_id)? {
            Some(r) => r,
            None => {
                // Create a placeholder model_config entry for this repo
                let mc = crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: None,
                    ..Default::default()
                };
                let config_key = repo_id.to_lowercase().replace('/', "--");
                let model_id = crate::db::save_model_config(conn, &config_key, &mc)?;
                crate::db::queries::get_model_config(conn, model_id)?
                    .expect("just-created model config should exist")
            }
        };

        // Upsert pull record with commit SHA
        if let Err(e) = crate::db::queries::upsert_model_pull(
            conn,
            model_record.id,
            repo_id,
            &listing.commit_sha,
        ) {
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
                model_record.id,
                repo_id,
                &filename,
                None,
                blob_info.lfs_sha256.as_deref(),
                blob_info.size,
            ) {
                tracing::warn!("Failed to upsert file record for {}: {}", filename, e);
            }
        }
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
            gpu_variant: "cpu".to_string(), // Legacy data has no gpu_variant; default to cpu
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

/// Migrate `[backends]` section from `config.toml` into the `backend_configs` SQLite table.
///
/// If the `[backends]` section does not exist or is empty, returns `Ok(0)` immediately.
/// After migrating all entries, the `[backends]` section is removed from `config.toml`
/// and the file is saved back to disk.
///
/// Returns the number of backend configs migrated.
pub fn migrate_backend_config_from_toml(
    conn: &Connection,
    config_dir: &std::path::Path,
) -> Result<usize> {
    use crate::db::queries::upsert_backend_config;

    let config_path = config_dir.join("config.toml");

    if !config_path.exists() {
        return Ok(0);
    }

    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;

    let config: crate::config::Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse {}", config_path.display()))?;

    // Nothing to migrate if backends section is empty
    if config.backends.is_empty() {
        return Ok(0);
    }

    let mut count = 0usize;

    for (name, backend_config) in &config.backends {
        let gpu_variant = backend_config
            .gpu_variant
            .clone()
            .unwrap_or_else(|| "cpu".to_string());

        upsert_backend_config(conn, name, &gpu_variant, &[], None).with_context(|| {
            format!(
                "Failed to insert backend config '{}' during migration",
                name
            )
        })?;
        count += 1;
    }

    // Remove the [backends] section from config.toml to prevent re-migration.
    // We rewrite the config with an empty backends map.
    let mut updated_config = config;
    updated_config.backends.clear();
    let new_content = toml::to_string_pretty(&updated_config)
        .with_context(|| "Failed to serialize updated config")?;
    std::fs::write(&config_path, &new_content)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    tracing::info!(
        "Migrated {} backend config(s) from config.toml [backends] section",
        count
    );

    Ok(count)
}

/// Repopulate `model_files` for any `model_configs` row that has zero
/// referencing files. Scans `<models_dir>/<repo_id>/` for `.gguf` files and
/// inserts one `model_files` row per file.
///
/// Exists to recover from the v9 migration FK-cascade bug, which silently
/// wiped every `model_files` row via `ON DELETE CASCADE` when the parent
/// table was rebuilt. For affected users, the files themselves are still on
/// disk — only the DB metadata is gone, and this function restores it.
///
/// Safe to call on every startup: a no-op for any `model_configs` row whose
/// `model_files` set is already populated.
///
/// Returns the number of `model_files` rows inserted.
pub fn repair_orphaned_model_files(
    conn: &Connection,
    models_dir: &std::path::Path,
) -> Result<usize> {
    use crate::config::QuantKind;
    use crate::db::queries::{get_all_model_configs, get_model_files, upsert_model_file};
    use crate::models::{pull::infer_quant_from_filename, repo_path};

    let records = get_all_model_configs(conn)?;
    let mut inserted = 0usize;

    for record in records {
        let existing = get_model_files(conn, record.id)?;
        if !existing.is_empty() {
            continue;
        }

        let repo_dir = repo_path(models_dir, &record.repo_id);
        let read_dir = match std::fs::read_dir(&repo_dir) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!(
                    repo_id = %record.repo_id,
                    dir = %repo_dir.display(),
                    error = %e,
                    "repair_orphaned_model_files: repo dir unreadable, skipping",
                );
                continue;
            }
        };

        let mut first_mmproj: Option<String> = None;

        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("gguf") {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };

            let kind = QuantKind::from_filename(filename);
            let quant = match kind {
                QuantKind::Mmproj => None,
                QuantKind::Model => infer_quant_from_filename(filename),
            };
            let size = std::fs::metadata(&path).ok().map(|m| m.len() as i64);

            if let Err(e) = upsert_model_file(
                conn,
                record.id,
                &record.repo_id,
                filename,
                quant.as_deref(),
                None,
                size,
            ) {
                tracing::warn!(
                    repo_id = %record.repo_id,
                    filename = %filename,
                    error = %e,
                    "repair_orphaned_model_files: upsert failed",
                );
                continue;
            }
            inserted += 1;

            if matches!(kind, QuantKind::Mmproj) && first_mmproj.is_none() {
                first_mmproj = Some(filename.to_string());
            }

            tracing::info!(
                repo_id = %record.repo_id,
                filename = %filename,
                "repair_orphaned_model_files: reinserted row",
            );
        }

        if record.selected_mmproj.is_none() {
            if let Some(mmproj) = first_mmproj {
                if let Err(e) = conn.execute(
                    "UPDATE model_configs SET selected_mmproj = ?1 WHERE id = ?2",
                    rusqlite::params![mmproj, record.id],
                ) {
                    tracing::warn!(
                        id = record.id,
                        error = %e,
                        "repair_orphaned_model_files: failed to set selected_mmproj",
                    );
                }
            }
        }
    }

    if inserted > 0 {
        tracing::info!(
            inserted,
            "repair_orphaned_model_files: restored {} model_files row(s) from disk",
            inserted,
        );
    }

    Ok(inserted)
}

/// Backfill HF metadata columns for existing models that have NULL values.
///
/// After migration v19 runs, existing model_configs rows have NULL for all 9
/// new columns. This function fetches metadata from the HuggingFace API for
/// each affected model and populates the columns.
///
/// Designed to run once on startup after migration, then be a no-op on
/// subsequent startups (no rows match `hf_format IS NULL`).
///
/// Failures for individual models are logged as warnings — the backfill
/// continues for remaining models even if some fail. A 200ms delay between
/// API calls avoids rate limiting.
///
/// Takes a `db_dir` path (not a `&Connection`) so it can be called from a
/// `tokio::spawn` task. Opens its own connection internally.
pub async fn backfill_hf_metadata(db_dir: &std::path::Path) -> Result<()> {
    // Open DB and read models needing backfill via spawn_blocking (keeps future Send)
    let db_dir_clone = db_dir.to_path_buf();
    let models: Vec<(i64, String)> =
        tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String)>> {
            let open_result = crate::db::open(&db_dir_clone)?;
            let conn = &open_result.conn;
            let models: Vec<(i64, String)> = conn
                .prepare("SELECT id, repo_id FROM model_configs WHERE hf_format IS NULL")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            // Connection dropped at end of closure
            Ok(models)
        })
        .await??;

    if models.is_empty() {
        tracing::debug!("No models need HF metadata backfill");
        return Ok(());
    }

    let total = models.len();
    tracing::info!("Backfilling HF metadata for {} model(s)", total);

    // ── Phase 1: Fetch metadata for all models (async) ──────────────────────
    let mut updates: Vec<(i64, String, crate::models::pull::HfModelMetadata)> = Vec::new();
    for (i, (model_id, repo_id)) in models.iter().enumerate() {
        tracing::info!(
            "[{}/{}] Fetching HF metadata for {}...",
            i + 1,
            total,
            repo_id
        );

        let meta = match crate::models::pull::fetch_hf_metadata(repo_id).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Failed to fetch HF metadata for '{}': {}", repo_id, e);
                continue;
            }
        };

        updates.push((*model_id, repo_id.clone(), meta));

        // Small delay between API calls to avoid rate limiting
        if i + 1 < total {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    // ── Phase 2: Write all updates in a single connection (sync) ─────────────
    if !updates.is_empty() {
        let db_dir_clone = db_dir.to_path_buf();
        let update_result = tokio::task::spawn_blocking(move || -> Result<()> {
            let open_result = crate::db::open(&db_dir_clone)?;
            for (mid, repo_id, meta) in updates {
                if let Err(e) = crate::models::update::update_model_config_hf_metadata(
                    &open_result.conn,
                    mid,
                    &meta,
                ) {
                    tracing::warn!(
                        "Failed to update HF metadata for '{}' (id={}): {}",
                        repo_id,
                        mid,
                        e
                    );
                }
            }
            Ok(())
        })
        .await?;

        if let Err(e) = update_result {
            tracing::warn!("HF metadata backfill DB write failed: {}", e);
        }
    }

    tracing::info!("HF metadata backfill complete");
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
        let record = crate::db::queries::get_active_backend(&conn, "llama_cpp", "cpu")
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
                gpu_variant: "cpu".to_string(),
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

    /// Simulates the state a user ends up in after the v9 FK-cascade bug:
    /// `model_configs` still has the row, the GGUF files are on disk, but
    /// `model_files` is empty. `repair_orphaned_model_files` must rebuild
    /// those rows and wire `selected_mmproj` for vision models.
    #[test]
    fn test_repair_orphaned_model_files_rebuilds_from_disk() {
        use crate::db::queries::{get_model_files, upsert_model_config, ModelConfigRecord};

        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        let repo_dir = models_dir.join("unsloth").join("Qwen3.6-35B-A3B-GGUF");
        std::fs::create_dir_all(&repo_dir).unwrap();

        std::fs::write(
            repo_dir.join("Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf"),
            b"fake-gguf-1",
        )
        .unwrap();
        std::fs::write(repo_dir.join("mmproj-F16.gguf"), b"fake-mmproj").unwrap();

        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let now = "2026-04-16T20:00:00Z".to_string();
        let record = ModelConfigRecord {
            id: 0,
            repo_id: "unsloth/Qwen3.6-35B-A3B-GGUF".to_string(),
            display_name: None,
            backend: "llama_cpp".to_string(),
            gpu_variant: None,
            enabled: true,
            selected_quant: Some("UD-Q4_K_XL".to_string()),
            selected_mmproj: None,
            context_length: None,
            num_parallel: Some(1),
            kv_unified: false,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let model_id = upsert_model_config(&conn, &record).unwrap();

        // Precondition: no model_files rows (the v9 cascade aftermath).
        assert!(get_model_files(&conn, model_id).unwrap().is_empty());

        let inserted = repair_orphaned_model_files(&conn, &models_dir).unwrap();
        assert_eq!(inserted, 2, "both gguf files must be reinserted");

        let files = get_model_files(&conn, model_id).unwrap();
        let mut filenames: Vec<_> = files.iter().map(|f| f.filename.as_str()).collect();
        filenames.sort();
        assert_eq!(
            filenames,
            vec!["Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf", "mmproj-F16.gguf"]
        );

        let main = files
            .iter()
            .find(|f| f.filename == "Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf")
            .unwrap();
        assert_eq!(main.quant.as_deref(), Some("UD-Q4_K_XL"));
        assert_eq!(main.size_bytes, Some(11)); // "fake-gguf-1" byte count

        // selected_mmproj must be set since the row had none and an mmproj
        // file was discovered.
        let selected_mmproj: Option<String> = conn
            .query_row(
                "SELECT selected_mmproj FROM model_configs WHERE id=?1",
                [model_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(selected_mmproj.as_deref(), Some("mmproj-F16.gguf"));

        // Second call is a no-op (rows already present).
        let again = repair_orphaned_model_files(&conn, &models_dir).unwrap();
        assert_eq!(again, 0, "repair must be idempotent");
    }

    /// If `selected_mmproj` is already set, the repair must not overwrite it.
    #[test]
    fn test_repair_preserves_existing_selected_mmproj() {
        use crate::db::queries::{upsert_model_config, ModelConfigRecord};

        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        let repo_dir = models_dir.join("u").join("r");
        std::fs::create_dir_all(&repo_dir).unwrap();
        std::fs::write(repo_dir.join("mmproj-F16.gguf"), b"x").unwrap();
        std::fs::write(repo_dir.join("model-Q4_K_M.gguf"), b"x").unwrap();

        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let now = "2026-04-16T20:00:00Z".to_string();
        let record = ModelConfigRecord {
            id: 0,
            repo_id: "u/r".to_string(),
            display_name: None,
            backend: "llama_cpp".to_string(),
            gpu_variant: None,
            enabled: true,
            selected_quant: Some("Q4_K_M".to_string()),
            selected_mmproj: Some("user-chosen.gguf".to_string()),
            context_length: None,
            num_parallel: Some(1),
            kv_unified: false,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let id = upsert_model_config(&conn, &record).unwrap();

        repair_orphaned_model_files(&conn, &models_dir).unwrap();

        let selected_mmproj: Option<String> = conn
            .query_row(
                "SELECT selected_mmproj FROM model_configs WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(selected_mmproj.as_deref(), Some("user-chosen.gguf"));
    }

    /// Test that backfill_hf_metadata runs without crashing when there are models
    /// with NULL hf_format. In tests, the HF API calls will fail (no network),
    /// but the function should handle failures gracefully and return Ok.
    #[tokio::test]
    async fn test_backfill_hf_metadata_no_crash_with_null_rows() {
        use crate::db::queries::{upsert_model_config, ModelConfigRecord};

        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();
        let now = "2026-05-03T00:00:00Z".to_string();

        // Insert a model_config row with NULL hf_format (simulating post-migration state)
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let record = ModelConfigRecord {
                id: 0,
                repo_id: "test/repo".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                context_length: None,
                num_parallel: Some(1),
                kv_unified: false,
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                hf_format: None,
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                created_at: now.clone(),
                updated_at: now,
            };
            upsert_model_config(&conn, &record).unwrap();

            // Verify the row exists with NULL hf_format
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM model_configs WHERE hf_format IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }

        // Run backfill — HF API calls will fail (no network in tests),
        // but the function should handle failures gracefully and return Ok
        let result = backfill_hf_metadata(&db_dir).await;
        assert!(
            result.is_ok(),
            "backfill should not crash even when HF API fails"
        );

        // hf_format will still be NULL since the fetch failed (expected in tests)
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM model_configs WHERE hf_format IS NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "hf_format should still be NULL after failed fetch"
            );
        }
    }

    /// Test that backfill_hf_metadata is a no-op when all models already have hf_format.
    #[tokio::test]
    async fn test_backfill_hf_metadata_noop_when_all_populated() {
        use crate::db::queries::{upsert_model_config, ModelConfigRecord};

        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();
        let now = "2026-05-03T00:00:00Z".to_string();

        // Insert a model_config row with hf_format already set
        {
            let OpenResult { conn, .. } = crate::db::open(&db_dir).unwrap();
            let record = ModelConfigRecord {
                id: 0,
                repo_id: "test/repo".to_string(),
                display_name: None,
                backend: "llama_cpp".to_string(),
                gpu_variant: None,
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                context_length: None,
                num_parallel: Some(1),
                kv_unified: false,
                gpu_layers: None,
                cache_type_k: None,
                cache_type_v: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                hf_format: Some("gguf".to_string()),
                hf_base_model: None,
                hf_pipeline_tag: None,
                hf_total_params: None,
                hf_active_params: None,
                hf_architecture_type: None,
                hf_context_length: None,
                hf_num_layers: None,
                hf_last_modified: None,
                created_at: now.clone(),
                updated_at: now,
            };
            upsert_model_config(&conn, &record).unwrap();
        }

        // Run backfill — should be a no-op (no models need backfill)
        let result = backfill_hf_metadata(&db_dir).await;
        assert!(result.is_ok());
    }

    /// Test that backfill_hf_metadata returns Ok with an empty DB.
    #[tokio::test]
    async fn test_backfill_hf_metadata_empty_db() {
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().to_path_buf();

        // Create the DB (empty, just migrations)
        let OpenResult { .. } = crate::db::open(&db_dir).unwrap();

        let result = backfill_hf_metadata(&db_dir).await;
        assert!(result.is_ok());
    }

    /// Test that migrate_backend_config_from_toml correctly migrates [backends] from config.toml.
    #[test]
    fn test_migrate_backend_config_from_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");

        // Write a config.toml with [backends] section
        // Note: default_args and health_check_url are no longer in TOML BackendConfig;
        // they are stored in the DB instead. Old TOML files with these fields will
        // deserialize fine (unknown fields are ignored), but the migration will not
        // copy them since they are no longer on the struct.
        let toml_content = r#"
[general]
log_level = "info"

[backends.llama_cpp]
gpu_variant = "cpu"

[backends.ik_llama]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Run the migration
        let count = migrate_backend_config_from_toml(&conn, tmp.path()).unwrap();
        assert_eq!(count, 2);

        // Verify llama_cpp config
        let llama = crate::db::queries::get_backend_config(&conn, "llama_cpp", "cpu")
            .unwrap()
            .expect("llama_cpp should exist in DB after migration");
        assert_eq!(llama.name, "llama_cpp");
        assert_eq!(llama.gpu_variant, "cpu");
        // default_args and health_check_url are no longer copied from TOML
        assert!(llama.default_args.is_empty());
        assert!(llama.health_check_url.is_none());

        // Verify ik_llama config (no gpu_variant specified, defaults to cpu)
        let ik = crate::db::queries::get_backend_config(&conn, "ik_llama", "cpu")
            .unwrap()
            .expect("ik_llama should exist in DB after migration");
        assert_eq!(ik.name, "ik_llama");
        assert_eq!(ik.gpu_variant, "cpu");
        assert!(ik.default_args.is_empty());
        assert!(ik.health_check_url.is_none());

        // Verify [backends] section was cleared from config.toml
        let after = std::fs::read_to_string(&config_path).unwrap();
        let reloaded: crate::config::Config = toml::from_str(&after).unwrap();
        assert!(
            reloaded.backends.is_empty(),
            "backends section should be cleared after migration"
        );
    }

    /// Test that migrate_backend_config_from_toml returns Ok(0) when no config file exists.
    #[test]
    fn test_migrate_backend_config_from_toml_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let OpenResult { conn, .. } = open_in_memory().unwrap();

        let count = migrate_backend_config_from_toml(&conn, tmp.path()).unwrap();
        assert_eq!(count, 0);
    }

    /// Test that migrate_backend_config_from_toml is idempotent (no-op on second call).
    #[test]
    fn test_migrate_backend_config_from_toml_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");

        let toml_content = r#"
[general]
log_level = "info"

[backends.llama_cpp]
default_args = ["-fa 1"]
"#;
        std::fs::write(&config_path, toml_content).unwrap();

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // First call migrates
        let count1 = migrate_backend_config_from_toml(&conn, tmp.path()).unwrap();
        assert_eq!(count1, 1);

        // Second call is a no-op (backends cleared)
        let count2 = migrate_backend_config_from_toml(&conn, tmp.path()).unwrap();
        assert_eq!(count2, 0);
    }
}
