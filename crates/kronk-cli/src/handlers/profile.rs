//! Profile command handler
//!
//! Handles `kronk profile list/set/clear/add/remove` commands using the new model card-based approach.

use anyhow::{Context, Result};
use kronk_core::config::Config;
use kronk_core::models::card::ModelCard;
use kronk_core::profiles::Profile;
use std::path::PathBuf;

/// Resolve a profile name to either a built-in profile or a custom profile from model cards.
fn resolve_profile(config: &Config, name: &str) -> Result<Profile> {
    // Built-in profiles
    let builtins = ["coding", "chat", "analysis", "creative"];
    if builtins.contains(&name) {
        return Ok(match name {
            "coding" => Profile::Coding,
            "chat" => Profile::Chat,
            "analysis" => Profile::Analysis,
            "creative" => Profile::Creative,
            _ => unreachable!(),
        });
    }

    // Check custom profiles from config
    if let Some(custom) = &config.custom_profiles {
        if custom.contains_key(name) {
            return Ok(Profile::Custom {
                name: name.to_string(),
            });
        }
    }

    // Check model cards for custom profiles
    let models_dir = config
        .general
        .models_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models.d"));
    let configs_dir = config
        .configs_dir()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("configs.d"));

    let registry = kronk_core::models::ModelRegistry::new(models_dir, configs_dir);
    let installed = registry
        .find("*") // Find any installed model
        .ok()
        .flatten();

    if let Some(installed) = installed {
        let model_card = ModelCard::load(installed.card_path)?;
        if let Some(sampling) = model_card.sampling_for(name) {
            return Ok(Profile::Custom {
                name: name.to_string(),
            });
        }
    }

    anyhow::bail!(
        "Unknown profile '{}'. Use `kronk profile list` to see available options, \
         or `kronk profile add {}` to create a custom one.",
        name,
        name
    );
}

/// Manage sampling profiles — presets for inference params
pub fn cmd_profile(config: &Config, command: crate::cli::ProfileCommands) -> Result<()> {
    match command {
        crate::cli::ProfileCommands::List => {
            // Load profiles from model cards
            let configs_dir = config
                .configs_dir()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("configs.d"));
            let models_dir = config
                .general
                .models_dir
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("models.d"));

            let registry = kronk_core::models::ModelRegistry::new(models_dir, configs_dir);
            let installed = registry
                .find("*") // Find any installed model
                .ok()
                .flatten();

            let mut disk_profiles = std::collections::HashMap::new();
            if let Some(installed) = installed {
                if let Ok(card) = ModelCard::load(installed.card_path) {
                    for (name, sampling) in &card.sampling {
                        disk_profiles.insert(name.clone(), sampling.clone());
                    }
                }
            }

            // Load custom profiles from config
            let mut custom_profiles = std::collections::HashMap::new();
            if let Some(custom) = &config.custom_profiles {
                for (name, sampling) in custom {
                    custom_profiles.insert(name.clone(), sampling.clone());
                }
            }

            // Merge disk and custom profiles
            for (name, sampling) in disk_profiles {
                custom_profiles
                    .entry(name)
                    .or_insert_with(|| sampling.clone());
            }

            println!("Available profiles:");
            println!();
            for (name, desc, profile) in Profile::all() {
                let params = disk_profiles
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| profile.params());
                println!("  {}:", name);
                println!("    {}", desc);
                println!(
                    "    temp={:.1}  top-k={}  top-p={:.2}  min-p={:.2}  presence-penalty={:.1}",
                    params.temperature.unwrap_or(0.0),
                    params.top_k.unwrap_or(0),
                    params.top_p.unwrap_or(0.0),
                    params.min_p.unwrap_or(0.0),
                    params.presence_penalty.unwrap_or(0.0),
                );
                if disk_profiles.contains_key(name) {
                    println!("    (loaded from model card)");
                }
                println!();
            }

            // Show additional custom profiles from model cards
            for (name, params) in &custom_profiles {
                if !Profile::all().iter().any(|(n, _, _)| *n == name.as_str()) {
                    println!("  {} (custom from model card):", name);
                    let args = params.to_args().join(" ");
                    println!(
                        "    {}",
                        if args.is_empty() {
                            "(default params)".to_string()
                        } else {
                            args
                        }
                    );
                    println!();
                }
            }

            // Show custom profiles from config
            if !custom_profiles.is_empty() {
                println!("Custom profiles (from config):");
                println!();
                for (name, params) in &custom_profiles {
                    println!("  {}:", name);
                    let args = params.to_args().join(" ");
                    println!(
                        "    {}",
                        if args.is_empty() {
                            "(default params)".to_string()
                        } else {
                            args
                        }
                    );
                    println!();
                }
            }

            // Show which models use which profile
            println!("Model assignments:");
            for (name, srv) in &config.models {
                let profile_str = srv
                    .profile
                    .as_ref()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "none".to_string());
                println!("  {} -> {}", name, profile_str);
            }

            Ok(())
        }
        crate::cli::ProfileCommands::Set { server, profile } => {
            let mut config = config.clone();

            // Validate server exists
            if !config.models.contains_key(&server) {
                anyhow::bail!("Model '{}' not found", server);
            }

            // Resolve profile name
            let resolved = resolve_profile(&config, &profile)?;

            config.models.get_mut(&server).unwrap().profile = Some(resolved);
            config.save()?;

            println!("Oh yeah, it's all coming together.");
            println!("  Model '{}' now uses '{}' preset.", server, profile);

            Ok(())
        }
        crate::cli::ProfileCommands::Clear { server } => {
            let mut config = config.clone();
            let srv = config
                .models
                .get_mut(&server)
                .with_context(|| format!("Model '{}' not found", server))?;

            srv.profile = None;
            config.save()?;

            println!("Profile cleared for model '{}'.", server);
            Ok(())
        }
        crate::cli::ProfileCommands::Add {
            name,
            temp,
            top_k,
            top_p,
            min_p,
            presence_penalty,
            frequency_penalty,
            repeat_penalty,
        } => {
            let mut config = config.clone();
            let params = kronk_core::profiles::SamplingParams {
                temperature: temp,
                top_k,
                top_p,
                min_p,
                presence_penalty,
                frequency_penalty,
                repeat_penalty,
            };

            if params.is_empty() {
                anyhow::bail!("At least one sampling parameter is required. Example:\n  kronk profile add my-preset --temp 0.4 --top-k 30");
            }

            // Reject names that shadow built-in profiles
            const RESERVED: &[&str] = &["coding", "chat", "analysis", "creative"];
            if RESERVED.contains(&name.as_str()) {
                anyhow::bail!(
                    "Cannot create custom profile '{}': it shadows a built-in profile. \
                     Edit configs.d/{}.toml instead to customize it.",
                    name,
                    name
                );
            }

            let custom = config
                .custom_profiles
                .get_or_insert_with(std::collections::HashMap::new);
            custom.insert(name.clone(), params);
            config.save()?;

            println!("Custom profile '{}' created.", name);
            println!("Assign it: kronk profile set <model> {}", name);
            Ok(())
        }
        crate::cli::ProfileCommands::Remove { name } => {
            let mut config = config.clone();

            let exists = config
                .custom_profiles
                .as_ref()
                .map(|m| m.contains_key(&name))
                .unwrap_or(false);
            if !exists {
                anyhow::bail!("Custom profile '{}' not found", name);
            }

            // Check if any servers reference this profile
            let referencing: Vec<&str> = config
                .models
                .iter()
                .filter(|(_, s)| {
                    matches!(
                        &s.profile,
                        Some(Profile::Custom { name: n }) if n == &name
                    )
                })
                .map(|(k, _)| k.as_str())
                .collect();
            if !referencing.is_empty() {
                anyhow::bail!(
                    "Cannot remove profile '{}': referenced by servers: {}. \
                     Clear them first with `kronk profile clear <server>`.",
                    name,
                    referencing.join(", ")
                );
            }

            config.custom_profiles.as_mut().unwrap().remove(&name);
            config.save()?;
            println!("Custom profile '{}' removed.", name);
            Ok(())
        }
    }
}
