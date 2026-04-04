use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use kronk_core::backends::*;
use kronk_core::config::Config;
use kronk_core::gpu;

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
        BackendSubcommand::Install {
            backend_type,
            version,
            build,
            name,
            gpu,
            force,
        } => cmd_install(config, &backend_type, version, build, name, gpu, force).await,
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
        _ => Err(anyhow!(
            "Unknown backend type '{}'. Supported: llama_cpp, ik_llama",
            s
        )),
    }
}

fn parse_gpu_type(gpu_str: &str) -> Result<kronk_core::gpu::GpuType> {
    let gpu_str = gpu_str.trim().to_lowercase();

    match gpu_str.as_str() {
        "cpu" => Ok(kronk_core::gpu::GpuType::CpuOnly),
        "cuda" => {
            let version = kronk_core::gpu::detect_cuda_version()
                .unwrap_or_else(|| {
                    eprintln!(
                        "Warning: Could not auto-detect CUDA version (nvcc/nvidia-smi not found). \
                         Defaulting to {}. Use 'cuda:<version>' to specify explicitly.",
                        kronk_core::gpu::DEFAULT_CUDA_VERSION
                    );
                    kronk_core::gpu::DEFAULT_CUDA_VERSION.to_string()
                });
            println!("Detected CUDA version: {}", version);
            Ok(kronk_core::gpu::GpuType::Cuda { version })
        }
        "rocm" => Ok(kronk_core::gpu::GpuType::RocM {
            version: "6.1".to_string(),
        }),
        "vulkan" => Ok(kronk_core::gpu::GpuType::Vulkan),
        "metal" => Ok(kronk_core::gpu::GpuType::Metal),
        s if s.starts_with("cuda:") => {
            let version = s.strip_prefix("cuda:").unwrap();
            if version.is_empty() {
                anyhow::bail!("Invalid --gpu value: missing CUDA version after 'cuda:'");
            }
            Ok(kronk_core::gpu::GpuType::Cuda {
                version: version.to_string(),
            })
        }
        s if s.starts_with("rocm:") => {
            let version = s.strip_prefix("rocm:").unwrap();
            if version.is_empty() {
                anyhow::bail!("Invalid --gpu value: missing ROCm version after 'rocm:'");
            }
            Ok(kronk_core::gpu::GpuType::RocM {
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

fn backends_dir() -> Result<std::path::PathBuf> {
    let base_dir = Config::base_dir()?;
    Ok(base_dir.join("backends"))
}

fn current_unix_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs() as i64
}

async fn cmd_install(
    _config: &Config,
    backend_type_str: &str,
    version: Option<String>,
    force_build: bool,
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
        version,
        path: binary_path.clone(),
        installed_at: current_unix_timestamp(),
        gpu_type: Some(gpu_type),
        source: Some(source),
    })?;

    println!("\nInstallation complete!");
    println!("  Name:   {}", backend_name);
    println!("  Binary: {}", binary_path.display());
    println!("\nTo use this backend:");
    println!(
        "  kronk server add my-server {} --host 0.0.0.0 -m model.gguf -ngl 999",
        binary_path.display()
    );

    Ok(())
}

async fn cmd_update(_config: &Config, name: &str) -> Result<()> {
    let mut registry = BackendRegistry::open(&registry_config_dir()?)?;

    let backend_info = registry.get(name)?.ok_or_else(|| {
        anyhow!(
            "Backend '{}' not found. Run `kronk backend list` to see installed backends.",
            name
        )
    })?;

    println!("Checking for updates to '{}'...", name);
    let update_check = check_updates(&backend_info).await?;

    if !update_check.update_available {
        println!(
            "'{}' is already up to date ({})",
            name, backend_info.version
        );
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
            BackendSource::Prebuilt { version: _ } => BackendSource::Prebuilt {
                version: update_check.latest_version.clone(),
            },
            BackendSource::SourceCode {
                version: _,
                git_url,
            } => BackendSource::SourceCode {
                version: update_check.latest_version.clone(),
                git_url,
            },
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
    let registry = BackendRegistry::open(&registry_config_dir()?)?;
    let backends = registry.list()?;

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
            if let Some(parent) = backend.path.parent() {
                // SECURITY: Prevent directory traversal attacks during file removal
                // - Canonicalize both paths: resolves symlinks and normalizes "."/".." sequences
                // - This prevents attacks where a symlink or ".." traversal could escape
                //   the intended removal directory and delete arbitrary files
                // - The "starts_with" check ensures we only remove files within our
                //   managed "backends/" directory, preventing accidental deletion of
                //   system or user files outside our control
                // - If canonicalization fails (permissions, etc.), deletion is skipped
                //   by default — a safe conservative behavior that prevents unintended
                //   side effects from transient file system issues
                let canonical_parent_opt = std::fs::canonicalize(parent).ok();
                let managed_opt = std::fs::canonicalize(backends_dir()?).ok();

                if let (Some(canonical_parent), Some(managed)) = (canonical_parent_opt, managed_opt)
                {
                    if canonical_parent.starts_with(&managed) {
                        // On Windows, remove_dir_all fails if a process is using the directory
                        #[cfg(windows)]
                        {
                            use std::io::ErrorKind;
                            match std::fs::remove_dir_all(parent) {
                                Ok(_) => {
                                    println!("Files removed.");
                                }
                                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                                    println!(
                                        "Skipping file removal: backend may still be running."
                                    );
                                    println!("Run 'kronk service stop' first, then try again.");
                                    return Err(anyhow!(
                                        "Failed to remove backend directory: {}",
                                        e
                                    ));
                                }
                                Err(e) => {
                                    eprintln!("Skipping file removal: {}", e);
                                    return Err(anyhow!(
                                        "Failed to remove backend directory: {}",
                                        e
                                    ));
                                }
                            }
                        }
                        // On Unix, remove_dir_all will fail if directory is in use
                        #[cfg(not(windows))]
                        {
                            match std::fs::remove_dir_all(parent) {
                                Ok(_) => {
                                    println!("Files removed.");
                                }
                                Err(e) => {
                                    eprintln!("Skipping file removal: {}", e);
                                    return Err(anyhow!(
                                        "Failed to remove backend directory: {}",
                                        e
                                    ));
                                }
                            }
                        }
                    } else {
                        eprintln!("Skipping file removal: path is outside managed directory.");
                    }
                } else {
                    eprintln!("Skipping file removal: directory does not exist.");
                }
            }
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
