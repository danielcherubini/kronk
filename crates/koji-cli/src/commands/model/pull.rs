use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
use koji_core::models::pull;
use koji_core::models::{ModelCard, ModelMeta, QuantInfo};
use reqwest::Client;

pub(super) async fn cmd_pull(config: &Config, repo_id: &str) -> Result<()> {
    println!("Pulling model...");
    println!();
    println!("  Fetching file list from {}...", repo_id);

    let listing = pull::list_gguf_files(repo_id).await?;

    if listing.repo_id != repo_id {
        println!("  Resolved to: {}", listing.repo_id);
    }

    // Use the resolved repo_id for all subsequent operations
    let repo_id = &listing.repo_id;
    let ggufs = &listing.files;

    let options: Vec<String> = ggufs
        .iter()
        .map(|g| {
            let quant_label = g.quant.as_deref().unwrap_or("unknown");
            format!("{} ({})", g.filename, quant_label)
        })
        .collect();

    let selected =
        inquire::MultiSelect::new("Which quants do you want to download?", options.clone())
            .with_help_message("Space to select, Enter to confirm")
            .prompt()
            .context("Interactive selection cancelled")?;

    if selected.is_empty() {
        println!("No files selected. Nothing to do.");
        return Ok(());
    }

    let model_id = repo_id.to_string();
    let models_dir_pathbuf = config.models_dir()?.to_path_buf();
    let mut model_dir: PathBuf = models_dir_pathbuf.clone();
    for part in repo_id.split('/') {
        model_dir.push(part);
    }
    std::fs::create_dir_all(&model_dir)
        .with_context(|| format!("Failed to create directory: {}", model_dir.display()))?;

    let configs_dir = config.configs_dir()?;
    std::fs::create_dir_all(&configs_dir)?;
    let card_filename = format!("{}.toml", model_id.replace('/', "--"));
    let card_path = configs_dir.join(&card_filename);
    let mut card = if card_path.exists() {
        ModelCard::load(&card_path)?
    } else {
        // Try to fetch a community model card with curated sampling presets
        println!("  Checking for community model card...");
        if let Some(mut community_card) = pull::fetch_community_card(repo_id).await {
            println!(
                "  Found community card for {} (context: {}, gpu-layers: {})",
                community_card.model.name,
                community_card
                    .model
                    .default_context_length
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "default".to_string()),
                community_card
                    .model
                    .default_gpu_layers
                    .map(|g| g.to_string())
                    .unwrap_or_else(|| "default".to_string()),
            );
            if !community_card.sampling.is_empty() {
                let presets: Vec<&str> =
                    community_card.sampling.keys().map(|s| s.as_str()).collect();
                println!("  Sampling presets: {}", presets.join(", "));
            }
            // Ensure source matches the repo we're pulling from
            community_card.model.source = repo_id.to_string();
            // Clear quants — we'll populate from what the user actually downloads
            community_card.quants.clear();
            community_card
        } else {
            let mut new_card = ModelCard {
                model: ModelMeta {
                    name: repo_id.to_string(),
                    source: repo_id.to_string(),
                    default_context_length: None, // set by interactive context prompt
                    default_gpu_layers: Some(999),
                },
                sampling: HashMap::new(),
                quants: HashMap::new(),
            };
            // Seed sampling from config's sampling_templates
            new_card.populate_sampling_from(&config.sampling_templates);
            new_card
        }
    };

    // Track successful downloads for DB recording
    struct DownloadedFile {
        filename: String,
        quant: Option<String>,
        size_bytes: u64,
    }
    let mut downloaded_files: Vec<DownloadedFile> = Vec::new();

    for display_str in &selected {
        let idx = options.iter().position(|o| o == display_str).unwrap();
        let gguf = &ggufs[idx];

        println!();
        println!("  Downloading {}...", gguf.filename);

        let client = Client::new();
        let result = pull::download_gguf(&client, repo_id, &gguf.filename, &model_dir).await?;

        let base_quant = gguf.quant.clone().unwrap_or_else(|| gguf.filename.clone());
        let quant_key = super::utils::unique_quant_key(&card.quants, &base_quant, &gguf.filename);

        card.quants.insert(
            quant_key,
            QuantInfo {
                file: gguf.filename.clone(),
                kind: koji_core::config::QuantKind::from_filename(&gguf.filename),
                size_bytes: Some(result.size_bytes),
                context_length: None,
            },
        );

        downloaded_files.push(DownloadedFile {
            filename: gguf.filename.clone(),
            quant: gguf.quant.clone(),
            size_bytes: result.size_bytes,
        });

        println!("  Downloaded: {}", result.path.display());
    }

    // Suggest context sizes based on VRAM and model size
    let largest_model_bytes = card
        .quants
        .values()
        .filter_map(|q| q.size_bytes)
        .max()
        .unwrap_or(0);

    let vram = koji_core::gpu::query_vram();

    let selected_ctx = if largest_model_bytes > 0 {
        let suggestions = koji_core::gpu::suggest_context_sizes(largest_model_bytes, vram.as_ref());

        if let Some(ref v) = vram {
            println!();
            println!(
                "  GPU: {} MiB total, {} MiB available",
                v.total_mib,
                v.available_mib()
            );
            println!(
                "  Model: ~{:.1} GiB",
                largest_model_bytes as f64 / 1_073_741_824.0
            );
        }

        let options: Vec<String> = suggestions.iter().map(|s| s.label.clone()).collect();

        // Default to the largest context that fits
        let default_idx = suggestions.iter().rposition(|s| s.fits).unwrap_or(2);

        let selected_label = inquire::Select::new("Select context size:", options)
            .with_starting_cursor(default_idx)
            .with_help_message("Based on your GPU VRAM and model size")
            .prompt()
            .context("Context selection cancelled")?;

        suggestions
            .iter()
            .find(|s| s.label == selected_label)
            .map(|s| s.context_length)
            .unwrap_or(8192)
    } else {
        // No file size info — show plain presets
        println!();
        let presets = vec![
            "2048 (2K — minimal)".to_string(),
            "4096 (4K — small)".to_string(),
            "8192 (8K — standard)".to_string(),
            "16384 (16K)".to_string(),
            "32768 (32K)".to_string(),
            "65536 (64K)".to_string(),
            "100000 (100K)".to_string(),
            "131072 (128K — max for most models)".to_string(),
        ];

        let selected = inquire::Select::new("Select context size:", presets)
            .with_starting_cursor(2) // default to 8K
            .with_help_message("Could not detect model size — choose based on your GPU")
            .prompt()
            .context("Context selection cancelled")?;

        // Parse the number from the start of the string
        selected
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(8192)
    };

    // Apply context length to all downloaded quants
    card.model.default_context_length = Some(selected_ctx);
    for quant in card.quants.values_mut() {
        quant.context_length = Some(selected_ctx);
    }

    println!("  Context: {} tokens", selected_ctx);

    // Fetch LFS blob metadata (async, before opening DB)
    let blobs = match pull::fetch_blob_metadata(repo_id).await {
        Ok(b) => Some(b),
        Err(e) => {
            tracing::warn!("Failed to fetch blob metadata (non-fatal): {}", e);
            None
        }
    };

    // Record all pull metadata in DB (sync, single connection, after all async work)
    let db_record_result: anyhow::Result<()> = (|| {
        let db_dir = koji_core::config::Config::config_dir()?;
        let OpenResult {
            conn,
            needs_backfill: _,
        } = koji_core::db::open(&db_dir)?;

        // Look up model_id before DB writes
        let model_id = match koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)? {
            Some(r) => r.id,
            None => anyhow::bail!("Model not found in DB: {}", repo_id),
        };

        koji_core::db::queries::upsert_model_pull(&conn, model_id, repo_id, &listing.commit_sha)?;

        let now = super::utils::manual_timestamp();
        for df in &downloaded_files {
            let lfs_sha = blobs
                .as_ref()
                .and_then(|b| b.get(&df.filename))
                .and_then(|b| b.lfs_sha256.as_deref());
            let size = blobs
                .as_ref()
                .and_then(|b| b.get(&df.filename))
                .and_then(|b| b.size)
                .unwrap_or(df.size_bytes as i64);

            koji_core::db::queries::upsert_model_file(
                &conn,
                model_id,
                repo_id,
                &df.filename,
                df.quant.as_deref(),
                lfs_sha,
                Some(size),
            )?;
            // started_at / completed_at are best-effort approximations: individual
            // download timings are not tracked at this level; both fields receive
            // the same timestamp captured before writing to DB.
            koji_core::db::queries::log_download(
                &conn,
                &koji_core::db::queries::DownloadLogEntry {
                    repo_id: repo_id.to_string(),
                    filename: df.filename.clone(),
                    started_at: now.clone(),
                    completed_at: Some(now.clone()),
                    size_bytes: Some(df.size_bytes as i64),
                    duration_ms: None,
                    success: true,
                    error_message: None,
                },
            )?;
        }
        Ok(())
    })();
    if let Err(e) = db_record_result {
        tracing::warn!("Failed to record pull metadata in DB (non-fatal): {}", e);
    }

    card.save(&card_path)?;

    println!();
    println!("Done.");
    println!("  Model card saved: {}", card_path.display());
    println!();
    println!("  Create a model config:");
    for quant_key in card.quants.keys() {
        println!(
            "    koji model create --model {} --quant {} --profile coding --name my-server",
            model_id, quant_key
        );
    }

    Ok(())
}

pub(crate) fn cmd_scan(config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;

    // Use the directory the config was loaded from as the base for the DB.
    // This ensures that in tests (and Windows services), we use the temporary/specified
    // directory instead of the default system config path.
    let db_dir = match config.loaded_from {
        Some(ref p) => p.clone(),
        None => koji_core::config::Config::config_dir()
            .map_err(|e| anyhow::anyhow!("Failed to determine config directory: {e}"))?,
    };

    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;

    let mut added_files = 0;
    let mut removed_files = 0;
    let mut removed_configs = 0;

    // 1. Walk filesystem for .gguf files and reconcile with DB
    fn walk_and_reconcile(
        dir: &std::path::Path,
        base_dir: &std::path::Path,
        conn: &koji_core::db::Connection,
        _added: &mut usize,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                walk_and_reconcile(&path, base_dir, conn, _added)?;
            } else if path.extension().is_some_and(|e| e == "gguf") {
                let relative_path = path.strip_prefix(base_dir)?;
                if let Some(repo_id_path) = relative_path.parent() {
                    let repo_id = repo_id_path
                        .to_string_lossy()
                        .to_string()
                        .replace(std::path::MAIN_SEPARATOR, "/");
                    let filename = path.file_name().unwrap().to_string_lossy().to_string();

                    // Get or create model_id
                    let model_id = match koji_core::db::queries::get_model_config_by_repo_id(
                        conn, &repo_id,
                    )? {
                        Some(r) => r.id,
                        None => {
                            let record = koji_core::db::queries::ModelConfigRecord {
                                id: 0,
                                repo_id: repo_id.clone(),
                                display_name: None,
                                backend: "llama.cpp".to_string(),
                                enabled: true,
                                selected_quant: None,
                                selected_mmproj: None,
                                context_length: None,
                                gpu_layers: None,
                                port: None,
                                args: None,
                                sampling: None,
                                modalities: None,
                                profile: None,
                                api_name: None,
                                health_check: None,
                                num_parallel: None,
                                created_at: super::utils::manual_timestamp(),
                                updated_at: super::utils::manual_timestamp(),
                            };
                            koji_core::db::queries::upsert_model_config(conn, &record)?;
                            koji_core::db::queries::get_model_config_by_repo_id(conn, &repo_id)?
                                .unwrap()
                                .id
                        }
                    };

                    let files = koji_core::db::queries::get_model_files(conn, model_id)?;
                    if !files.iter().any(|f| f.filename == filename) {
                        koji_core::db::queries::upsert_model_file(
                            conn, model_id, &repo_id, &filename, None, None, None,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    walk_and_reconcile(&models_dir, &models_dir, &conn, &mut added_files)?;

    // 2. Remove files from DB that are missing on disk
    let all_files = koji_core::db::queries::get_all_model_files(&conn)?;
    let mut repos_to_check = std::collections::HashSet::new();
    for file in all_files {
        let repo_id = file.repo_id;
        let filename = file.filename;
        let path = koji_core::models::repo_path(&models_dir, &repo_id).join(&filename);
        if !path.exists() {
            koji_core::db::queries::delete_model_file(&conn, file.model_id, &filename)?;
            removed_files += 1;
            repos_to_check.insert(repo_id);
        }
    }

    // 3. Remove configs whose directory doesn't exist or is empty
    let all_configs = koji_core::db::queries::get_all_model_configs(&conn)?;
    for config in all_configs {
        let repo_id = config.repo_id;
        let model_dir = koji_core::models::repo_path(&models_dir, &repo_id);
        if !model_dir.exists()
            || model_dir
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(true)
        {
            koji_core::db::queries::delete_model_config(&conn, config.id)?;
            removed_configs += 1;
        }
    }

    println!("Scan complete:");
    println!("  Added:   {} files", added_files);
    println!("  Removed: {} files", removed_files);
    println!("  Removed: {} ghost configs", removed_configs);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use koji_core::config::Config;
    use koji_core::db::queries::{
        get_all_model_configs, get_model_files, upsert_model_config, upsert_model_file,
        ModelConfigRecord,
    };
    use koji_core::db::{open, OpenResult};
    use std::fs;
    use tempfile::tempdir;

    pub(super) async fn setup_test_env() -> (tempfile::TempDir, Config, OpenResult) {
        let dir = tempdir().unwrap();
        let config = Config {
            loaded_from: Some(dir.path().to_path_buf()),
            ..Default::default()
        };

        // Create models dir
        let models_dir = config.models_dir().unwrap();
        fs::create_dir_all(&models_dir).unwrap();

        // Create configs dir
        let configs_dir = config.configs_dir().unwrap();
        fs::create_dir_all(&configs_dir).unwrap();

        let open_res = open(dir.path()).unwrap();

        (dir, config, open_res)
    }

    #[tokio::test]
    async fn test_scan_adds_new_files() {
        let (_dir, config, open_res) = setup_test_env().await;
        let conn = &open_res.conn;

        // Create a model file on disk
        let repo_id = "test/model";
        let models_dir = config.models_dir().unwrap();
        let model_dir = models_dir.join(repo_id);
        fs::create_dir_all(&model_dir).unwrap();
        let filename = "model.gguf";
        fs::write(model_dir.join(filename), "dummy data").unwrap();

        // Run scan
        cmd_scan(&config).unwrap();

        // Verify it was added to DB
        let configs = koji_core::db::queries::get_all_model_configs(conn).unwrap();
        let model_id = configs.iter().find(|c| c.repo_id == repo_id).unwrap().id;
        let files = koji_core::db::queries::get_model_files(conn, model_id).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, filename);

        let configs = koji_core::db::queries::get_all_model_configs(conn).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].repo_id, repo_id);
    }

    #[tokio::test]
    async fn test_scan_removes_missing_files() {
        let (_dir, config, open_res) = setup_test_env().await;
        let conn = &open_res.conn;

        let repo_id = "test/model";
        let filename = "missing.gguf";

        // Add to DB but NOT on disk
        // First create the model config
        let record = ModelConfigRecord {
            id: 0,
            repo_id: repo_id.to_string(),
            display_name: None,
            backend: "llama.cpp".to_string(),
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            context_length: None,
            gpu_layers: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            num_parallel: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        upsert_model_config(conn, &record).unwrap();
        let configs = get_all_model_configs(conn).unwrap();
        let model_id = configs.iter().find(|c| c.repo_id == repo_id).unwrap().id;
        upsert_model_file(
            conn,
            model_id,
            repo_id,
            filename,
            Some("Q4"),
            None,
            Some(100),
        )
        .unwrap();

        // Run scan
        cmd_scan(&config).unwrap();

        // Verify it was removed from DB
        let files = get_model_files(conn, model_id).unwrap();
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn test_scan_removes_ghost_configs() {
        let (_dir, config, open_res) = setup_test_env().await;
        let conn = &open_res.conn;

        let repo_id = "ghost/model";
        let record = ModelConfigRecord {
            id: 1,
            repo_id: repo_id.to_string(),
            display_name: None,
            backend: "llama.cpp".to_string(),
            enabled: true,
            selected_quant: None,
            selected_mmproj: None,
            context_length: None,
            gpu_layers: None,
            port: None,
            args: None,
            sampling: None,
            modalities: None,
            profile: None,
            api_name: None,
            health_check: None,
            num_parallel: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        upsert_model_config(conn, &record).unwrap();

        // No directory on disk

        // Run scan
        cmd_scan(&config).unwrap();

        // Verify config was removed
        let configs = koji_core::db::queries::get_all_model_configs(conn).unwrap();
        assert!(configs.is_empty());
    }

    #[tokio::test]
    async fn test_scan_empty_dir_removes_everything() {
        let (_dir, config, open_res) = setup_test_env().await;
        let conn = &open_res.conn;

        // Populate DB with some garbage (let DB assign IDs via AUTOINCREMENT)
        upsert_model_config(
            conn,
            &ModelConfigRecord {
                id: 0,
                repo_id: "repo1".to_string(),
                display_name: None,
                backend: "llama.cpp".to_string(),
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                context_length: None,
                gpu_layers: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                num_parallel: None,
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
            },
        )
        .unwrap();
        let id1 = get_all_model_configs(conn)
            .unwrap()
            .iter()
            .find(|c| c.repo_id == "repo1")
            .unwrap()
            .id;
        upsert_model_file(conn, id1, "repo1", "file1.gguf", None, None, None).unwrap();

        upsert_model_config(
            conn,
            &ModelConfigRecord {
                id: 0,
                repo_id: "repo2".to_string(),
                display_name: None,
                backend: "llama.cpp".to_string(),
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                context_length: None,
                gpu_layers: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                num_parallel: None,
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
            },
        )
        .unwrap();
        upsert_model_config(
            conn,
            &ModelConfigRecord {
                id: 0,
                repo_id: "repo2".to_string(),
                display_name: None,
                backend: "llama.cpp".to_string(),
                enabled: true,
                selected_quant: None,
                selected_mmproj: None,
                context_length: None,
                gpu_layers: None,
                port: None,
                args: None,
                sampling: None,
                modalities: None,
                profile: None,
                api_name: None,
                health_check: None,
                num_parallel: None,
                created_at: "now".to_string(),
                updated_at: "now".to_string(),
            },
        )
        .unwrap();
        let id2 = get_all_model_configs(conn)
            .unwrap()
            .iter()
            .find(|c| c.repo_id == "repo2")
            .unwrap()
            .id;
        upsert_model_file(conn, id2, "repo2", "file2.gguf", None, None, None).unwrap();

        // Models dir is empty

        // Run scan
        cmd_scan(&config).unwrap();

        // Verify DB is clean
        let files = get_model_files(conn, id1).unwrap();
        assert!(files.is_empty());
        let configs = koji_core::db::queries::get_all_model_configs(conn).unwrap();
        assert!(configs.is_empty());
    }
}
