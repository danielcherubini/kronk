use anyhow::{Context, Result};
use kronk_core::config::Config;
/// Add a new server from a command line, extracting kronk flags.
pub async fn cmd_server_add(
    config: &Config,
    name: &str,
    command: Vec<String>,
    overwrite: bool,
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

    // Extract kronk flags from args
    let extracted = crate::flags::extract_kronk_flags(args.clone())?;

    // Check for duplicate server
    if config.models.contains_key(name) && !overwrite {
        anyhow::bail!(
            "Server '{}' already exists. Use `kronk server edit` to modify it.",
            name
        );
    }

    // Resolve model card if model ref is provided
    let model_info = if let Some(ref model_ref) = extracted.model {
        let models_dir = config.models_dir()?;
        let configs_dir = config.configs_dir()?;
        let registry = kronk_core::models::ModelRegistry::new(
            models_dir.to_path_buf(),
            configs_dir.to_path_buf(),
        );

        match registry.find(model_ref) {
            Ok(Some(installed)) => Some(installed),
            Ok(None) => {
                anyhow::bail!(
                    "Model '{}' not found. Use `kronk model pull <repo>` to install it first.",
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
                "No quants available for '{}'. Run `kronk model pull {}` first.",
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

    // Parse profile if provided
    let profile = extracted
        .profile
        .as_ref()
        .map(|s| {
            s.parse::<kronk_core::profiles::Profile>()
                .map_err(|e| anyhow::anyhow!(e))
        })
        .transpose()?;

    // Build ModelConfig
    let model_config = kronk_core::config::ModelConfig {
        backend: backend_key.clone(),
        args: extracted.remaining_args.clone(),
        profile,
        sampling: None,
        model: extracted.model.clone(),
        quant: quant_name,
        port: extracted.port,
        health_check: None,
        enabled: true,
        source: model_info.as_ref().map(|m| m.card.model.source.clone()),
        context_length: extracted.context_length,
    };

    config.models.insert(name.to_string(), model_config.clone());
    config.save()?;

    // Output
    println!("Oh yeah, it's all coming together.");
    println!();
    println!("  Model:    {}", name);
    println!("  Backend:  {} ({})", backend_key, exe_str);

    if let Some(ref model) = model_config.model {
        let quant = model_config.quant.as_deref().unwrap_or("?");
        let source = model_config.source.as_deref().unwrap_or(model);
        println!("  Model:    {} ({})", source, quant);
    }

    if let Some(ref profile) = model_config.profile {
        println!("  Profile:  {}", profile);
    }

    if let Some(ref quant) = model_config.quant {
        if let Some(ref model) = model_config.model {
            let models_dir = config.models_dir()?;
            let configs_dir = config.configs_dir()?;
            let registry = kronk_core::models::ModelRegistry::new(
                models_dir.to_path_buf(),
                configs_dir.to_path_buf(),
            );
            if let Ok(Some(installed)) = registry.find(model) {
                if let Some(q) = installed.card.quants.get(quant) {
                    println!("  GGUF:     {}", q.file);
                }
            }
        }
    }

    println!();
    println!("Enable it:  kronk model enable {}", name);
    println!("Start:      kronk serve");

    Ok(())
}
