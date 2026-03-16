use anyhow::{Context, Result};
use kronk_core::config::Config;
use kronk_core::models::pull;
use kronk_core::models::{ModelCard, ModelMeta, ModelRegistry, QuantInfo};

use crate::ModelCommands;

pub async fn run(config: &Config, command: ModelCommands) -> Result<()> {
    match command {
        ModelCommands::Pull { repo } => cmd_pull(config, &repo).await,
        ModelCommands::Ls => cmd_ls(config),
        ModelCommands::Ps => cmd_ps(config).await,
        ModelCommands::Create {
            name,
            model,
            quant,
            use_case,
        } => cmd_create(config, &name, &model, quant, use_case).await,
        ModelCommands::Rm { model } => cmd_rm(config, &model),
        ModelCommands::Scan => cmd_scan(config),
    }
}

async fn cmd_pull(config: &Config, repo_id: &str) -> Result<()> {
    println!("Pull the lever!");
    println!();
    println!("  Fetching file list from {}...", repo_id);

    let ggufs = pull::list_gguf_files(repo_id).await?;

    if ggufs.is_empty() {
        anyhow::bail!("No GGUF files found in repository '{}'", repo_id);
    }

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

    let models_dir = config.models_dir()?;
    let model_dir = models_dir.join(repo_id);
    std::fs::create_dir_all(&model_dir)
        .with_context(|| format!("Failed to create directory: {}", model_dir.display()))?;

    let card_path = model_dir.join("model.toml");
    let mut card = if card_path.exists() {
        ModelCard::load(&card_path)?
    } else {
        let name = repo_id.rsplit('/').next().unwrap_or(repo_id).to_string();
        ModelCard {
            model: ModelMeta {
                name,
                source: repo_id.to_string(),
                default_context_length: Some(8192),
                default_gpu_layers: Some(999),
            },
            sampling: std::collections::HashMap::new(),
            quants: std::collections::HashMap::new(),
        }
    };

    for display_str in &selected {
        let idx = options.iter().position(|o| o == display_str).unwrap();
        let gguf = &ggufs[idx];

        println!();
        println!("  Downloading {}...", gguf.filename);

        let local_path = pull::download_gguf(repo_id, &gguf.filename, &model_dir).await?;

        let size_bytes = std::fs::metadata(&local_path).map(|m| m.len()).ok();
        let quant_name = gguf.quant.clone().unwrap_or_else(|| gguf.filename.clone());

        card.quants.insert(
            quant_name.clone(),
            QuantInfo {
                file: gguf.filename.clone(),
                size_bytes,
                context_length: None,
            },
        );

        println!("  Downloaded: {}", local_path.display());
    }

    card.save(&card_path)?;

    println!();
    println!("Oh yeah, it's all coming together.");
    println!("  Model card saved: {}", card_path.display());
    println!();
    println!("  Create a profile:");
    println!(
        "    kronk model create my-profile --model {} --use-case coding",
        repo_id
    );

    Ok(())
}

fn cmd_ls(config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir);
    let models = registry.scan()?;

    if models.is_empty() {
        println!("No models installed.");
        println!();
        println!("Pull one:  kronk model pull <huggingface-repo>");
        return Ok(());
    }

    println!("Installed models:");
    println!("{}", "-".repeat(60));

    for model in &models {
        println!();
        println!("  {}  ({})", model.id, model.card.model.name);
        if let Some(ctx) = model.card.model.default_context_length {
            print!("    context: {}  ", ctx);
        }
        if let Some(ngl) = model.card.model.default_gpu_layers {
            print!("gpu-layers: {}", ngl);
        }
        println!();

        if model.card.quants.is_empty() {
            println!("    (no quants)");
        } else {
            for (qname, qinfo) in &model.card.quants {
                let size_str = qinfo
                    .size_bytes
                    .map(format_size)
                    .unwrap_or_else(|| "?".to_string());
                println!("    {} -- {} ({})", qname, qinfo.file, size_str);
            }
        }

        let linked_profiles: Vec<&str> = config
            .profiles
            .iter()
            .filter(|(_, p)| p.model.as_deref() == Some(&model.id))
            .map(|(name, _)| name.as_str())
            .collect();
        if !linked_profiles.is_empty() {
            println!("    profiles: {}", linked_profiles.join(", "));
        }

        let untracked = registry
            .untracked_ggufs(&model.dir, &model.card)
            .unwrap_or_default();
        if !untracked.is_empty() {
            println!("    untracked: {}", untracked.join(", "));
        }
    }

    println!();
    Ok(())
}

async fn cmd_ps(config: &Config) -> Result<()> {
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    let model_profiles: Vec<(&str, &kronk_core::config::ProfileConfig)> = config
        .profiles
        .iter()
        .filter(|(_, p)| p.model.is_some())
        .map(|(n, p)| (n.as_str(), p))
        .collect();

    if model_profiles.is_empty() {
        println!("No model-based profiles.");
        println!();
        println!("Create one:  kronk model create <name> --model <id> --use-case coding");
        return Ok(());
    }

    println!("Model processes:");
    println!("{}", "-".repeat(60));

    for (name, profile) in model_profiles {
        let model_id = profile.model.as_deref().unwrap_or("?");
        let quant = profile.quant.as_deref().unwrap_or("?");
        let use_case = profile
            .use_case
            .as_ref()
            .map(|uc| uc.to_string())
            .unwrap_or_else(|| "none".to_string());

        let service_name = Config::service_name(name);
        let service_status = {
            #[cfg(target_os = "windows")]
            {
                kronk_core::platform::windows::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(target_os = "linux")]
            {
                kronk_core::platform::linux::query_service(&service_name)
                    .unwrap_or_else(|_| "UNKNOWN".to_string())
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                let _ = &service_name;
                "N/A".to_string()
            }
        };

        let backend = config.backends.get(&profile.backend);
        let health = if let Some(url) = backend.and_then(|b| b.health_check_url.as_ref()) {
            match http_client.get(url).send().await {
                Ok(resp) if resp.status().is_success() => "HEALTHY",
                _ => "DOWN",
            }
        } else {
            "N/A"
        };

        println!();
        println!("  {}  {} / {}", name, model_id, quant);
        println!(
            "    use-case: {}  service: {}  health: {}",
            use_case, service_status, health
        );
    }

    println!();
    Ok(())
}

async fn cmd_create(
    config: &Config,
    name: &str,
    model_id: &str,
    quant: Option<String>,
    use_case: Option<String>,
) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir);

    let installed = registry.find(model_id)?.with_context(|| {
        format!(
            "Model '{}' not found. Run `kronk model ls` to see installed models.",
            model_id
        )
    })?;

    let quant_name = match quant {
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

    let resolved_use_case = match use_case {
        Some(uc) => {
            use kronk_core::use_cases::UseCase;
            let parsed = match uc.as_str() {
                "coding" => UseCase::Coding,
                "chat" => UseCase::Chat,
                "analysis" => UseCase::Analysis,
                "creative" => UseCase::Creative,
                custom => UseCase::Custom {
                    name: custom.to_string(),
                },
            };
            Some(parsed)
        }
        None => None,
    };

    let gguf_path = registry
        .gguf_path(model_id, &quant_name)?
        .with_context(|| format!("GGUF file for quant '{}' not found on disk", quant_name))?;

    let mut args = vec![
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "-m".to_string(),
        gguf_path.to_string_lossy().to_string(),
    ];

    if let Some(ctx) = installed.card.context_length_for(&quant_name) {
        args.push("-c".to_string());
        args.push(ctx.to_string());
    }

    if let Some(ngl) = installed.card.model.default_gpu_layers {
        args.push("-ngl".to_string());
        args.push(ngl.to_string());
    }

    let mut config = config.clone();
    if config.profiles.contains_key(name) {
        anyhow::bail!(
            "Profile '{}' already exists. Use `kronk update` or choose a different name.",
            name
        );
    }

    let backend_key = config
        .backends
        .keys()
        .next()
        .cloned()
        .context("No backends configured. Add one first with `kronk add`.")?;

    config.profiles.insert(
        name.to_string(),
        kronk_core::config::ProfileConfig {
            backend: backend_key.clone(),
            args,
            use_case: resolved_use_case,
            sampling: None,
            model: Some(model_id.to_string()),
            quant: Some(quant_name.clone()),
        },
    );

    config.save()?;

    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Profile:   {}", name);
    println!("  Model:     {}", model_id);
    println!("  Quant:     {}", quant_name);
    println!("  GGUF:      {}", gguf_path.display());
    if let Some(uc) = &config.profiles[name].use_case {
        println!("  Use case:  {}", uc);
    }
    println!();
    println!("Run it:      kronk run --profile {}", name);
    println!("Install it:  kronk service install --profile {}", name);

    Ok(())
}

fn cmd_rm(config: &Config, model_id: &str) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir);

    let model = registry
        .find(model_id)?
        .with_context(|| format!("Model '{}' not found.", model_id))?;

    let linked_profiles: Vec<&str> = config
        .profiles
        .iter()
        .filter(|(_, p)| p.model.as_deref() == Some(model_id))
        .map(|(name, _)| name.as_str())
        .collect();

    if !linked_profiles.is_empty() {
        anyhow::bail!(
            "Cannot remove '{}': referenced by profiles: {}. Remove those first.",
            model_id,
            linked_profiles.join(", ")
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

    println!("No touchy! Model '{}' removed.", model_id);
    Ok(())
}

fn cmd_scan(config: &Config) -> Result<()> {
    let models_dir = config.models_dir()?;
    let registry = ModelRegistry::new(models_dir.clone());
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
                let quant = pull::infer_quant_from_filename(filename)
                    .unwrap_or_else(|| "unknown".to_string());
                let size_bytes = model.dir.join(filename).metadata().map(|m| m.len()).ok();
                println!("    + {} ({})", filename, quant);
                card.quants.insert(
                    quant,
                    QuantInfo {
                        file: filename.clone(),
                        size_bytes,
                        context_length: None,
                    },
                );
            }
            card.save(&model.dir.join("model.toml"))?;
            found_any = true;
        }
    }

    // Scan for directories with GGUFs but no model.toml
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
                if model_path.join("model.toml").exists() {
                    continue;
                }

                let gguf_files: Vec<String> = std::fs::read_dir(&model_path)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".gguf"))
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();

                if !gguf_files.is_empty() {
                    let model_name = model_entry.file_name().to_string_lossy().to_string();
                    let model_id = format!("{}/{}", company, model_name);
                    println!(
                        "  {} -- new model with {} GGUF file(s):",
                        model_id,
                        gguf_files.len()
                    );

                    let mut quants = std::collections::HashMap::new();
                    for filename in &gguf_files {
                        let quant = pull::infer_quant_from_filename(filename)
                            .unwrap_or_else(|| "unknown".to_string());
                        let size_bytes = model_path.join(filename).metadata().map(|m| m.len()).ok();
                        println!("    + {} ({})", filename, quant);
                        quants.insert(
                            quant,
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
                            source: model_id,
                            default_context_length: Some(8192),
                            default_gpu_layers: Some(999),
                        },
                        sampling: std::collections::HashMap::new(),
                        quants,
                    };
                    card.save(&model_path.join("model.toml"))?;
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

fn format_size(bytes: u64) -> String {
    const GB: u64 = 1_000_000_000;
    const MB: u64 = 1_000_000;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    }
}
