use anyhow::{anyhow, Context, Result};
use tama_core::backends::{safe_remove_installation, BackendInfo, BackendManager};

use super::parse::registry_config_dir;

pub async fn cmd_remove(
    _config: &tama_core::config::Config,
    name: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    let mgr = BackendManager::open(&registry_config_dir()?)?;

    // Get all versions to determine what we're removing
    let all_versions = mgr.list_versions(name, gpu_variant)?.ok_or_else(|| {
        anyhow!(
            "Backend '{}' not found. Run `tama backend list` to see installed backends.",
            name
        )
    })?;

    if all_versions.is_empty() {
        anyhow::bail!("No versions found for backend '{}'", name);
    }

    // Show what will be removed
    if let Some(variant) = gpu_variant {
        println!("Removing backend '{}' [{}]:", name, variant);
    } else {
        let variants: std::collections::HashSet<&str> = all_versions
            .iter()
            .map(|v| v.gpu_variant.as_str())
            .collect();
        println!("Removing backend '{}' (all variants):", name);
        for variant in &variants {
            println!("  - [{}]", variant);
        }
    }

    for v in &all_versions {
        println!("  Version: {} ({})", v.version, v.path.display());
    }

    let confirm = inquire::Confirm::new("Are you sure?")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // Optionally remove files for all variants
    let remove_files = inquire::Confirm::new("Also delete the backend files from disk?")
        .with_default(true)
        .prompt()?;

    if remove_files {
        // Iterate all versions and delete each — abort on first failure
        for v in &all_versions {
            if v.path.exists() {
                let info = BackendInfo {
                    name: v.name.clone(),
                    backend_type: v.backend_type.clone(),
                    version: v.version.clone(),
                    path: v.path.clone(),
                    installed_at: v.installed_at,
                    gpu_type: v.gpu_type.clone(),
                    gpu_variant: v.gpu_variant.clone(),
                    source: v.source.clone(),
                };
                // Use the shared safe_remove_installation helper which handles:
                // - Path validation (prevents directory traversal attacks)
                // - Windows PermissionDenied retry logic
                // - Cross-platform file removal
                safe_remove_installation(&info).with_context(|| {
                    format!("Failed to remove files for {} version {}", name, v.version)
                })?;
            }
        }
    }

    // Remove from registry only after all file deletions succeeded
    mgr.delete_all_versions(name, gpu_variant)?;

    println!("Backend '{}' removed.", name);
    Ok(())
}

pub async fn cmd_remove_version(
    _config: &tama_core::config::Config,
    name: &str,
    version: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    let mgr = BackendManager::open(&registry_config_dir()?)?;

    // Get all versions for this backend
    let all_versions = mgr.list_versions(name, None)?.ok_or_else(|| {
        anyhow!(
            "Backend '{}' not found. Run `tama backend list` to see installed backends.",
            name
        )
    })?;

    // Determine the gpu_variant to use
    let gpu_variant = match gpu_variant {
        Some(v) => v.to_string(),
        None => {
            // Auto-infer: find variants that have the requested version
            let matching: Vec<&BackendInfo> = all_versions
                .iter()
                .filter(|v| v.version == version)
                .collect();
            if matching.is_empty() {
                let available: Vec<String> =
                    all_versions.iter().map(|v| v.version.clone()).collect();
                anyhow::bail!(
                    "Version '{}' not found for backend '{}'. Available: {}",
                    version,
                    name,
                    available.join(", ")
                );
            }

            // Get unique variants for this version
            let variants: std::collections::HashSet<&str> =
                matching.iter().map(|v| v.gpu_variant.as_str()).collect();

            if variants.len() == 1 {
                variants.into_iter().next().unwrap().to_string()
            } else {
                let variant_list: Vec<&str> = variants.into_iter().collect();
                anyhow::bail!(
                    "Multiple variants exist for '{}' version '{}'. Use --gpu to specify ({})",
                    name,
                    version,
                    variant_list.join(", ")
                );
            }
        }
    };

    // Find the specific version record
    let record = all_versions
        .iter()
        .find(|v| v.version == version && v.gpu_variant == gpu_variant)
        .ok_or_else(|| {
            anyhow!(
                "Backend '{}' version '{}' [{}] not found",
                name,
                version,
                gpu_variant
            )
        })?;

    println!(
        "Removing backend '{}' [{}] version '{}'",
        name, gpu_variant, version
    );
    println!("  Path: {}", record.path.display());

    let confirm = inquire::Confirm::new("Are you sure? This will delete the backend files.")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // STEP 1: Delete files FIRST (before any DB changes)
    if record.path.exists() {
        safe_remove_installation(record)?;
    }

    // STEP 2: Remove from registry (activates another version if this was active)
    mgr.remove_version(name, &gpu_variant, version)?;

    println!("Version '{}' [{}] removed.", version, gpu_variant);

    Ok(())
}
