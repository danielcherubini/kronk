pub mod parse;

// Re-export parsing utilities for backward compatibility
pub(crate) use parse::{parse_backend_type, parse_gpu_type};

use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use tama_core::backends::{
    backends_dir, check_latest_version, check_updates, get_backend_install_path, install_backend,
    install_tts_kokoro, safe_remove_installation, update_backend, BackendInfo, BackendRegistry,
    BackendSource, BackendType, InstallOptions, NullSink,
};
use tama_core::config::Config;
use tama_core::gpu;

use crate::commands::backend::parse::{current_unix_timestamp, registry_config_dir};

#[derive(Debug, Args)]
pub struct BackendArgs {
    #[command(subcommand)]
    pub command: BackendSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum BackendSubcommand {
    /// Install a new backend (LLM or TTS)
    Install {
        /// Backend type: llama_cpp, ik_llama, or tts_kokoro
        #[arg(value_name = "TYPE")]
        backend_type: String,

        /// Version to install (e.g., b8407). Defaults to latest.
        #[arg(short, long)]
        version: Option<String>,

        /// Force build from source instead of downloading pre-built binary
        #[arg(long)]
        build: bool,

        /// Pin to a specific git commit hash (implies --build).
        /// Example: --commit 61fad8b0940af2bfda9c2708b899c1fe16f9455b
        #[arg(long)]
        commit: Option<String>,

        /// Custom name for this backend installation
        #[arg(short, long)]
        name: Option<String>,

        /// GPU acceleration type (cpu, cuda, cuda:12, rocm, rocm:6, vulkan, metal)
        #[arg(long)]
        gpu: Option<String>,

        /// Overwrite existing backend installation
        #[arg(short, long)]
        force: bool,
    },

    /// Update an installed backend to the latest version
    Update {
        /// Name of the backend to update
        name: String,

        /// Force reinstall even if already up to date
        #[arg(short, long)]
        force: bool,
    },

    /// List installed backends
    #[command(alias = "ls")]
    List,

    /// Remove an installed backend
    #[command(alias = "rm")]
    Remove {
        /// Name of the backend to remove
        name: String,
        /// GPU variant to remove (cpu, cuda, vulkan, rocm, metal). Omit to remove all variants.
        #[arg(long)]
        gpu: Option<String>,
    },

    /// Check for updates to all installed backends
    CheckUpdates,

    /// List all versions of a backend (not just the active one)
    #[command(alias = "versions")]
    AllVersions {
        /// Name of the backend (omit to list all backends with all their versions)
        #[arg(long)]
        name: Option<String>,
    },

    /// Activate a specific version of a backend
    Switch {
        /// Name of the backend
        name: String,
        /// Version to activate
        version: String,
        /// GPU variant (cpu, cuda, vulkan, rocm, metal). Auto-inferred if only one variant exists.
        #[arg(long)]
        gpu: Option<String>,
    },

    /// Remove a single version (not all versions)
    RemoveVersion {
        /// Name of the backend
        name: String,
        /// Version to remove
        version: String,
        /// GPU variant (cpu, cuda, vulkan, rocm, metal). Auto-inferred if only one variant exists.
        #[arg(long)]
        gpu: Option<String>,
    },
}

pub async fn run(config: &Config, cmd: BackendArgs) -> Result<()> {
    match cmd.command {
        BackendSubcommand::Install {
            backend_type,
            version,
            build,
            commit,
            name,
            gpu,
            force,
        } => {
            cmd_install(
                config,
                &backend_type,
                version,
                build,
                commit,
                name,
                gpu,
                force,
            )
            .await
        }
        BackendSubcommand::Update { name, force } => cmd_update(config, &name, force).await,
        BackendSubcommand::List => cmd_list(config).await,
        BackendSubcommand::Remove { name, gpu } => cmd_remove(config, &name, gpu.as_deref()).await,
        BackendSubcommand::CheckUpdates => cmd_check_updates(config).await,
        BackendSubcommand::AllVersions { name } => cmd_all_versions(config, name.as_deref()).await,
        BackendSubcommand::Switch { name, version, gpu } => {
            cmd_switch(config, &name, &version, gpu.as_deref()).await
        }
        BackendSubcommand::RemoveVersion { name, version, gpu } => {
            cmd_remove_version(config, &name, &version, gpu.as_deref()).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn cmd_install(
    _config: &Config,
    backend_type_str: &str,
    version: Option<String>,
    force_build: bool,
    commit: Option<String>,
    name: Option<String>,
    gpu_flag: Option<String>,
    force: bool,
) -> Result<()> {
    let backend_type = parse_backend_type(backend_type_str)?;

    // Check build prerequisites
    println!("Checking system...");
    let caps = gpu::detect_build_prerequisites();
    println!("  OS:       {} {}", caps.os, caps.arch);
    println!(
        "  Git:      {}",
        if caps.git_available {
            "found"
        } else {
            "not found"
        }
    );
    println!(
        "  CMake:    {}",
        if caps.cmake_available {
            "found"
        } else {
            "not found"
        }
    );
    println!(
        "  Compiler: {}",
        if caps.compiler_available {
            "found"
        } else {
            "not found"
        }
    );
    println!();

    // Fetch latest version if not specified (skip for TTS backends — they use pinned versions)
    let version = match version {
        Some(v) => v,
        None if matches!(backend_type, BackendType::TtsKokoro) => String::from("latest"),
        None => {
            println!("\nFetching latest version...");
            check_latest_version(&backend_type).await?
        }
    };
    if !matches!(backend_type, BackendType::TtsKokoro) {
        println!("Version: {}", version);
    }

    // Parse GPU type from flag or use interactive selection
    let gpu_type = if let Some(gpu_str) = gpu_flag {
        let gpu = parse_gpu_type(&gpu_str)?;
        println!("[--gpu] Using: {:?}", gpu);
        gpu
    } else {
        // Interactive selection
        let gpu_choice = inquire::Select::new(
            "What GPU acceleration do you want?",
            vec![
                "NVIDIA (CUDA)",
                "AMD (ROCm)",
                "Intel / AMD (Vulkan)",
                "Apple Silicon (Metal)",
                "CPU only",
            ],
        )
        .prompt()?;

        match gpu_choice {
            "NVIDIA (CUDA)" => {
                // Auto-detect and show CUDA version
                let detected = gpu::detect_cuda_version();
                let detected_hint = match &detected {
                    Some(v) => format!(" [detected: {}]", v),
                    None => String::new(),
                };

                // Ask for CUDA version for prebuilt binary selection
                let cuda_ver_choice = inquire::Select::new(
                    &format!("Which CUDA version do you have?{}", detected_hint),
                    vec![
                        "CUDA 11.x (default: 11.1)",
                        "CUDA 12.x (default: 12.4)",
                        "CUDA 13.x (default: 13.1)",
                    ],
                )
                .prompt()?;

                gpu::GpuType::Cuda {
                    version: match cuda_ver_choice {
                        "CUDA 11.x (default: 11.1)" => "11.1".to_string(),
                        "CUDA 12.x (default: 12.4)" => "12.4".to_string(),
                        "CUDA 13.x (default: 13.1)" => "13.1".to_string(),
                        other => anyhow::bail!("Unexpected CUDA version choice: {}", other),
                    },
                }
            }
            "AMD (ROCm)" => {
                let rocm_ver_choice = inquire::Select::new(
                    "Which ROCm version do you have?",
                    vec!["ROCm 5.x (default: 5.7)", "ROCm 6.x (default: 6.1)"],
                )
                .prompt()?;

                gpu::GpuType::RocM {
                    version: match rocm_ver_choice {
                        "ROCm 5.x (default: 5.7)" => "5.7".to_string(),
                        "ROCm 6.x (default: 6.1)" => "6.1".to_string(),
                        other => anyhow::bail!("Unexpected ROCm version choice: {}", other),
                    },
                }
            }
            "Intel / AMD (Vulkan)" => gpu::GpuType::Vulkan,
            "Apple Silicon (Metal)" => gpu::GpuType::Metal,
            _ => gpu::GpuType::CpuOnly,
        }
    };

    // --commit implies --build (can't pin a commit to a pre-built binary)
    let force_build = force_build || commit.is_some();

    // Determine installation method.
    // ik_llama has no pre-built binaries, so source is the only option.
    let use_source = match backend_type {
        BackendType::IkLlama => {
            if !force_build {
                println!("\nik_llama does not provide pre-built binaries. Building from source.");
            }
            true
        }
        _ if force_build => true,
        BackendType::TtsKokoro => {
            // TTS backends are handled separately above; this is unreachable.
            false
        }
        _ => {
            let choice = inquire::Select::new(
                "Installation method:",
                vec![
                    "Download pre-built binary (faster)",
                    "Build from source (hardware-optimized)",
                ],
            )
            .prompt()?;
            choice.starts_with("Build")
        }
    };

    // Determine install directory using versioned path structure
    let backend_name = name.unwrap_or_else(|| backend_type.to_string());
    let gpu_variant = gpu_type.variant_folder().to_string();

    let target_dir =
        get_backend_install_path(&backends_dir()?, &backend_type, &gpu_variant, &version);

    // Handle TTS backends with dedicated installers (no GPU selection needed)
    if matches!(backend_type, BackendType::TtsKokoro) {
        let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

        install_tts_kokoro(&mut registry, Box::new(NullSink)).await?;

        println!("\nKokoro TTS backend installed successfully!");
        println!("  Name:    {}", backend_name);
        return Ok(());
    }

    let git_url = match backend_type {
        BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
        BackendType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
        BackendType::Custom => {
            anyhow::bail!("Custom backends cannot be installed via this command");
        }
        // TTS variants handled earlier with dedicated installers
        BackendType::TtsKokoro => unreachable!(),
    };

    let source = if use_source {
        BackendSource::SourceCode {
            version: version.clone(),
            git_url: git_url.to_string(),
            commit: commit.clone(),
        }
    } else {
        BackendSource::Prebuilt {
            version: version.clone(),
        }
    };

    let options = InstallOptions {
        backend_type: backend_type.clone(),
        source: source.clone(),
        target_dir,
        gpu_type: Some(gpu_type.clone()),
        gpu_variant: gpu_variant.clone(),
        allow_overwrite: force,
    };

    // Install
    println!("\nStarting installation...");
    let binary_path = install_backend(options).await?;

    // Register
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;
    registry.add(BackendInfo {
        name: backend_name.clone(),
        backend_type,
        version: version.clone(),
        path: binary_path.clone(),
        installed_at: current_unix_timestamp(),
        gpu_type: Some(gpu_type.clone()),
        gpu_variant: gpu_type.variant_folder().to_string(),
        source: Some(source),
    })?;

    println!("\nInstallation complete!");
    println!("  Name:    {}", backend_name);
    println!("  Version: {}", version);
    println!("  Binary:  {}", binary_path.display());
    println!(
        "\nThe backend is already referenced in config.toml as '{}'.",
        backend_name
    );
    println!("To pin this exact version, add to config.toml:");
    println!("  [backends.{}]", backend_name);
    println!("  version = \"{}\"", version);

    Ok(())
}

async fn cmd_update(_config: &Config, name: &str, force: bool) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Find the active backend by listing all active backends
    let active_backends = registry.list()?;
    let backend_info = active_backends
        .iter()
        .find(|b| b.name == name)
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "Backend '{}' not found. Run `tama backend list` to see installed backends.",
                name
            )
        })?;

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

async fn cmd_list(_config: &Config) -> Result<()> {
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

async fn cmd_remove(_config: &Config, name: &str, gpu_variant: Option<&str>) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Get all versions to determine what we're removing
    let all_versions = registry
        .list_all_versions(name, gpu_variant)?
        .ok_or_else(|| {
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
        // Iterate all versions and delete each
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
                if let Err(e) = safe_remove_installation(&info) {
                    eprintln!("Warning: Failed to remove files for {}: {}", v.version, e);
                }
            }
        }
    }

    // Remove from registry only after successful file deletion
    registry.remove(name, gpu_variant)?;

    println!("Backend '{}' removed.", name);
    Ok(())
}

async fn cmd_check_updates(_config: &Config) -> Result<()> {
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

async fn cmd_all_versions(_config: &Config, name: Option<&str>) -> Result<()> {
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
        for active in &active_backends {
            let name = active.name.clone();
            let active_version = active.version.clone();
            let active_gpu_variant = active.gpu_variant.clone();

            // Get all versions for this backend
            let all_versions = match registry.list_all_versions(&name, None)? {
                Some(v) => v,
                None => vec![active.clone()],
            };

            for v in all_versions {
                let is_active = v.version == active_version && v.gpu_variant == active_gpu_variant;
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

async fn cmd_switch(
    _config: &Config,
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

async fn cmd_remove_version(
    _config: &Config,
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
    registry.remove_version(name, &gpu_variant, version)?;

    println!("Version '{}' [{}] removed.", version, gpu_variant);

    Ok(())
}
