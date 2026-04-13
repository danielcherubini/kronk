use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
use koji_core::models::pull;
use koji_core::models::search::{self, SortBy};
use koji_core::models::{ModelCard, ModelMeta, ModelRegistry, QuantInfo};
use reqwest::Client;

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
        ModelCommands::Prune { dry_run, yes } => cmd_prune(config, dry_run, yes),
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
        ModelCommands::Verify { model } => cmd_verify(config, model).await,
        ModelCommands::VerifyExisting { model, verbose } => {
            cmd_verify_existing(config, model, verbose).await
        }
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
        let quant_key = unique_quant_key(&card.quants, &base_quant, &gguf.filename);

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
    _quant_arg: Option<String>,
    _profile_arg: Option<String>,
) -> Result<()> {
    let models_dir = config.models_dir()?;

    match model_id_arg {
        None => {
            // List all configured models
            if config.models.is_empty() {
                println!("No models configured. Run `koji model pull <repo>` to add one.");
                return Ok(());
            }

            let mut entries: Vec<(&String, &koji_core::config::ModelConfig)> =
                config.models.iter().collect();
            entries.sort_by_key(|(k, _)| k.as_str());

            println!("Configured models:\n");
            for (name, mc) in &entries {
                let repo = mc.model.as_deref().unwrap_or("(raw-args)");
                let quant = mc.quant.as_deref().unwrap_or("—");
                let status = if mc.enabled { "enabled" } else { "disabled" };

                // Check whether the GGUF file is present on disk
                let on_disk = mc
                    .model
                    .as_ref()
                    .and_then(|m| mc.quant.as_ref().map(|q| (m, q)))
                    .and_then(|(_m, q)| mc.quants.get(q.as_str()))
                    .map(|qe| {
                        koji_core::models::repo_path(&models_dir, repo)
                            .join(&qe.file)
                            .exists()
                    })
                    .unwrap_or(false);

                let disk_icon = if on_disk { "✓" } else { "✗" };

                println!(
                    "  {} {}  repo={} quant={}  backend={}  [{}]",
                    disk_icon, name, repo, quant, mc.backend, status
                );
            }
            println!();
        }
        Some(model_id) => {
            // Show detail for a specific config entry
            let mc = config.models.get(&model_id).with_context(|| {
                format!(
                    "Model config '{}' not found. Run `koji model ls` to see configured models.",
                    model_id
                )
            })?;

            println!("Config:   {}", model_id);
            if let Some(ref repo) = mc.model {
                println!("  Repo:     {}", repo);
            }
            println!("  Backend:  {}", mc.backend);
            if let Some(ref q) = mc.quant {
                println!("  Quant:    {}", q);
            }
            if let Some(ref ctx) = mc.context_length {
                println!("  Context:  {}", ctx);
            }
            println!("  Enabled:  {}", mc.enabled);

            if !mc.quants.is_empty() {
                println!("  Files:");
                let mut quants: Vec<_> = mc.quants.iter().collect();
                quants.sort_by_key(|(k, _)| k.as_str());
                for (qname, qe) in quants {
                    let repo = mc.model.as_deref().unwrap_or("");
                    let path = koji_core::models::repo_path(&models_dir, repo).join(&qe.file);
                    let present = if path.exists() { "✓" } else { "✗" };
                    println!("    {} {}  ({})", present, qname, qe.file);
                }
            }
        }
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
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            api_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
            modalities: None,
            display_name: None,
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
            let client = Client::new();
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
                        kind: koji_core::config::QuantKind::from_filename(filename),
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
                                kind: koji_core::config::QuantKind::from_filename(filename),
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

/// Remove orphaned GGUF files not referenced by any server config
fn cmd_prune(config: &Config, dry_run: bool, yes: bool) -> Result<()> {
    use std::collections::HashSet;

    // Helper to format bytes
    fn format_bytes(b: u64) -> String {
        const KIB: f64 = 1024.0;
        const MIB: f64 = KIB * 1024.0;
        const GIB: f64 = MIB * 1024.0;
        let bf = b as f64;
        if bf >= GIB {
            format!("{:.2} GiB", bf / GIB)
        } else if bf >= MIB {
            format!("{:.1} MiB", bf / MIB)
        } else if bf >= KIB {
            format!("{:.1} KiB", bf / KIB)
        } else {
            format!("{} B", b)
        }
    }

    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;

    // Build set of referenced files: (repo_id, filename)
    let mut referenced_files: HashSet<(String, String)> = HashSet::new();
    for model_config in config.models.values() {
        if let Some(ref repo_id) = model_config.model {
            for quant_entry in model_config.quants.values() {
                referenced_files.insert((repo_id.clone(), quant_entry.file.clone()));
            }
        }
    }

    // Scan for orphaned GGUF files recursively
    // Tuple: (repo_id, filename, file_path, size)
    let mut orphaned_files: Vec<(String, String, std::path::PathBuf, u64)> = Vec::new();

    if models_dir.exists() {
        // Recursively walk models_dir to find all GGUF files
        fn scan_for_ggufs(
            dir: &std::path::Path,
            base_dir: &std::path::Path,
            referenced_files: &std::collections::HashSet<(String, String)>,
            orphaned_files: &mut Vec<(String, String, std::path::PathBuf, u64)>,
        ) -> std::io::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_file() {
                    if path.extension().is_none_or(|e| e != "gguf") {
                        continue;
                    }

                    // Compute repo_id from relative path
                    let relative_path = path
                        .strip_prefix(base_dir)
                        .map_err(|_| std::io::Error::other("Failed to compute relative path"))?;
                    let repo_id = relative_path
                        .parent()
                        .and_then(|p| p.to_str())
                        .unwrap_or("")
                        .replace(std::path::MAIN_SEPARATOR, "/");

                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let file_key = (repo_id.clone(), filename.clone());

                    if !referenced_files.contains(&file_key) {
                        if let Ok(metadata) = std::fs::metadata(&path) {
                            orphaned_files.push((repo_id, filename, path, metadata.len()));
                        }
                    }
                } else if path.is_dir() {
                    scan_for_ggufs(&path, base_dir, referenced_files, orphaned_files)?;
                }
            }
            Ok(())
        }

        scan_for_ggufs(
            &models_dir,
            &models_dir,
            &referenced_files,
            &mut orphaned_files,
        )?;
    }

    if orphaned_files.is_empty() {
        println!("No orphaned GGUF files found.");
        return Ok(());
    }

    // Display what would be deleted
    println!("Found {} orphaned GGUF file(s):", orphaned_files.len());
    let mut total_size = 0u64;
    for (repo_id, filename, _, size) in &orphaned_files {
        println!("  {} ({}) [{}]", filename, format_bytes(*size), repo_id);
        total_size += size;
    }
    println!();
    println!("Total size: {}", format_bytes(total_size));

    if dry_run {
        println!();
        println!("Dry run complete. No files were deleted.");
        return Ok(());
    }

    // Prompt for confirmation
    if !yes {
        let confirm = inquire::Confirm::new("Delete these files?")
            .with_default(false)
            .prompt()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Delete files and track which ones succeeded for DB cleanup
    let mut deleted_count = 0;
    let mut actually_deleted: Vec<(String, String)> = Vec::new();

    for (repo_id, filename, file_path, _) in &orphaned_files {
        if let Err(e) = std::fs::remove_file(file_path) {
            tracing::warn!("Failed to delete {}: {}", file_path.display(), e);
        } else {
            deleted_count += 1;
            actually_deleted.push((repo_id.clone(), filename.clone()));
        }
    }

    // Clean up empty directories
    for (_, _, file_path, _) in &orphaned_files {
        if let Some(parent) = file_path.parent() {
            if parent
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
            {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    // Clean up orphaned model cards
    let mut orphaned_cards = Vec::new();
    if configs_dir.exists() {
        for entry in std::fs::read_dir(&configs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            // Try to load card and check if model dir still exists
            if let Ok(card) = ModelCard::load(&path) {
                if !card.model.source.is_empty() {
                    let model_dir = koji_core::models::repo_path(&models_dir, &card.model.source);
                    if !model_dir.exists()
                        || model_dir
                            .read_dir()
                            .map(|mut d| d.next().is_none())
                            .unwrap_or(true)
                    {
                        orphaned_cards.push(path.clone());
                    }
                }
            }
        }
    }

    for card_path in &orphaned_cards {
        let _ = std::fs::remove_file(card_path);
    }

    // Clean up DB records for actually deleted files
    if let Ok(db_dir) = koji_core::config::Config::config_dir() {
        if let Ok(koji_core::db::OpenResult { conn, .. }) = koji_core::db::open(&db_dir) {
            for (repo_id, filename) in &actually_deleted {
                let _ = koji_core::db::queries::delete_model_file(&conn, repo_id, filename);
            }
        }
    }

    println!();
    println!("Deleted {} file(s).", deleted_count);
    if !orphaned_cards.is_empty() {
        println!("Deleted {} orphaned model card(s).", orphaned_cards.len());
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

/// `koji model verify [<model>]`
///
/// Re-hashes every downloaded GGUF file and compares it to the HuggingFace
/// LFS SHA-256 stored at pull time. Writes the outcome back to the DB so the
/// web UI verified column stays consistent across invocations.
///
/// Exits with status 1 if any file fails verification.
async fn cmd_verify(config: &Config, model_filter: Option<String>) -> Result<()> {
    use koji_core::models::verify;

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

    println!("Verifying {} model(s)...", models.len());
    println!();

    let mut any_failed = false;
    let mut total_files: usize = 0;
    let mut total_ok: usize = 0;
    let mut total_unknown: usize = 0;
    let mut total_bad: usize = 0;

    for model in &models {
        // Mirror cmd_rm: legacy/hand-edited cards may have an empty
        // card.model.source, in which case fall back to the model id so we
        // still hit the right rows in the model_files table.
        let repo_id: &str = if model.card.model.source.is_empty() {
            &model.id
        } else {
            &model.card.model.source
        };
        // Use the registry-resolved directory from the InstalledModel itself
        // rather than reconstructing the path — legacy/hand-edited cards may
        // live under a directory that doesn't match `models_dir/repo_id`.
        let model_dir = &model.dir;
        println!("{}", repo_id);

        let results = match verify::verify_model(&conn, repo_id, model_dir) {
            Ok(r) => r,
            Err(e) => {
                println!("  verify error: {}", e);
                any_failed = true;
                continue;
            }
        };

        if results.is_empty() {
            println!("  (no files tracked — run `koji model update --refresh` first)");
            continue;
        }

        for r in &results {
            total_files += 1;
            let (icon, label) = match r.ok {
                Some(true) => {
                    total_ok += 1;
                    ("✓", "ok".to_string())
                }
                Some(false) => {
                    total_bad += 1;
                    any_failed = true;
                    (
                        "✗",
                        r.error.clone().unwrap_or_else(|| "mismatch".to_string()),
                    )
                }
                None => {
                    total_unknown += 1;
                    (
                        "—",
                        r.error
                            .clone()
                            .unwrap_or_else(|| "no upstream hash".to_string()),
                    )
                }
            };
            println!("  {} {}  {}", icon, r.filename, label);
        }
        println!();
    }

    println!(
        "Summary: {} file(s) total — {} ok, {} failed, {} unverifiable.",
        total_files, total_ok, total_bad, total_unknown
    );

    if any_failed {
        // Non-zero exit so scripting/CI can detect corruption.
        std::process::exit(1);
    }

    Ok(())
}

/// Verify existing models and backfill missing LFS hashes from HuggingFace.
///
/// This command:
/// 1. Scans for all installed models
/// 2. For models without LFS hashes, fetches metadata from HuggingFace
/// 3. Verifies all files against their stored LFS SHA-256 hashes
/// 4. Provides detailed progress output
///
/// Returns exit code 1 if any file fails verification.
async fn cmd_verify_existing(
    config: &Config,
    model_filter: Option<String>,
    verbose: bool,
) -> Result<()> {
    use koji_core::models::verify;

    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult {
        conn,
        needs_backfill: _,
    } = koji_core::db::open(&db_dir)?;

    let models_dir = config.models_dir()?;

    // Collect unique HF repo IDs from cfg.models (same source as the web UI).
    // Entries without a `model` field (raw-args entries) are skipped.
    let mut repo_ids: Vec<String> = config
        .models
        .values()
        .filter_map(|mc| mc.model.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    repo_ids.sort();

    let repo_ids: Vec<String> = match model_filter {
        Some(ref id) => {
            if repo_ids.contains(id) {
                vec![id.clone()]
            } else {
                anyhow::bail!("Model '{}' not found in config.", id);
            }
        }
        None => repo_ids,
    };

    // Warn about any entries that have no `model` field
    let skipped: Vec<&str> = config
        .models
        .iter()
        .filter(|(_, mc)| mc.model.is_none())
        .map(|(name, _)| name.as_str())
        .collect();
    for name in &skipped {
        println!(
            "Skipping '{}': no HuggingFace repo ID in config (raw-args entry).",
            name
        );
    }

    if repo_ids.is_empty() {
        println!("No models with a HuggingFace repo ID found in config.");
        return Ok(());
    }

    println!(
        "Verifying {} model(s) and backfilling missing hashes...",
        repo_ids.len()
    );
    println!();

    let mut any_failed = false;
    let mut total_files: usize = 0;
    let mut total_ok: usize = 0;
    let mut total_unknown: usize = 0;
    let mut total_bad: usize = 0;
    let mut total_backfilled: usize = 0;

    for repo_id in &repo_ids {
        let repo_id: &str = repo_id.as_str();
        let model_dir = koji_core::models::repo_path(&models_dir, repo_id);

        println!("Model: {}", repo_id);

        // Check if any files need hash backfilling
        let records = match koji_core::db::queries::get_model_files(&conn, repo_id) {
            Ok(r) => r,
            Err(e) => {
                println!("  Error reading database: {}", e);
                any_failed = true;
                continue;
            }
        };

        if records.is_empty() {
            println!(
                "  (no files tracked — run `koji model pull {}` first)",
                repo_id
            );
            println!();
            continue;
        }

        let needs_backfill = records.iter().any(|r| r.lfs_oid.is_none());

        if needs_backfill {
            // Count how many records need backfilling before we fetch
            let records_needing_backfill = records.iter().filter(|r| r.lfs_oid.is_none()).count();

            if verbose {
                println!(
                    "  Fetching metadata from HuggingFace to backfill {} missing hash(es)...",
                    records_needing_backfill
                );
            }

            // Always refresh metadata when needed, regardless of verbose flag
            match koji_core::models::update::refresh_metadata(&conn, repo_id).await {
                Ok(_) => {
                    // Re-fetch records to see how many were successfully backfilled
                    let updated_records =
                        match koji_core::db::queries::get_model_files(&conn, repo_id) {
                            Ok(r) => r,
                            Err(e) => {
                                println!("  Error reading database: {}", e);
                                any_failed = true;
                                continue;
                            }
                        };
                    // Count how many still need backfilling after the refresh
                    let still_needing_backfill = updated_records
                        .iter()
                        .filter(|r| r.lfs_oid.is_none())
                        .count();
                    // The difference is how many were successfully backfilled
                    let backfilled_count =
                        records_needing_backfill.saturating_sub(still_needing_backfill);
                    if verbose {
                        println!("  Backfilled {} missing hash(es)", backfilled_count);
                    }
                    total_backfilled += backfilled_count;
                }
                Err(e) => {
                    if verbose {
                        println!(
                            "  Warning: Failed to fetch metadata: {}. Proceeding with verification; files without hashes will be marked as unverifiable.",
                            e
                        );
                    }
                }
            }
        }

        let results = match verify::verify_model(&conn, repo_id, &model_dir) {
            Ok(r) => r,
            Err(e) => {
                println!("  verify error: {}", e);
                any_failed = true;
                continue;
            }
        };

        if results.is_empty() {
            println!("  (no files tracked)");
            continue;
        }

        for r in &results {
            total_files += 1;
            let (icon, label) = match r.ok {
                Some(true) => {
                    total_ok += 1;
                    if verbose {
                        (
                            "✓",
                            format!(
                                "ok ({}...)",
                                r.expected_sha
                                    .as_deref()
                                    .unwrap_or("unknown")
                                    .chars()
                                    .take(10)
                                    .collect::<String>()
                            ),
                        )
                    } else {
                        ("✓", "ok".to_string())
                    }
                }
                Some(false) => {
                    total_bad += 1;
                    any_failed = true;
                    if verbose {
                        (
                            "✗",
                            r.error.clone().unwrap_or_else(|| "mismatch".to_string()),
                        )
                    } else {
                        ("✗", "failed".to_string())
                    }
                }
                None => {
                    total_unknown += 1;
                    if verbose {
                        (
                            "—",
                            r.error
                                .clone()
                                .unwrap_or_else(|| "no upstream hash".to_string()),
                        )
                    } else {
                        ("—", "unverifiable".to_string())
                    }
                }
            };
            if verbose {
                println!("  {} {}  {}", icon, r.filename, label);
            }
        }
        println!();
    }

    // Build summary
    let mut summary_parts: Vec<String> = Vec::new();
    summary_parts.push(format!("{} file(s) total", total_files));
    summary_parts.push(format!("{} verified OK", total_ok));
    if total_bad > 0 {
        summary_parts.push(format!("{} failed", total_bad));
    }
    if total_unknown > 0 {
        summary_parts.push(format!("{} unverifiable", total_unknown));
    }
    if total_backfilled > 0 {
        summary_parts.push(format!("{} hashes backfilled", total_backfilled));
    }

    println!("Summary: {}", summary_parts.join(", "));
    println!();

    if any_failed {
        std::process::exit(1);
    }

    Ok(())
}
