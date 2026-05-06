use anyhow::{anyhow, Result};
use tama_core::backends::{
    backends_dir, check_updates, get_backend_install_path, update_backend, BackendSource,
    BackendType, InstallOptions,
};

use super::parse::registry_config_dir;

pub async fn cmd_update(
    _config: &tama_core::config::Config,
    name: &str,
    force: bool,
) -> Result<()> {
    let mut registry = tama_core::backends::BackendRegistry::open(&registry_config_dir()?)?;

    // Find the active backend by listing all active backends
    let active_backends = registry.list()?;
    let matches: Vec<_> = active_backends.iter().filter(|b| b.name == name).collect();

    let backend_info = match matches.len() {
        0 => Err(anyhow!(
            "Backend '{}' not found. Run `tama backend list` to see installed backends.",
            name
        )),
        1 => Ok(matches[0].clone()),
        _ => {
            // Multiple variants active — show them and ask user to specify
            let variants: Vec<String> = matches.iter().map(|b| b.gpu_variant.clone()).collect();
            Err(anyhow!(
                "Backend '{}' has multiple active variants: {}. Please specify which variant to update.",
                name,
                variants.join(", ")
            ))
        }
    }?;

    println!("Checking for updates to '{}'...", name);
    let update_check = check_updates(&backend_info).await?;

    if !update_check.update_available && !force {
        println!(
            "'{}' is already up to date ({})",
            name, backend_info.version
        );
        return Ok(());
    }

    if force && !update_check.update_available {
        println!(
            "Force reinstalling '{}' (already at latest: {})",
            name, backend_info.version
        );
    } else {
        println!("Update available:");
        println!("  Current: {}", update_check.current_version);
        println!("  Latest:  {}", update_check.latest_version);
    }

    if !force {
        let confirm = inquire::Confirm::new("Proceed with update?")
            .with_default(true)
            .prompt()?;

        if !confirm {
            println!("Update cancelled.");
            return Ok(());
        }
    }

    // Use the versioned path structure for the update target
    let target_dir = get_backend_install_path(
        &backends_dir()?,
        &backend_info.backend_type,
        &backend_info.gpu_variant,
        &update_check.latest_version,
    );

    // Preserve the original installation method, but update the version.
    // On update we always go to latest, so we clear any pinned commit.
    let source = match backend_info.source.clone() {
        Some(source) => match source {
            BackendSource::Prebuilt { version: _ } => BackendSource::Prebuilt {
                version: update_check.latest_version.clone(),
            },
            BackendSource::SourceCode {
                version: _,
                git_url,
                commit: _,
            } => BackendSource::SourceCode {
                version: update_check.latest_version.clone(),
                git_url,
                commit: None,
            },
        },
        None => {
            // Fallback for existing backends without source info
            match backend_info.backend_type {
                BackendType::IkLlama => BackendSource::SourceCode {
                    version: update_check.latest_version.clone(),
                    git_url: "https://github.com/ikawrakow/ik_llama.cpp.git".to_string(),
                    commit: None,
                },
                BackendType::LlamaCpp => BackendSource::Prebuilt {
                    version: update_check.latest_version.clone(),
                },
                BackendType::TtsKokoro => {
                    return Err(anyhow!("Cannot update TTS backends via this command"))
                }
                BackendType::Custom => return Err(anyhow!("Cannot update custom backends")),
            }
        }
    };

    let options = InstallOptions {
        backend_type: backend_info.backend_type.clone(),
        source,
        target_dir,
        gpu_type: backend_info.gpu_type.clone(),
        gpu_variant: backend_info.gpu_variant.clone(),
        allow_overwrite: true,
    };

    update_backend(
        &mut registry,
        name,
        &backend_info.gpu_variant,
        options,
        update_check.latest_version,
    )
    .await?;

    Ok(())
}
