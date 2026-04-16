use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
/// Add a new server from a command line, extracting koji flags.
pub async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    _overwrite: bool,
) -> Result<()> {
    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    // Split exe from args
    let exe_path = &command[0];
    let args: Vec<String> = command[1..].to_vec();

    // Resolve backend path
    let mut config = config.clone();
    let (backend_key, exe_str) = super::resolve_backend(&mut config, exe_path)?;

    // Extract koji flags from args
    let extracted = crate::flags::extract_koji_flags(args)?;

    // Check for duplicate server — use the same config_dir the Config was loaded from
    let db_dir = config
        .loaded_from
        .clone()
        .unwrap_or_else(|| koji_core::config::Config::config_dir().unwrap());
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    // Note: get_model_config now takes integer ID, so we skip this pre-check
    // The database will reject duplicate inserts

    // Resolve model card if model ref is provided
    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry =
        koji_core::models::ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());

    let model_info = if let Some(ref model_ref) = extracted.model {
        match registry.find(model_ref) {
            Ok(Some(installed)) => Some(installed),
            Ok(None) => {
                anyhow::bail!(
                    "Model '{}' not found. Use `koji model pull <repo>` to install it first.",
                    model_ref
                );
            }
            Err(e) => {
                anyhow::bail!("Failed to look up model '{}': {}", model_ref, e);
            }
        }
    } else {
        None
    };

    // Resolve quantization with interactive selection if needed
    let quant_name = if let Some(ref quant) = extracted.quant {
        // If model card exists, validate quant against available quants
        if let Some(ref installed) = model_info {
            if !installed.card.quants.contains_key(quant) {
                let available: Vec<_> = installed.card.quants.keys().collect();
                anyhow::bail!(
                    "Quant '{}' not found in model '{}'. Available quants: {}",
                    quant,
                    installed.card.model.source,
                    available
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Some(quant.clone())
        } else {
            // No model card - quant is just stored as-is
            Some(quant.clone())
        }
    } else if let Some(ref installed) = model_info {
        // Model ref provided but no --quant - offer interactive selection
        let quant_names: Vec<String> = installed.card.quants.keys().cloned().collect();
        if quant_names.is_empty() {
            anyhow::bail!(
                "No quants available for '{}'. Run `koji model pull {}` first.",
                installed.card.model.source,
                installed.card.model.source
            );
        }
        Some(if quant_names.len() == 1 {
            quant_names.into_iter().next().unwrap()
        } else {
            inquire::Select::new("Select a quant:", quant_names)
                .prompt()
                .context("Quant selection cancelled")?
        })
    } else {
        None
    };

    // Verify GGUF file exists if both model card and quant are specified
    if let Some(ref quant_name) = quant_name {
        if let Some(ref installed) = model_info {
            let gguf_path = installed
                .card
                .quants
                .get(quant_name)
                .map(|q| installed.dir.join(&q.file));

            if let Some(ref path) = gguf_path {
                if !path.exists() {
                    anyhow::bail!(
                        "GGUF file '{}' not found. Make sure the model is properly installed.",
                        path.display()
                    );
                }
            }
        }
    }

    // Parse profile if provided and look up sampling template
    let sampling = if let Some(ref profile_name) = extracted.profile {
        config
            .sampling_templates
            .get(profile_name)
            .cloned()
            .or_else(|| {
                tracing::warn!(
                    "Unknown profile '{}' not found in sampling_templates",
                    profile_name
                );
                None
            })
    } else {
        None
    };

    // Build ModelConfig
    let model_config = koji_core::config::ModelConfig {
        backend: backend_key.clone(),
        args: extracted.remaining_args.clone(),
        profile: extracted.profile.clone(), // Keep for migration compatibility
        sampling,
        model: extracted.model.clone(),
        quant: quant_name,
        mmproj: None,
        port: extracted.port,
        health_check: None,
        enabled: true,
        context_length: extracted.context_length,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        db_id: None,
    };

    koji_core::db::save_model_config(&conn, name, &model_config)?;

    // Output
    println!("Server added successfully.");
    println!();
    println!("  Model:    {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = model_config.model {
        let quant = model_config.quant.as_deref().unwrap_or("?");
        println!("  Model:    {} ({})", model, quant);
    }

    if let Some(sampling) = &model_config.sampling {
        println!("  Profile:  {}", sampling.preset_label());
    }

    // Use single registry for both lookups
    if let Some(model) = &model_config.model {
        if let Some(quant) = &model_config.quant {
            if let Ok(Some(installed)) = registry.find(model) {
                if let Some(q) = installed.card.quants.get(quant) {
                    println!("  GGUF:     {}", q.file);
                }
            }
        }
    }

    println!();
    println!("Enable it:  koji model enable {}", name);
    println!("Start:      koji serve");

    Ok(())
}
