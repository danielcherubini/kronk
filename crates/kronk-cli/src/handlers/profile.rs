//! Profile command handler
//!
//! Handles `kronk profile list/set/clear/add/remove` commands.

use anyhow::{Context, Result};
use kronk_core::config::Config;
use kronk_core::profiles::Profile;

/// Manage sampling profiles — presets for inference params
pub fn cmd_profile(config: &Config, command: crate::cli::ProfileCommands) -> Result<()> {
    match command {
        crate::cli::ProfileCommands::List => {
            // Load profiles from disk
            let profiles_dir = config.profiles_dir()?;
            let disk_profiles = kronk_core::profiles::load_profiles_d(&profiles_dir)
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "Failed to load profiles from {}: {}",
                        profiles_dir.display(),
                        e
                    );
                    std::collections::HashMap::new()
                });

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
                    println!("    (loaded from profiles.d/{}.toml)", name);
                }
                println!();
            }

            // Show additional custom profiles from disk
            for (name, params) in &disk_profiles {
                if !Profile::all().iter().any(|(n, _, _)| *n == name.as_str()) {
                    println!("  {} (custom from profiles.d/):", name);
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
            if let Some(custom) = &config.custom_profiles {
                if !custom.is_empty() {
                    println!("Custom profiles (from config):");
                    println!();
                    for (name, params) in custom {
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

            // Resolve profile name before mutable borrow
            let resolved = match profile.as_str() {
                "coding" => Profile::Coding,
                "chat" => Profile::Chat,
                "analysis" => Profile::Analysis,
                "creative" => Profile::Creative,
                name => {
                    // Check config custom_profiles and profiles.d/
                    let is_custom_config = config
                        .custom_profiles
                        .as_ref()
                        .map(|m| m.contains_key(name))
                        .unwrap_or(false);
                    let is_custom_disk = config
                        .profiles_dir()
                        .ok()
                        .and_then(|dir| kronk_core::profiles::load_profiles_d(&dir).ok())
                        .map(|m| m.contains_key(name))
                        .unwrap_or(false);
                    if !is_custom_config && !is_custom_disk {
                        anyhow::bail!(
                            "Unknown profile '{}'. Use `kronk profile list` to see available options, \
                             or `kronk profile add {}` to create a custom one.",
                            name, name
                        );
                    }
                    Profile::Custom {
                        name: name.to_string(),
                    }
                }
            };

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
                     Edit profiles.d/{}.toml instead to customize it.",
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
                .filter(
                    |(_, s)| matches!(&s.profile, Some(Profile::Custom { name: n }) if n == &name),
                )
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
