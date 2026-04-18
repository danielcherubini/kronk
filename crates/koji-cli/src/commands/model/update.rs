use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
use koji_core::models::search::{self, SortBy};
use koji_core::models::ModelRegistry;
use reqwest::Client;

pub(super) async fn cmd_update(
    config: &Config,
    model_filter: Option<String>,
    check_only: bool,
    refresh: bool,
    yes: bool,
) -> Result<()> {
    use koji_core::models::update;

    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult {
        conn,
        needs_backfill: _,
    } = koji_core::db::open(&db_dir)?;

    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());

    let models: Vec<koji_core::models::InstalledModel> = match model_filter {
        Some(ref id) => {
            let found = registry
                .find(id)?
                .with_context(|| format!("Model '{}' not found.", id))?;
            vec![found]
        }
        None => registry.scan()?,
    };

    if models.is_empty() {
        println!("No installed models found.");
        return Ok(());
    }

    // --- Refresh mode: stamp metadata without downloading ---
    if refresh {
        for model in &models {
            let repo_id = &model.card.model.source;
            print!("  Refreshing metadata for {}...", repo_id);
            update::refresh_metadata(&conn, &models_dir, repo_id).await?;
            println!(" done.");
        }
        println!();
        println!("Metadata refreshed.");
        return Ok(());
    }

    // --- Check / update mode ---
    println!("Checking for updates...");
    println!();

    let mut check_results: Vec<update::UpdateCheckResult> = Vec::new();
    for model in &models {
        let repo_id = &model.card.model.source;
        let result = update::check_for_updates(&conn, repo_id).await?;
        check_results.push(result);
    }

    // Display results
    for result in &check_results {
        println!("{}", result.repo_id);
        let status_label = match &result.status {
            update::UpdateStatus::UpToDate => "Up to date".to_string(),
            update::UpdateStatus::NoPriorRecord => {
                "No prior record (run with --refresh to enable tracking)".to_string()
            }
            update::UpdateStatus::UpdatesAvailable => "Updates available".to_string(),
            update::UpdateStatus::RepoChangedFilesUnchanged => {
                "Repo changed, files unchanged".to_string()
            }
            update::UpdateStatus::VerificationFailed => {
                "Verification failed (no stored hashes — run with --refresh)".to_string()
            }
            update::UpdateStatus::CheckFailed(msg) => format!("Check failed: {}", msg),
        };
        println!("  Status: {}", status_label);

        for file in &result.file_updates {
            let quant_label = file.quant.as_deref().unwrap_or("?");
            let file_status = match &file.status {
                update::FileStatus::Unchanged => "unchanged".to_string(),
                update::FileStatus::Changed { .. } => {
                    let old_gib = file
                        .local_size
                        .map(|s| format!("{:.1} GiB", s as f64 / 1_073_741_824.0))
                        .unwrap_or_else(|| "?".to_string());
                    let new_gib = file
                        .remote_size
                        .map(|s| format!("{:.1} GiB", s as f64 / 1_073_741_824.0))
                        .unwrap_or_else(|| "?".to_string());
                    format!("changed ({} → {})", old_gib, new_gib)
                }
                update::FileStatus::NewRemote => "new remote file".to_string(),
                update::FileStatus::Unknown => "unknown (no hash stored)".to_string(),
                update::FileStatus::RemovedFromRemote => "removed from remote".to_string(),
            };
            println!("    {:<8}  {}  {}", quant_label, file.filename, file_status);
        }
        println!();
    }

    if check_only {
        return Ok(());
    }

    // Collect models with available updates
    let models_to_update: Vec<(
        &update::UpdateCheckResult,
        &koji_core::models::InstalledModel,
    )> = check_results
        .iter()
        .zip(models.iter())
        .filter(|(r, _)| matches!(r.status, update::UpdateStatus::UpdatesAvailable))
        .collect();

    if models_to_update.is_empty() {
        println!("All models up to date.");
        return Ok(());
    }

    // Count files that need downloading
    let total_files: usize = models_to_update
        .iter()
        .flat_map(|(r, _)| r.file_updates.iter())
        .filter(|f| {
            matches!(
                f.status,
                update::FileStatus::Changed { .. } | update::FileStatus::NewRemote
            )
        })
        .count();

    if !yes {
        let confirm =
            inquire::Confirm::new(&format!("Download updates for {} file(s)?", total_files))
                .with_default(true)
                .prompt()
                .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Download updates
    for (result, model) in &models_to_update {
        let repo_id = &result.repo_id;

        // Fetch fresh listing and blob metadata once per repo (not per file)
        let listing = koji_core::models::pull::list_gguf_files(repo_id).await?;
        let blobs = koji_core::models::pull::fetch_blob_metadata(&listing.repo_id)
            .await
            .ok();

        // Clone card once before the loop so all file updates are accumulated
        let mut card = model.card.clone();

        // Reuse a single HTTP client for all downloads in this repo
        let client = Client::new();

        for file_info in &result.file_updates {
            let should_download = matches!(
                file_info.status,
                update::FileStatus::Changed { .. } | update::FileStatus::NewRemote
            );
            if !should_download {
                continue;
            }

            // Delete old file if Changed
            if matches!(file_info.status, update::FileStatus::Changed { .. }) {
                let old_path = model.dir.join(&file_info.filename);
                if old_path.exists() {
                    std::fs::remove_file(&old_path).with_context(|| {
                        format!("Failed to remove old file: {}", old_path.display())
                    })?;
                }
            }

            println!("  Downloading {}...", file_info.filename);
            let dl = koji_core::models::pull::download_gguf(
                &client,
                repo_id,
                &file_info.filename,
                &model.dir,
            )
            .await?;

            // Update DB with blob metadata (fetched once above)
            let lfs_sha = blobs
                .as_ref()
                .and_then(|b| b.get(&file_info.filename))
                .and_then(|b| b.lfs_sha256.as_deref());

            // Look up model_id for DB writes
            let model_id =
                match koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)? {
                    Some(r) => r.id,
                    None => {
                        tracing::warn!("Model {} not in DB, skipping", repo_id);
                        continue;
                    }
                };
            let _ = koji_core::db::queries::upsert_model_file(
                &conn,
                model_id,
                repo_id,
                &file_info.filename,
                file_info.quant.as_deref(),
                lfs_sha,
                Some(dl.size_bytes as i64),
            );

            let now = super::utils::manual_timestamp();

            let _ = koji_core::db::queries::log_download(
                &conn,
                &koji_core::db::queries::DownloadLogEntry {
                    repo_id: repo_id.to_string(),
                    filename: file_info.filename.clone(),
                    started_at: now.clone(),
                    completed_at: Some(now),
                    size_bytes: Some(dl.size_bytes as i64),
                    duration_ms: None,
                    success: true,
                    error_message: None,
                },
            );

            // Accumulate size_bytes updates into the shared card clone
            for quant_info in card.quants.values_mut() {
                if quant_info.file == file_info.filename {
                    quant_info.size_bytes = Some(dl.size_bytes);
                }
            }

            println!("    Done: {}", dl.path.display());
        }

        // Save card once after all files for this repo are processed
        card.save(&model.card_path)?;

        // Update DB pull record with new commit SHA
        let model_id =
            koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)?.map(|r| r.id);
        if let Some(id) = model_id {
            let _ =
                koji_core::db::queries::upsert_model_pull(&conn, id, repo_id, &listing.commit_sha);
        }
    }

    println!();
    println!("Models updated.");
    Ok(())
}

pub(super) async fn cmd_search(
    config: &Config,
    query: &str,
    sort: &str,
    limit: usize,
    pull: bool,
) -> Result<()> {
    let sort_by = match sort {
        "likes" => SortBy::Likes,
        "modified" => SortBy::Modified,
        _ => SortBy::Downloads,
    };

    println!("  Searching HuggingFace for GGUF models: \"{}\"...", query);
    println!();

    let results = search::search_models(query, sort_by, limit).await?;

    if results.is_empty() {
        println!("  No GGUF models found for \"{}\".", query);
        return Ok(());
    }

    // Display results as a formatted table
    println!("  {:<50} {:>12} {:>8}", "MODEL", "DOWNLOADS", "LIKES");
    println!("  {}", "-".repeat(74));

    for result in &results {
        let id = if result.model_id.len() > 48 {
            let chars: Vec<char> = result.model_id.chars().take(45).collect();
            format!("{}...", chars.iter().collect::<String>())
        } else {
            result.model_id.clone()
        };
        println!(
            "  {:<50} {:>12} {:>8}",
            id,
            super::utils::format_downloads(result.downloads),
            result.likes,
        );
    }

    println!();

    if pull {
        // Let user pick a result to pull
        let options: Vec<String> = results.iter().map(|r| r.model_id.clone()).collect();
        let selected = inquire::Select::new("Pull which model?", options)
            .prompt()
            .context("Selection cancelled")?;

        // Delegate to cmd_pull
        super::pull::cmd_pull(config, &selected).await?;
    } else {
        println!("  Pull one:  koji model pull <model-id>");
    }

    Ok(())
}
