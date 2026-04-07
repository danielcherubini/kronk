use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
use koji_core::models::pull;
use koji_core::models::search::{self, SortBy};
use koji_core::models::{ModelCard, ModelMeta, ModelRegistry, QuantInfo};

use crate::cli::ModelCommands;

/// Return a naive ISO 8601 UTC timestamp for DB logging.
fn manual_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = secs_to_datetime(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        y, mo, d, h, mi, s
    )
}

/// Convert Unix seconds to (year, month, day, hour, min, sec) UTC.
fn secs_to_datetime(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let days = secs / 86400;
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days_in_year = if leap { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_months: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &dim in &days_in_months {
        if remaining < dim {
            break;
        }
        remaining -= dim;
        month += 1;
    }
    (year, month, remaining + 1, hour, min, sec)
}

/// Generate a unique key for a quant entry, avoiding collisions in the map.
/// If `base_key` is already taken, appends the filename stem as a suffix.
fn unique_quant_key(quants: &HashMap<String, QuantInfo>, base_key: &str, filename: &str) -> String {
    if !quants.contains_key(base_key) {
        return base_key.to_string();
    }
    // Use filename without .gguf extension as a unique fallback
    let stem = filename.strip_suffix(".gguf").unwrap_or(filename);
    let candidate = format!("{}:{}", base_key, stem);
    if !quants.contains_key(&candidate) {
        return candidate;
    }
    // Numeric suffix as last resort
    let mut i = 1;
    loop {
        let key = format!("{}-{}", base_key, i);
        if !quants.contains_key(&key) {
            return key;
        }
        i += 1;
    }
}

pub async fn run(config: &Config, command: ModelCommands) -> Result<()> {
    match command {
        ModelCommands::Pull { repo } => cmd_pull(config, &repo).await,
        ModelCommands::Ls {
            model,
            quant,
            profile,
        } => cmd_ls(config, model, quant, profile),
        ModelCommands::Enable { name } => cmd_enable(config, &name),
        ModelCommands::Disable { name } => cmd_disable(config, &name),
        ModelCommands::Create {
            name,
            model,
            quant,
            profile,
            backend,
        } => cmd_create(config, name, &model, quant, profile, backend).await,
        ModelCommands::Rm { model } => cmd_rm(config, &model),
        ModelCommands::Scan => cmd_scan(config),
        ModelCommands::Update {
            model,
            check,
            refresh,
            yes,
        } => cmd_update(config, model, check, refresh, yes).await,
        ModelCommands::Search {
            query,
            sort,
            limit,
            pull,
        } => cmd_search(config, &query, &sort, limit, pull).await,
    }
}

async fn cmd_pull(config: &Config, repo_id: &str) -> Result<()> {
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
            let name = repo_id.rsplit('/').next().unwrap_or(repo_id).to_string();
            let mut new_card = ModelCard {
                model: ModelMeta {
                    name,
                    source: repo_id.to_string(),
                    default_context_length: None, // set by interactive context prompt
                    default_gpu_layers: Some(999),
                    mmproj: None,
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

        let result = pull::download_gguf(repo_id, &gguf.filename, &model_dir).await?;

        let base_quant = gguf.quant.clone().unwrap_or_else(|| gguf.filename.clone());
        let quant_key = unique_quant_key(&card.quants, &base_quant, &gguf.filename);

        card.quants.insert(
            quant_key,
            QuantInfo {
                file: gguf.filename.clone(),
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

        koji_core::db::queries::upsert_model_pull(&conn, repo_id, &listing.commit_sha)?;

        let now = manual_timestamp();
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

fn cmd_ls(
    config: &Config,
    model_id_arg: Option<String>,
    quant_arg: Option<String>,
    profile_arg: Option<String>,
) -> Result<()> {
    let model_id = model_id_arg.context("Model identifier required")?;
    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());

    let installed = registry.find(&model_id)?.with_context(|| {
        format!(
            "Model '{}' not found. Run `koji model ls` to see installed models.",
            model_id
        )
    })?;

    let quant_name = match quant_arg {
        Some(q) => {
            if !installed.card.quants.contains_key(&q) {
                let available: Vec<&str> =
                    installed.card.quants.keys().map(|s| s.as_str()).collect();
                anyhow::bail!(
                    "Quant '{}' not found. Available: {}",
                    q,
                    available.join(", ")
                );
            }
            q
        }
        None => {
            let quant_names: Vec<String> = installed.card.quants.keys().cloned().collect();
            if quant_names.is_empty() {
                anyhow::bail!("No quants available for '{}'. Pull some first.", model_id);
            }
            if quant_names.len() == 1 {
                quant_names.into_iter().next().unwrap()
            } else {
                inquire::Select::new("Select a quant:", quant_names)
                    .prompt()
                    .context("Quant selection cancelled")?
            }
        }
    };

    let resolved_profile: Option<koji_core::profiles::Profile> = match profile_arg {
        Some(p) => Some(
            p.parse::<koji_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))?,
        ),
        None => None,
    };

    // Verify the GGUF file exists on disk
    let gguf_path = registry
        .gguf_path(&model_id, &quant_name)?
        .with_context(|| format!("GGUF file for quant '{}' not found on disk", quant_name))?;

    println!("Model:     {}", model_id);
    println!("  Quant:     {}", quant_name);
    println!("  GGUF:      {}", gguf_path.display());
    if let Some(p) = &resolved_profile {
        println!("  Profile:   {}", p);
    }

    Ok(())
}

async fn cmd_create(
    config: &Config,
    server_name_arg: Option<String>,
    model_id_arg: &str,
    quant_name: Option<String>,
    profile_name_arg: Option<String>,
    backend_arg: Option<String>,
) -> Result<()> {
    let mut config = config.clone();

    // No host args needed — proxy handles routing
    let args = vec![];

    // Resolve server name — prompt if not provided
    let server_name = match server_name_arg {
        Some(n) => n,
        None => inquire::Text::new("Config name (e.g. gemma4-coding):")
            .prompt()
            .context("Config name input cancelled")?,
    };

    // Check if server name already exists
    if config.models.contains_key(&server_name) {
        anyhow::bail!(
            "Server '{}' already exists. Use `koji server edit` or choose a different name.",
            server_name
        );
    }

    // Resolve backend
    let resolved_backend_key = match backend_arg {
        // Use backend_arg
        Some(b) => {
            if !config.backends.contains_key(&b) {
                let available: Vec<&str> = config.backends.keys().map(|s| s.as_str()).collect();
                anyhow::bail!(
                    "Backend '{}' not found. Available: {}",
                    b,
                    available.join(", ")
                );
            }
            b
        }
        None => {
            let keys: Vec<String> = config.backends.keys().cloned().collect();
            match keys.len() {
                0 => anyhow::bail!("No backends configured. Add one first with `koji add`."),
                1 => keys.into_iter().next().unwrap(),
                _ => inquire::Select::new("Select a backend:", keys)
                    .prompt()
                    .context("Backend selection cancelled")?,
            }
        }
    };

    // Resolve profile
    let resolved_profile: Option<koji_core::profiles::Profile> = match profile_name_arg {
        Some(p) => Some(
            p.parse::<koji_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))?,
        ),
        None => None,
    };

    config.models.insert(
        server_name.clone(),
        koji_core::config::ModelConfig {
            backend: resolved_backend_key.clone(),
            args,
            profile: resolved_profile.map(|p| p.to_string()),
            sampling: None,
            model: Some(model_id_arg.to_string()),
            quant: quant_name.clone(),
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            display_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
        },
    );

    config.save()?;

    println!("Created.");
    println!();
    println!("  Name:      {}", server_name);
    println!("  Model:     {}", model_id_arg);
    if let Some(ref q) = quant_name {
        println!("  Quant:     {}", q);
    }
    if let Some(mc) = config.models.get(&server_name) {
        if let Some(sampling) = &mc.sampling {
            println!("  Profile:   {}", sampling.preset_label());
        } else if let Some(p) = &mc.profile {
            println!("  Profile:   {}", p);
        }
    }
    println!();
    println!("Enable it:   koji model enable {}", server_name);
    println!("Start:       koji serve");

    Ok(())
}

fn cmd_rm(config: &Config, model_id: &str) -> Result<()> {
    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());

    let model = registry
        .find(model_id)?
        .with_context(|| format!("Model '{}' not found.", model_id))?;

    let linked_servers: Vec<&str> = config
        .models
        .iter()
        .filter(|(_, p)| p.model.as_deref() == Some(model_id))
        .map(|(name, _)| name.as_str())
        .collect();

    if !linked_servers.is_empty() {
        anyhow::bail!(
            "Cannot remove '{}': referenced by servers: {}. Remove those first.",
            model_id,
            linked_servers.join(", ")
        );
    }

    let confirm = inquire::Confirm::new(&format!("Remove model '{}' and all its files?", model_id))
        .with_default(false)
        .prompt()
        .context("Confirmation cancelled")?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    std::fs::remove_dir_all(&model.dir)
        .with_context(|| format!("Failed to remove: {}", model.dir.display()))?;

    // Clean up empty parent dir
    if let Some(parent) = model.dir.parent() {
        if parent
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(parent);
        }
    }

    // Also remove the config card from configs/
    if model.card_path.exists() {
        std::fs::remove_file(&model.card_path)?;
    }

    // Clean up DB metadata (best-effort — model deletion succeeds even if DB is unavailable).
    // Use model.card.model.source as the DB key (it's the HF repo_id stored during pull),
    // falling back to model.id (file-derived) if source is empty.
    if let Ok(db_dir) = koji_core::config::Config::config_dir() {
        if let Ok(OpenResult {
            conn,
            needs_backfill: _,
        }) = koji_core::db::open(&db_dir)
        {
            let repo_key = if model.card.model.source.is_empty() {
                &model.id
            } else {
                &model.card.model.source
            };
            let _ = koji_core::db::queries::delete_model_records(&conn, repo_key);
        }
    }

    println!("Removed model '{}'.", model_id);
    Ok(())
}

async fn cmd_update(
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
            update::refresh_metadata(&conn, repo_id).await?;
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
        let blobs = koji_core::models::pull::fetch_blob_metadata(repo_id)
            .await
            .ok();

        // Clone card once before the loop so all file updates are accumulated
        let mut card = model.card.clone();

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
            let dl =
                koji_core::models::pull::download_gguf(repo_id, &file_info.filename, &model.dir)
                    .await?;

            // Update DB with blob metadata (fetched once above)
            let lfs_sha = blobs
                .as_ref()
                .and_then(|b| b.get(&file_info.filename))
                .and_then(|b| b.lfs_sha256.as_deref());

            let _ = koji_core::db::queries::upsert_model_file(
                &conn,
                repo_id,
                &file_info.filename,
                file_info.quant.as_deref(),
                lfs_sha,
                Some(dl.size_bytes as i64),
            );

            let now = manual_timestamp();

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
        let _ = koji_core::db::queries::upsert_model_pull(&conn, repo_id, &listing.commit_sha);
    }

    println!();
    println!("Models updated.");
    Ok(())
}

fn cmd_scan(config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());
    let models = registry.scan()?;

    let mut found_any = false;

    // Check existing models for untracked GGUFs
    for model in &models {
        let untracked = registry.untracked_ggufs(&model.dir, &model.card)?;
        if !untracked.is_empty() {
            println!(
                "  {} -- found {} untracked GGUF file(s):",
                model.id,
                untracked.len()
            );
            let mut card = model.card.clone();
            for filename in &untracked {
                let base_quant = pull::infer_quant_from_filename(filename)
                    .unwrap_or_else(|| "unknown".to_string());
                let quant_key = unique_quant_key(&card.quants, &base_quant, filename);
                let size_bytes = model.dir.join(filename).metadata().map(|m| m.len()).ok();
                println!("    + {} ({})", filename, base_quant);
                card.quants.insert(
                    quant_key,
                    QuantInfo {
                        file: filename.clone(),
                        size_bytes,
                        context_length: None,
                    },
                );
            }
            card.save(&model.card_path)?;
            found_any = true;
        }
    }

    // Scan for directories with GGUFs but no model card in configs/
    let known_ids: std::collections::HashSet<String> =
        models.iter().map(|m| m.id.clone()).collect();

    if models_dir.exists() {
        for company_entry in std::fs::read_dir(&models_dir)? {
            let company_entry = company_entry?;
            if !company_entry.path().is_dir() {
                continue;
            }
            let company = company_entry.file_name().to_string_lossy().to_string();

            for model_entry in std::fs::read_dir(company_entry.path())? {
                let model_entry = model_entry?;
                let model_path = model_entry.path();
                if !model_path.is_dir() {
                    continue;
                }

                let model_name = model_entry.file_name().to_string_lossy().to_string();
                let model_id = format!("{}/{}", company, model_name);

                // Skip if already tracked in configs/
                if known_ids.contains(&model_id) {
                    continue;
                }

                let gguf_files: Vec<String> = std::fs::read_dir(&model_path)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".gguf"))
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();

                if !gguf_files.is_empty() {
                    println!(
                        "  {} -- new model with {} GGUF file(s):",
                        model_id,
                        gguf_files.len()
                    );

                    let mut quants = HashMap::new();
                    for filename in &gguf_files {
                        let base_quant = pull::infer_quant_from_filename(filename)
                            .unwrap_or_else(|| "unknown".to_string());
                        let quant_key = unique_quant_key(&quants, &base_quant, filename);
                        let size_bytes = model_path.join(filename).metadata().map(|m| m.len()).ok();
                        println!("    + {} ({})", filename, base_quant);
                        quants.insert(
                            quant_key,
                            QuantInfo {
                                file: filename.clone(),
                                size_bytes,
                                context_length: None,
                            },
                        );
                    }

                    let card = ModelCard {
                        model: ModelMeta {
                            name: model_name,
                            source: model_id.clone(),
                            default_context_length: Some(8192),
                            default_gpu_layers: Some(999),
                            mmproj: None,
                        },
                        sampling: HashMap::new(),
                        quants,
                    };
                    std::fs::create_dir_all(&configs_dir)?;
                    let card_filename = format!("{}.toml", model_id.replace('/', "--"));
                    card.save(&configs_dir.join(&card_filename))?;
                    found_any = true;
                }
            }
        }
    }

    if !found_any {
        println!("No untracked models or GGUF files found.");
    } else {
        println!();
        println!("Model cards updated.");
    }

    Ok(())
}

fn cmd_enable(config: &Config, name: &str) -> Result<()> {
    let mut config = config.clone();
    let srv = config
        .models
        .get_mut(name)
        .with_context(|| format!("Model '{}' not found", name))?;
    srv.enabled = true;
    config.save()?;
    println!("Enabled model: {}", name);
    Ok(())
}

fn cmd_disable(config: &Config, name: &str) -> Result<()> {
    let mut config = config.clone();
    let srv = config
        .models
        .get_mut(name)
        .with_context(|| format!("Model '{}' not found", name))?;
    srv.enabled = false;
    config.save()?;
    println!("Disabled model: {}", name);
    Ok(())
}

async fn cmd_search(
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
            format_downloads(result.downloads),
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
        cmd_pull(config, &selected).await?;
    } else {
        println!("  Pull one:  koji model pull <model-id>");
    }

    Ok(())
}

fn format_downloads(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
