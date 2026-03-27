use anyhow::{Context, Result};
use kronk_core::config::Config;
use kronk_core::models::pull;
use kronk_core::models::search::{self, SortBy};
use kronk_core::models::{ModelCard, ModelMeta, ModelRegistry, QuantInfo};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::cli::ModelCommands;

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
        } => cmd_create(config, &name, &model, quant, profile, backend).await,
        ModelCommands::Rm { model } => cmd_rm(config, &model),
        ModelCommands::Scan => cmd_scan(config),
        ModelCommands::Search {
            query,
            sort,
            limit,
            pull,
        } => cmd_search(config, &query, &sort, limit, pull).await,
    }
}

async fn cmd_pull(config: &Config, repo_id: &str) -> Result<()> {
    println!("Pull the lever!");
    println!();
    println!("  Fetching file list from {}...", repo_id);

    let (resolved_repo, ggufs) = pull::list_gguf_files(repo_id).await?;

    if resolved_repo != repo_id {
        println!("  Resolved to: {}", resolved_repo);
    }

    // Use the resolved repo_id for all subsequent operations
    let repo_id = &resolved_repo;

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

    let models_dir_pathbuf = config.models_dir()?.to_path_buf();
    // Strip -GGUF suffix from directory name (cleaner paths)
    // "Tesslate/OmniCoder-9B-GGUF" -> models_dir/Tesslate/OmniCoder-9B
    let clean_parts: Vec<String> = repo_id
        .split('/')
        .map(|part| {
            part.strip_suffix("-GGUF")
                .or_else(|| part.strip_suffix("-gguf"))
                .unwrap_or(part)
                .to_string()
        })
        .collect();
    let model_id = clean_parts.join("/"); // Re-introduce model_id
    let mut model_dir: PathBuf = models_dir_pathbuf.clone();
    for part in clean_parts.iter() {
        model_dir.push(part.as_str()); // Use push with &str
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
                },
                sampling: HashMap::new(),
                quants: HashMap::new(),
            };
            // Seed sampling from config's sampling_templates
            new_card.populate_sampling_from(&config.sampling_templates);
            new_card
        }
    };

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

        println!("  Downloaded: {}", result.path.display());
    }

    // Suggest context sizes based on VRAM and model size
    let largest_model_bytes = card
        .quants
        .values()
        .filter_map(|q| q.size_bytes)
        .max()
        .unwrap_or(0);

    let vram = kronk_core::gpu::query_vram();

    let selected_ctx = if largest_model_bytes > 0 {
        let suggestions =
            kronk_core::gpu::suggest_context_sizes(largest_model_bytes, vram.as_ref());

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

    card.save(&card_path)?;

    println!();
    println!("Oh yeah, it's all coming together.");
    println!("  Model card saved: {}", card_path.display());
    println!();
    println!("  Create a model config:");
    println!(
        "    kronk model create my-server --model {} --profile coding",
        model_id
    );

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
            "Model '{}' not found. Run `kronk model ls` to see installed models.",
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

    let resolved_profile: Option<kronk_core::profiles::Profile> = match profile_arg {
        Some(p) => Some(
            p.parse::<kronk_core::profiles::Profile>()
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
    server_name: &str,  // Renamed from 'name' to avoid confusion and match usage
    model_id_arg: &str, // Renamed from 'model_id' to avoid confusion and match usage
    quant_name: Option<String>,
    profile_name_arg: Option<String>, // Renamed from 'profile' to avoid confusion and match usage
    backend_arg: Option<String>, // Renamed from 'backend_key' to avoid confusion and match usage
) -> Result<()> {
    let mut config = config.clone(); // Clone config to allow modification

    // No host args needed — proxy handles routing
    let args = vec![];

    // Check if server name already exists
    if config.models.contains_key(server_name) {
        anyhow::bail!(
            "Server '{}' already exists. Use `kronk server edit` or choose a different name.",
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
                0 => anyhow::bail!("No backends configured. Add one first with `kronk add`."),
                1 => keys.into_iter().next().unwrap(),
                _ => inquire::Select::new("Select a backend:", keys)
                    .prompt()
                    .context("Backend selection cancelled")?,
            }
        }
    };

    // Resolve profile
    let resolved_profile: Option<kronk_core::profiles::Profile> = match profile_name_arg {
        Some(p) => Some(
            p.parse::<kronk_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))?,
        ),
        None => None,
    };

    config.models.insert(
        server_name.to_string(), // Use the 'server_name' parameter
        kronk_core::config::ModelConfig {
            backend: resolved_backend_key.clone(),
            args,
            profile: resolved_profile,
            sampling: None,
            model: Some(model_id_arg.to_string()),
            quant: quant_name.clone(), // Use quant_name
            port: None,
            health_check: None,
            enabled: true,
            source: Some(model_id_arg.to_string()),
            context_length: None,
        },
    );

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Name:      {}", server_name);
    println!("  Model:     {}", model_id_arg);
    if let Some(ref q) = quant_name {
        // Use quant_name
        println!("  Quant:     {}", q);
    }
    // GGUF path needs to be resolved here if needed for output, for now just remove.
    // let models_dir = config.models_dir()?;
    // let configs_dir = config.configs_dir()?;
    // let registry = ModelRegistry::new(models_dir.clone(), configs_dir.clone());
    // let gguf_path = registry.gguf_path(model_id_arg, &quant_name.unwrap_or_default())?; // This needs error handling
    // println!("  GGUF:      {}", gguf_path.display());

    // Instead, let's look up the model config that was just created to get the profile.
    if let Some(p) = &config
        .models
        .get(server_name)
        .and_then(|mc| mc.profile.as_ref())
    {
        println!("  Profile:   {}", p);
    }
    println!();
    println!("Enable it:   kronk model enable {}", server_name);
    println!("Start:       kronk serve");

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

    println!("No touchy! Model '{}' removed.", model_id);
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
        println!("Oh yeah, it's all coming together. Model cards updated.");
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
        println!("  Pull one:  kronk model pull <model-id>");
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
