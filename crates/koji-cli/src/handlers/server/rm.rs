use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;

/// Remove a server
pub fn cmd_server_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    let db_dir = config
        .loaded_from
        .clone()
        .unwrap_or_else(|| koji_core::config::Config::config_dir().unwrap());
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    let model_configs = koji_core::db::load_model_configs(&conn)?;

    if !model_configs.contains_key(name) {
        anyhow::bail!("Server '{}' not found.", name);
    }

    // Check if a service is installed for this server
    let service_name = Config::service_name(name);
    let service_installed = {
        #[cfg(target_os = "windows")]
        {
            koji_core::platform::windows::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(target_os = "linux")]
        {
            koji_core::platform::linux::auto_query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = &service_name;
            false
        }
    };

    if service_installed {
        anyhow::bail!(
            "Server '{}' has an installed service '{}'. Remove it first with: koji service remove {}",
            name, service_name, name
        );
    }

    if !force {
        let confirm = inquire::Confirm::new(&format!("Remove model '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Remove from DB
    let all_configs = koji_core::db::queries::get_all_model_configs(&conn)?;
    let model_id = all_configs.iter().find(|c| c.repo_id == name).map(|c| c.id);
    if let Some(id) = model_id {
        koji_core::db::queries::delete_model_config(&conn, id)?;
    }

    println!("Model '{}' removed.", name);
    Ok(())
}
