use anyhow::Result;
use tama_core::backends::{check_updates, BackendInfo, BackendRegistry};
use tama_core::gpu;

use super::parse::registry_config_dir;

pub async fn cmd_list(_config: &tama_core::config::Config) -> Result<()> {
    let registry = BackendRegistry::open(&registry_config_dir()?)?;
    let active_backends = registry.list()?;

    if active_backends.is_empty() {
        println!("No backends installed.");
        println!("\nTo install one:");
        println!("  tama backend install llama_cpp");
        println!("  tama backend install ik_llama");
        return Ok(());
    }

    // Group by backend name, then by variant
    println!("Installed backends:\n");

    // Track unique backend names to avoid duplicates
    let mut seen_names = std::collections::HashSet::new();
    for backend in &active_backends {
        if !seen_names.insert(&backend.name) {
            continue;
        }

        // Get all versions for this backend
        let all_versions = registry
            .list_all_versions(&backend.name, None)
            .unwrap_or(None);

        if let Some(versions) = all_versions {
            // Group versions by gpu_variant
            let mut variants: std::collections::HashMap<&str, Vec<&BackendInfo>> =
                std::collections::HashMap::new();
            for v in &versions {
                variants.entry(&v.gpu_variant).or_default().push(v);
            }

            for (variant, variant_versions) in variants.iter() {
                let active_version = active_backends
                    .iter()
                    .find(|b| b.name == backend.name && b.gpu_variant == *variant)
                    .map(|b| b.version.as_str());

                for v in variant_versions {
                    let marker = if active_version == Some(v.version.as_str()) {
                        " * active"
                    } else {
                        ""
                    };
                    println!("  {} [{}]{} (v{})", v.name, variant, marker, v.version);
                    println!("    Version:  {}", v.version);
                    println!("    Path:     {}", v.path.display());
                    if let Some(ref gpu) = v.gpu_type {
                        println!("    GPU:      {:?}", gpu);
                    }
                    println!();
                }
            }
        } else {
            // Fallback if list_all_versions fails
            println!(
                "  {} [{}] * active (v{})",
                backend.name, backend.gpu_variant, backend.version
            );
            println!("    Version:  {}", backend.version);
            println!("    Path:     {}", backend.path.display());
            if let Some(ref gpu) = backend.gpu_type {
                println!("    GPU:      {:?}", gpu);
            }
            println!();
        }
    }

    // Tip using first backend as example
    if let Some(first) = active_backends.first() {
        println!("To pin a version in config.toml, add:");
        println!("  [backends.{}]", first.name);
        println!("  version = \"{}\"", first.version);
    }

    Ok(())
}

pub async fn cmd_check_updates(_config: &tama_core::config::Config) -> Result<()> {
    let registry = BackendRegistry::open(&registry_config_dir()?)?;
    let backends = registry.list()?;

    if backends.is_empty() {
        println!("No backends installed.");
        return Ok(());
    }

    println!("Checking for updates...\n");

    for backend in backends {
        print!("  {} ({}): ", backend.name, backend.version);

        match check_updates(&backend).await {
            Ok(check) => {
                if check.update_available {
                    println!("UPDATE AVAILABLE -> {}", check.latest_version);
                } else {
                    println!("up to date");
                }
            }
            Err(e) => {
                eprintln!("error: {}", e);
            }
        }
    }

    Ok(())
}

struct VersionEntry {
    name: String,
    version: String,
    path: std::path::PathBuf,
    gpu_type: Option<gpu::GpuType>,
    gpu_variant: String,
    is_active: bool,
}

pub async fn cmd_all_versions(
    _config: &tama_core::config::Config,
    name: Option<&str>,
) -> Result<()> {
    let registry = BackendRegistry::open(&registry_config_dir()?)?;
    let active_backends = registry.list()?;

    if active_backends.is_empty() {
        println!("No backends installed.");
        return Ok(());
    }

    let mut entries: Vec<VersionEntry> = Vec::new();

    if let Some(target_name) = name {
        // Show all versions for a specific backend
        match registry.list_all_versions(target_name, None)? {
            Some(versions) => {
                // Get all active versions for comparison (across all variants)
                let active_versions: Vec<(String, String)> = active_backends
                    .iter()
                    .filter(|b| b.name == target_name)
                    .map(|b| (b.gpu_variant.clone(), b.version.clone()))
                    .collect();

                for v in versions {
                    let is_active = active_versions
                        .iter()
                        .any(|(gv, ver)| gv == &v.gpu_variant && ver == &v.version);
                    entries.push(VersionEntry {
                        name: v.name.clone(),
                        version: v.version.clone(),
                        path: v.path.clone(),
                        gpu_type: v.gpu_type.clone(),
                        gpu_variant: v.gpu_variant.clone(),
                        is_active,
                    });
                }
            }
            None => {
                println!("Backend '{}' not found.", target_name);
                return Ok(());
            }
        }
    } else {
        // Show all versions for all backends
        // Collect unique backend names to avoid duplicates when multiple variants are active
        let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for active in &active_backends {
            if !seen_names.insert(active.name.clone()) {
                continue; // Already processed this backend
            }

            let name = active.name.clone();

            // Get all versions for this backend
            let all_versions = match registry.list_all_versions(&name, None)? {
                Some(v) => v,
                None => vec![active.clone()],
            };

            // Build a set of (version, gpu_variant) pairs that are active
            let active_set: std::collections::HashSet<(String, String)> = active_backends
                .iter()
                .filter(|a| a.name == name)
                .map(|a| (a.version.clone(), a.gpu_variant.clone()))
                .collect();

            for v in all_versions {
                let is_active = active_set.contains(&(v.version.clone(), v.gpu_variant.clone()));
                entries.push(VersionEntry {
                    name: v.name.clone(),
                    version: v.version.clone(),
                    path: v.path.clone(),
                    gpu_type: v.gpu_type.clone(),
                    gpu_variant: v.gpu_variant.clone(),
                    is_active,
                });
            }
        }
    }

    if entries.is_empty() {
        println!("No versions found.");
        return Ok(());
    }

    println!("Backend versions:\n");
    for entry in &entries {
        let active_marker = if entry.is_active { " * active" } else { "" };
        println!(
            "  {} [{}]{} (v{})",
            entry.name, entry.gpu_variant, active_marker, entry.version
        );
        println!("    Path:     {}", entry.path.display());
        if let Some(ref gpu) = entry.gpu_type {
            println!("    GPU:      {:?}", gpu);
        }
        println!();
    }

    // Show usage tip
    if let Some(target) = name {
        println!(
            "To activate a version: tama backend switch {} <version>",
            target
        );
    } else {
        println!("To activate a version: tama backend switch <backend_name> <version>");
    }

    Ok(())
}
