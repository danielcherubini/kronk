use anyhow::{Context, Result};
use tama_core::config::Config;
use tama_core::models::ModelManager;

pub(super) fn cmd_enable(_config: &Config, name: &str) -> Result<()> {
    let db_dir = tama_core::config::Config::config_dir()?;
    let mgr = ModelManager::open(&db_dir)?;

    // Verify the config exists before enabling
    let repo_id = tama_core::db::config_key_to_repo_id(name);
    mgr.get_config_by_repo_id(&repo_id)
        .with_context(|| format!("Failed to lookup model '{}' in database", name))?
        .with_context(|| format!("Model '{}' not found", name))?;

    mgr.enable_model(name)
        .with_context(|| format!("Failed to enable model '{}'", name))?;
    println!("Enabled model: {}", name);
    Ok(())
}

pub(super) fn cmd_disable(_config: &Config, name: &str) -> Result<()> {
    let db_dir = tama_core::config::Config::config_dir()?;
    let mgr = ModelManager::open(&db_dir)?;

    // Verify the config exists before disabling
    let repo_id = tama_core::db::config_key_to_repo_id(name);
    mgr.get_config_by_repo_id(&repo_id)
        .with_context(|| format!("Failed to lookup model '{}' in database", name))?
        .with_context(|| format!("Model '{}' not found", name))?;

    mgr.disable_model(name)
        .with_context(|| format!("Failed to disable model '{}'", name))?;
    println!("Disabled model: {}", name);
    Ok(())
}
