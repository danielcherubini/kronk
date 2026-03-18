use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use kronk_core::backends::*;
use kronk_core::config::Config;
use kronk_core::gpu;
use std::time::Duration;

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

        /// Custom name for this backend installation
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Update an installed backend to the latest version
    Update {
        /// Name of the backend to update
        name: String,
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
}

pub async fn run(config: &Config, cmd: BackendArgs) -> Result<()> {
    match cmd.command {
        BackendSubcommand::Install { backend_type, version, build, name } => {
            cmd_install(config, &backend_type, version, build, name).await
        }
        BackendSubcommand::Update { name } => cmd_update(config, &name).await,
        BackendSubcommand::List => cmd_list(config).await,
        BackendSubcommand::Remove { name } => cmd_remove(config, &name).await,
        BackendSubcommand::CheckUpdates => cmd_check_updates(config).await,
    }
}

fn parse_backend_type(s: &str) -> Result<BackendType> {
    match s.to_lowercase().as_str() {
        "llama_cpp" | "llama.cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
        "ik_llama" | "ik-llama" | "ikllama" | "ik_llama.cpp" => Ok(BackendType::IkLlama),
        _ => Err(anyhow!("Unknown backend type '{}'. Supported: llama_cpp, ik_llama", s)),
    }
}

fn registry_path() -> Result<std::path::PathBuf> {
    let base_dir = Config::base_dir()?;
    Ok(base_dir.join("backend_registry.toml"))
}

fn backends_dir() -> Result<std::path::PathBuf> {
    let base_dir = Config::base_dir()?;
    Ok(base_dir.join("backends"))
}

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64
}

async fn cmd_install(
    _config: &Config,
    backend_type_str: &str,
    version: Option<String>,
    force_build: bool,
    name: Option<String>,
) -> Result<()> {
    let backend_type = parse_backend_type(backend_type_str)?;

    // Check build prerequisites
    println!("Checking system...");
    let caps = gpu::detect_build_prerequisites();
    println!("  OS:       {} {}", caps.os, caps.arch);
    println!("  Git:      {}", if caps.git_available { "found" } else { "not found" });
    println!("  CMake:    {}", if caps.cmake_available { "found" } else { "not found" });
    println!("  Compiler: {}", if caps.compiler_available { "found" } else { "not found" });
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
                vec!["Download pre-built binary (faster)", "Build from source (hardware-optimized)"],
            )
            .prompt()?;
            choice.starts_with("Build")
        }
    };

    // Ask user about GPU acceleration
    let gpu_type = {
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
                // Ask for CUDA version for prebuilt binary selection
                let cuda_ver_choice = inquire::Select::new(
                    "Which CUDA version do you have?",
                    vec![
                        "CUDA 11.x (default: 11.1)",
                        "CUDA 12.x (default: 12.4)",
                        "CUDA 13.x (default: 13.1)",
                    ],
                )
                .prompt()?;

                Some(gpu::GpuType::Cuda {
                    version: match cuda_ver_choice {
                        "CUDA 11.x (default: 11.1)" => "11.1".to_string(),
                        "CUDA 12.x (default: 12.4)" => "12.4".to_string(),
                        "CUDA 13.x (default: 13.1)" => "13.1".to_string(),
                        _ => unreachable!(),
                    },
                })
            }
            "AMD (ROCm)" => {
                let rocm_ver_choice = inquire::Select::new(
                    "Which ROCm version do you have?",
                    vec![
                        "ROCm 5.x (default: 5.7)",
                        "ROCm 6.x (default: 6.1)",
                    ],
                )
                .prompt()?;

                Some(gpu::GpuType::RocM {
                    version: match rocm_ver_choice {
                        "ROCm 5.x (default: 5.7)" => "5.7".to_string(),
                        "ROCm 6.x (default: 6.1)" => "6.1".to_string(),
                        _ => unreachable!(),
                    },
                })
            }
            "Intel / AMD (Vulkan)" => Some(gpu::GpuType::Vulkan),
            "Apple Silicon (Metal)" => Some(gpu::GpuType::Metal),
            _ => None,
        }
    };

    // Determine install directory
    let backend_name = name.unwrap_or_else(|| {
        let type_str = match backend_type {
            BackendType::LlamaCpp => "llama_cpp",
            BackendType::IkLlama => "ik_llama",
            BackendType::Custom => "custom",
        };
        format!("{}_{}", type_str, version)
    });

    let target_dir = backends_dir()?.join(&backend_name);

    // Build install options
    let git_url = match backend_type {
        BackendType::LlamaCpp => "https://github.com/ggml-org/llama.cpp.git",
        BackendType::IkLlama => "https://github.com/ikawrakow/ik_llama.cpp.git",
        BackendType::Custom => unreachable!(),
    };

    let source = if use_source {
        BackendSource::SourceCode {
            version: version.clone(),
            git_url: git_url.to_string(),
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
        gpu_type: gpu_type.clone(),
        allow_overwrite: false,
    };

    // Install
    println!("\nStarting installation...");
    let binary_path = install_backend(options).await?;

    // Register
    let mut registry = BackendRegistry::load(&registry_path()?)?;
    registry.add(BackendInfo {
        name: backend_name.clone(),
        backend_type,
        version,
        path: binary_path.clone(),
        installed_at: current_unix_timestamp(),
        gpu_type,
        source: Some(source),
    })?;

    println!("\nInstallation complete!");
    println!("  Name:   {}", backend_name);
    println!("  Binary: {}", binary_path.display());
    println!("\nTo use this backend:");
    println!("  kronk server add my-server {} --host 0.0.0.0 -m model.gguf -ngl 999", binary_path.display());

    Ok(())
}

async fn cmd_update(_config: &Config, name: &str) -> Result<()> {
    let mut registry = BackendRegistry::load(&registry_path()?)?;

    let backend_info = registry
        .get(name)
        .ok_or_else(|| anyhow!("Backend '{}' not found. Run `kronk backend list` to see installed backends.", name))?
        .clone();

    println!("Checking for updates to '{}'...", name);
    let update_check = check_updates(&backend_info).await?;

    if !update_check.update_available {
        println!("'{}' is already up to date ({})", name, backend_info.version);
        return Ok(());
    }

    println!("Update available:");
    println!("  Current: {}", update_check.current_version);
    println!("  Latest:  {}", update_check.latest_version);

    let confirm = inquire::Confirm::new("Proceed with update?")
        .with_default(true)
        .prompt()?;

    if !confirm {
        println!("Update cancelled.");
        return Ok(());
    }

    let target_dir = backend_info
        .path
        .parent()
        .ok_or_else(|| anyhow!("Invalid backend path: {}", backend_info.path.display()))?
        .to_path_buf();

    // Preserve the original installation method, but update the version
    let source = match backend_info.source.clone() {
        Some(source) => match source {
            BackendSource::Prebuilt { version: _ } => {
                BackendSource::Prebuilt { version: update_check.latest_version.clone() }
            }
            BackendSource::SourceCode { version: _, git_url } => {
                BackendSource::SourceCode { version: update_check.latest_version.clone(), git_url }
            }
        },
        None => {
            // Fallback for existing backends without source info
            match backend_info.backend_type {
                BackendType::IkLlama => BackendSource::SourceCode {
                    version: update_check.latest_version.clone(),
                    git_url: "https://github.com/ikawrakow/ik_llama.cpp.git".to_string(),
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
    let registry = BackendRegistry::load(&registry_path()?)?;
    let backends = registry.list();

    if backends.is_empty() {
        println!("No backends installed.");
        println!("\nTo install one:");
        println!("  kronk backend install llama_cpp");
        println!("  kronk backend install ik_llama");
        return Ok(());
    }

    println!("Installed backends:\n");
    for backend in backends {
        println!("  {} ({:?})", backend.name, backend.backend_type);
        println!("    Version: {}", backend.version);
        println!("    Path:    {}", backend.path.display());
        if let Some(ref gpu) = backend.gpu_type {
            println!("    GPU:     {:?}", gpu);
        }
        println!();
    }

    Ok(())
}

async fn cmd_remove(_config: &Config, name: &str) -> Result<()> {
    let mut registry = BackendRegistry::load(&registry_path()?)?;

    let backend = registry
        .get(name)
        .ok_or_else(|| anyhow!("Backend '{}' not found", name))?
        .clone();

    println!("Removing backend '{}'", name);
    println!("  Path: {}", backend.path.display());

    let confirm = inquire::Confirm::new("Are you sure?")
        .with_default(false)
        .prompt()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    // Remove from registry first
    registry.remove(name)?;

    // Optionally remove files
    if backend.path.exists() {
        let remove_files = inquire::Confirm::new("Also delete the backend files from disk?")
            .with_default(true)
            .prompt()?;

        if remove_files {
            if let Some(parent) = backend.path.parent() {
                // Safety: only remove if it's under our managed backends dir
                // Canonicalize both paths to prevent symlink/.. bypass attacks
                let canonical_parent = std::fs::canonicalize(parent)
                    .map_err(|_e| anyhow::anyhow!("Failed to canonicalize path {:?}", parent))?;
                let managed = std::fs::canonicalize(backends_dir()?)
                    .map_err(|_e| anyhow::anyhow!("Failed to canonicalize backends directory"))?;
                if canonical_parent.starts_with(&managed) {
                    std::fs::remove_dir_all(parent)?;
                    println!("Files removed.");
                } else {
                    println!("Skipping file removal: path is outside managed directory.");
                }
            }
        }
    }

    println!("Backend '{}' removed.", name);
    Ok(())
}

async fn cmd_check_updates(_config: &Config) -> Result<()> {
    let registry = BackendRegistry::load(&registry_path()?)?;
    let backends = registry.list();

    if backends.is_empty() {
        println!("No backends installed.");
        return Ok(());
    }

    println!("Checking for updates...\n");

    for backend in backends {
        print!("  {} ({}): ", backend.name, backend.version);

        match check_updates(backend).await {
            Ok(check) => {
                if check.update_available {
                    println!("UPDATE AVAILABLE -> {}", check.latest_version);
                } else {
                    println!("up to date");
                }
            }
            Err(e) => {
                println!("error: {}", e);
            }
        }
    }

    Ok(())
}