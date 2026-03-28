use anyhow::{Context, Result};
use kronk_core::config::Config;
/// Remove a server
pub fn cmd_server_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    if !config.models.contains_key(name) {
        anyhow::bail!("Server '{}' not found.", name);
    }

    // Check if a service is installed for this server
    let service_name = Config::service_name(name);
    let service_installed = {
        #[cfg(target_os = "windows")]
        {
            kronk_core::platform::windows::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(target_os = "linux")]
        {
            kronk_core::platform::linux::query_service(&service_name)
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
            "Server '{}' has an installed service '{}'. Remove it first with: kronk service remove {}",
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

    let mut config = config.clone();
    config.models.remove(name);
    config.save()?;

    println!("Model '{}' removed.", name);
    Ok(())
}
