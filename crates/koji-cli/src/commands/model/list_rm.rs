use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;
use koji_core::models::ModelRegistry;

pub(super) fn cmd_ls(
    config: &Config,
    model_id_arg: Option<String>,
    _quant_arg: Option<String>,
    _profile_arg: Option<String>,
) -> Result<()> {
    let models_dir = config.models_dir()?;
    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    let model_configs = koji_core::db::load_model_configs(&conn)?;

    match model_id_arg {
        None => {
            if model_configs.is_empty() {
                println!("No models configured. Run `koji model pull <repo>` to add one.");
                return Ok(());
            }

            let mut entries: Vec<(&String, &koji_core::config::ModelConfig)> =
                model_configs.iter().collect();
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
            let mc = model_configs.get(&model_id).with_context(|| {
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

pub(super) fn cmd_rm(config: &Config, model_id: &str) -> Result<()> {
    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());

    let model = registry
        .find(model_id)?
        .with_context(|| format!("Model '{}' not found.", model_id))?;

    // Check for referencing model configurations in DB
    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    let model_configs = koji_core::db::load_model_configs(&conn)?;
    let linked_configs: Vec<&str> = model_configs
        .iter()
        .filter(|(_, p)| p.model.as_deref() == Some(model_id))
        .map(|(name, _)| name.as_str())
        .collect();

    if !linked_configs.is_empty() {
        anyhow::bail!(
            "Cannot remove '{}': referenced by model configurations: {}. Remove those first.",
            model_id,
            linked_configs.join(", ")
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

    // Clean up DB metadata (best-effort)
    let repo_key = if model.card.model.source.is_empty() {
        &model.id
    } else {
        &model.card.model.source
    };
    // Look up model_id for DB deletion
    if let Some(record) = koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_key)? {
        let _ = koji_core::db::queries::delete_model_records(&conn, record.id);
    }

    println!("Removed model '{}'.", model_id);
    Ok(())
}
