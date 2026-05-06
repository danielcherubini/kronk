use anyhow::{anyhow, Result};
use tama_core::backends::{BackendInfo, BackendRegistry};

use super::parse::registry_config_dir;

pub async fn cmd_switch(
    _config: &tama_core::config::Config,
    name: &str,
    version: &str,
    gpu_variant: Option<&str>,
) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Get all versions for this backend
    let all_versions = registry.list_all_versions(name, None)?.ok_or_else(|| {
        anyhow!(
            "Backend '{}' not found. Run `tama backend list` to see installed backends.",
            name
        )
    })?;

    // Determine the gpu_variant to use
    let gpu_variant = match gpu_variant {
        Some(v) => v.to_string(),
        None => {
            // Auto-infer: find unique variants that have the requested version
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

    // Verify the version exists for the specified variant
    let version_record = all_versions
        .iter()
        .find(|v| v.version == version && v.gpu_variant == gpu_variant);
    if version_record.is_none() {
        let available: Vec<String> = all_versions
            .iter()
            .filter(|v| v.gpu_variant == gpu_variant)
            .map(|v| v.version.clone())
            .collect();
        anyhow::bail!(
            "Version '{}' not found for backend '{}' [{}]. Available: {}",
            version,
            name,
            gpu_variant,
            if available.is_empty() {
                "(none)".to_string()
            } else {
                available.join(", ")
            }
        );
    }

    // Activate the version
    let activated = registry.activate(name, &gpu_variant, version)?;
    if !activated {
        anyhow::bail!("Failed to activate version '{}' [{}]", version, gpu_variant);
    }

    println!(
        "Activated backend '{}' [{}] version '{}'.",
        name, gpu_variant, version
    );

    Ok(())
}
