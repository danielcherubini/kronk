//! Profile command handler
//!
//! Handles `koji profile list/set/clear` commands.
//! Profiles are now pure labels into model card `[sampling.<profile>]` sections.

use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
use koji_core::profiles::Profile;

/// Manage sampling profiles — presets for inference params
pub fn cmd_profile(config: &Config, command: crate::cli::ProfileCommands) -> Result<()> {
    match command {
        crate::cli::ProfileCommands::List => {
            println!("Built-in profiles:");
            println!();
            for (name, desc, _profile) in Profile::all() {
                // Show template defaults from config.sampling_templates
                let params = config.sampling_templates.get(name);
                println!("  {}:", name);
                println!("    {}", desc);
                if let Some(p) = params {
                    println!(
                        "    temp={:.1}  top-k={}  top-p={:.2}  min-p={:.2}  presence-penalty={:.1}",
                        p.temperature.unwrap_or(0.0),
                        p.top_k.unwrap_or(0),
                        p.top_p.unwrap_or(0.0),
                        p.min_p.unwrap_or(0.0),
                        p.presence_penalty.unwrap_or(0.0),
                    );
                }
                println!();
            }

            println!("These are seed defaults for new model cards.");
            println!(
                "Per-model overrides live in configs/<model>.toml under [sampling.<profile>]."
            );
            println!();

            // Show which models use which profile
            println!("Model assignments:");
            let db_dir = koji_core::config::Config::config_dir()?;
            let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
            let model_configs = koji_core::db::load_model_configs(&conn)?;

            for (name, srv) in &model_configs {
                // Check if model uses sampling (unified config) or has legacy profile field
                let profile_str = if let Some(sampling) = &srv.sampling {
                    sampling.preset_label().to_string()
                } else if let Some(ref profile) = srv.profile {
                    profile.clone()
                } else {
                    "none".to_string()
                };
                println!("  {} -> {}", name, profile_str);
            }

            Ok(())
        }
        crate::cli::ProfileCommands::Set { server, profile } => {
            let db_dir = koji_core::config::Config::config_dir()?;
            let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
            let mut model_configs = koji_core::db::load_model_configs(&conn)?;

            // Validate server exists
            if !model_configs.contains_key(&server) {
                anyhow::bail!("Model '{}' not found", server);
            }

            // Only accept built-in profile names
            let builtins = ["coding", "chat", "analysis", "creative"];
            if !builtins.contains(&profile.as_str()) {
                anyhow::bail!(
                    "Unknown profile '{}'. Available profiles: {}",
                    profile,
                    builtins.join(", ")
                );
            }

            // Look up the sampling template for this profile
            let template = config
                .sampling_templates
                .get(&profile)
                .ok_or_else(|| anyhow::anyhow!("Profile '{}' not found", profile))?;

            // Set sampling from template
            let srv = model_configs.get_mut(&server).unwrap();
            srv.sampling = Some(template.clone());
            srv.profile = None; // Clear legacy profile field

            koji_core::db::save_model_config(&conn, &server, srv)?;

            println!("Updated.");
            println!("  Model '{}' now uses '{}' preset.", server, profile);

            Ok(())
        }
        crate::cli::ProfileCommands::Clear { server } => {
            let db_dir = koji_core::config::Config::config_dir()?;
            let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
            let mut model_configs = koji_core::db::load_model_configs(&conn)?;
            let srv = model_configs
                .get_mut(&server)
                .with_context(|| format!("Model '{}' not found", server))?;

            srv.sampling = None;
            srv.profile = None;
            koji_core::db::save_model_config(&conn, &server, srv)?;

            println!("Profile cleared for model '{}'.", server);
            Ok(())
        }
    }
}
