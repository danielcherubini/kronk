use anyhow::Result;
use tama_core::backends::{
    backends_dir, check_latest_version, get_backend_install_path, install_backend,
    install_tts_kokoro, BackendInfo, BackendRegistry, BackendSource, BackendType, InstallOptions,
    NullSink,
};
use tama_core::gpu;

use super::parse::{
    current_unix_timestamp, parse_backend_type, parse_gpu_type, registry_config_dir,
};

#[allow(clippy::too_many_arguments)]
pub async fn cmd_install(
    _config: &tama_core::config::Config,
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
