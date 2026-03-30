use crate::config::Config;
use crate::models::registry::ModelRegistry;
use anyhow::Result;
use rusqlite::Connection;

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
}
