use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use koji_core::backends::{
    backends_dir, check_latest_version, check_updates, install_backend, safe_remove_installation,
    update_backend, BackendInfo, BackendRegistry, BackendSource, BackendType, InstallOptions,
};
use koji_core::config::Config;
use koji_core::db::queries::get_backend_by_version;
use koji_core::gpu;

#[derive(Debug, Args)]
pub struct BackendArgs {
    #[command(subcommand)]
    pub command: BackendSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum BackendSubcommand {
    /// Install a new backend
    Install {
        /// Backend type: llama_cpp or ik_llama
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
    },

    /// Remove a single version (not all versions)
    RemoveVersion {
        /// Name of the backend
        name: String,
        /// Version to remove
        version: String,
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
        BackendSubcommand::Remove { name } => cmd_remove(config, &name).await,
        BackendSubcommand::CheckUpdates => cmd_check_updates(config).await,
        BackendSubcommand::AllVersions { name } => cmd_all_versions(config, name.as_deref()).await,
        BackendSubcommand::Switch { name, version } => cmd_switch(config, &name, &version).await,
        BackendSubcommand::RemoveVersion { name, version } => {
            cmd_remove_version(config, &name, &version).await
        }
    }
}

fn parse_backend_type(s: &str) -> Result<BackendType> {
    match s.to_lowercase().as_str() {
        "llama_cpp" | "llama.cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
        "ik_llama" | "ik-llama" | "ikllama" | "ik_llama.cpp" => Ok(BackendType::IkLlama),
        _ => Err(anyhow!(
            "Unknown backend type '{}'. Supported: llama_cpp, ik_llama",
            s
        )),
    }
}

fn parse_gpu_type(gpu_str: &str) -> Result<koji_core::gpu::GpuType> {
    let gpu_str = gpu_str.trim().to_lowercase();

    match gpu_str.as_str() {
        "cpu" => Ok(koji_core::gpu::GpuType::CpuOnly),
        "cuda" => {
            let version = koji_core::gpu::detect_cuda_version()
                .unwrap_or_else(|| {
                    eprintln!(
                        "Warning: Could not auto-detect CUDA version (nvcc/nvidia-smi not found). \
                         Defaulting to {}. Use 'cuda:<version>' to specify explicitly.",
                        koji_core::gpu::DEFAULT_CUDA_VERSION
                    );
                    koji_core::gpu::DEFAULT_CUDA_VERSION.to_string()
                });
            println!("Detected CUDA version: {}", version);
            Ok(koji_core::gpu::GpuType::Cuda { version })
        }
        "rocm" => Ok(koji_core::gpu::GpuType::RocM {
            version: "6.1".to_string(),
        }),
        "vulkan" => Ok(koji_core::gpu::GpuType::Vulkan),
        "metal" => Ok(koji_core::gpu::GpuType::Metal),
        s if s.starts_with("cuda:") => {
            let version = s.strip_prefix("cuda:").unwrap();
            if version.is_empty() {
                anyhow::bail!("Invalid --gpu value: missing CUDA version after 'cuda:'");
            }
            Ok(koji_core::gpu::GpuType::Cuda {
                version: version.to_string(),
            })
        }
        s if s.starts_with("rocm:") => {
            let version = s.strip_prefix("rocm:").unwrap();
            if version.is_empty() {
                anyhow::bail!("Invalid --gpu value: missing ROCm version after 'rocm:'");
            }
            Ok(koji_core::gpu::GpuType::RocM {
                version: version.to_string(),
            })
        }
        _ => anyhow::bail!(
            "Unknown GPU type '{}'. Supported: cpu, cuda, cuda:<version>, rocm, rocm:<version>, vulkan, metal",
            gpu_str
        ),
    }
}

fn registry_config_dir() -> Result<std::path::PathBuf> {
    Config::base_dir()
}

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
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

    // Fetch latest version if not specified
    let version = match version {
        Some(v) => v,
        None => {
            println!("\nFetching latest version...");
            check_latest_version(&backend_type).await?
        }
    };
    println!("Version: {}", version);

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
                        _ => unreachable!(),
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
                        _ => unreachable!(),
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

    // Determine install directory
    let backend_name = name.unwrap_or_else(|| backend_type.to_string());

    let target_dir = backends_dir()?.join(&backend_name);

    // Build install options
    let git_url = match backend_type {
        BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
        BackendType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
        BackendType::Custom => {
            anyhow::bail!("Custom backends cannot be installed via this command");
        }
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
        gpu_type: Some(gpu_type),
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

    let backend_info = registry.get(name)?.ok_or_else(|| {
        anyhow!(
            "Backend '{}' not found. Run `koji backend list` to see installed backends.",
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

    // Use the base backends directory for the backend name,
    // rather than the parent of the current binary, to prevent
    // recursive directory nesting during updates.
    let target_dir = backends_dir()?.join(name);

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
                BackendType::Custom => return Err(anyhow!("Cannot update custom backends")),
            }
        }
    };

    let options = InstallOptions {
        backend_type: backend_info.backend_type.clone(),
        source,
        target_dir,
        gpu_type: backend_info.gpu_type.clone(),
        allow_overwrite: true,
    };

    update_backend(&mut registry, name, options, update_check.latest_version).await?;

    Ok(())
}

async fn cmd_list(_config: &Config) -> Result<()> {
    let registry = BackendRegistry::open(&registry_config_dir()?)?;
    let backends = registry.list()?;

    if backends.is_empty() {
        println!("No backends installed.");
        println!("\nTo install one:");
        println!("  koji backend install llama_cpp");
        println!("  koji backend install ik_llama");
        return Ok(());
    }

    // registry.list() only returns active backends, so all shown are active.
    // Use list_all_versions to show all versions with * active marker.
    println!("Installed backends:\n");
    for backend in &backends {
        let name = backend.name.clone();
        let all_versions = registry.list_all_versions(&name).unwrap_or(None);

        if let Some(versions) = all_versions {
            // Show all versions, marking the active one
            let active_version = backend.version.clone();
            for v in &versions {
                let marker = if v.version == active_version {
                    " * active"
                } else {
                    ""
                };
                println!(
                    "  {} [{}]{} (v{})",
                    v.name, v.backend_type, marker, v.version
                );
                println!("    Version:  {}", v.version);
                println!("    Path:     {}", v.path.display());
                if let Some(ref gpu) = v.gpu_type {
                    println!("    GPU:      {:?}", gpu);
                }
                println!();
            }
        } else {
            // Fallback if list_all_versions fails
            println!(
                "  {} [{}] (v{})",
                backend.name, backend.backend_type, backend.version
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
    if let Some(first) = backends.first() {
        println!("To pin a version in config.toml, add:");
        println!("  [backends.{}]", first.name);
        println!("  version = \"{}\"", first.version);
    }

    Ok(())
}

async fn cmd_remove(_config: &Config, name: &str) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    let backend = registry
        .get(name)?
        .ok_or_else(|| anyhow!("Backend '{}' not found", name))?;

    println!("Removing backend '{}'", name);
    println!("  Path: {}", backend.path.display());

    let confirm = inquire::Confirm::new("Are you sure?")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // Optionally remove files
    if backend.path.exists() {
        let remove_files = inquire::Confirm::new("Also delete the backend files from disk?")
            .with_default(true)
            .prompt()?;

        if remove_files {
            // Use the shared safe_remove_installation helper which handles:
            // - Path validation (prevents directory traversal attacks)
            // - Windows PermissionDenied retry logic
            // - Cross-platform file removal
            safe_remove_installation(&backend)?;
        }
    }

    // Remove from registry only after successful file deletion
    registry.remove(name)?;

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
    backend_type: BackendType,
    version: String,
    path: std::path::PathBuf,
    gpu_type: Option<koji_core::gpu::GpuType>,
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
        match registry.list_all_versions(target_name)? {
            Some(versions) => {
                // Get the active version for comparison
                let active_version = registry.get(target_name)?.map(|a| a.version);

                for v in versions {
                    entries.push(VersionEntry {
                        name: v.name.clone(),
                        backend_type: v.backend_type.clone(),
                        version: v.version.clone(),
                        path: v.path.clone(),
                        gpu_type: v.gpu_type.clone(),
                        is_active: active_version.as_deref() == Some(&v.version),
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
            let _backend_type = active.backend_type.clone();
            let _gpu_type = active.gpu_type.clone();
            let active_version = active.version.clone();

            // Get all versions for this backend
            let all_versions = match registry.list_all_versions(&name)? {
                Some(v) => v,
                None => vec![active.clone()],
            };

            for v in all_versions {
                entries.push(VersionEntry {
                    name: v.name.clone(),
                    backend_type: v.backend_type.clone(),
                    version: v.version.clone(),
                    path: v.path.clone(),
                    gpu_type: v.gpu_type.clone(),
                    is_active: v.version == active_version,
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
            entry.name, entry.backend_type, active_marker, entry.version
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
            "To activate a version: koji backend switch {} <version>",
            target
        );
    } else {
        println!("To activate a version: koji backend switch <backend_name> <version>");
    }

    Ok(())
}

async fn cmd_switch(_config: &Config, name: &str, version: &str) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Check the version exists
    let versions = match registry.list_all_versions(name)? {
        Some(v) => v,
        None => anyhow::bail!(
            "Backend '{}' not found. Run `koji backend list` to see installed backends.",
            name
        ),
    };

    let version_exists = versions.iter().any(|v| v.version == version);
    if !version_exists {
        let available: Vec<String> = versions.iter().map(|v| v.version.clone()).collect();
        anyhow::bail!(
            "Version '{}' not found for backend '{}'. Available: {}",
            version,
            name,
            available.join(", ")
        );
    }

    // Activate the version
    let activated = registry.activate(name, version)?;
    if !activated {
        anyhow::bail!("Failed to activate version '{}'", version);
    }

    println!("Activated backend '{}' version '{}'.", name, version);

    Ok(())
}

async fn cmd_remove_version(_config: &Config, name: &str, version: &str) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    // Get the version info before removing
    let record = get_backend_by_version(&Config::open_db(), name, version)?
        .ok_or_else(|| anyhow!("Backend '{}' version '{}' not found", name, version))?;

    println!("Removing backend '{}' version '{}'", name, version);
    println!("  Path: {}", record.path);

    let confirm = inquire::Confirm::new("Are you sure? This will delete the backend files.")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // STEP 1: Delete files FIRST (before any DB changes)
    let info = BackendInfo {
        name: record.name.clone(),
        backend_type: record
            .backend_type
            .parse()
            .map_err(|e| anyhow!("Invalid backend type: {}", e))?,
        version: record.version.clone(),
        path: std::path::PathBuf::from(&record.path),
        installed_at: record.installed_at,
        gpu_type: None,
        source: None,
    };

    if info.path.exists() {
        safe_remove_installation(&info)?;
    }

    // STEP 2: Remove from registry (activates another version if this was active)
    registry.remove_version(name, version)?;

    println!("Version '{}' removed.", version);

    Ok(())
}
