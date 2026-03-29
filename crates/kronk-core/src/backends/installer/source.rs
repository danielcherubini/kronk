use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

use super::extract::find_backend_binary;
use super::prebuilt::prepare_target_dir;
use super::InstallOptions;
use crate::gpu::GpuType;

/// Build and install a backend from source using git + cmake.
pub async fn install_from_source(
    options: &InstallOptions,
    version: &str,
    git_url: &str,
) -> Result<PathBuf> {
    tracing::info!("Building from source: {} version {}", git_url, version);

    prepare_target_dir(&options.target_dir, options.allow_overwrite)?;

    // Check prerequisites
    let caps = crate::gpu::detect_build_prerequisites();
    if !caps.git_available {
        return Err(anyhow!(
            "Git is required to build from source.\n\
             Install it: https://git-scm.com/downloads\n\
             Linux: sudo apt install git (Debian/Ubuntu) or sudo dnf install git (Fedora)"
        ));
    }
    if !caps.cmake_available {
        return Err(anyhow!(
            "CMake is required to build from source.\n\
             Install it: https://cmake.org/download/\n\
             Linux: sudo apt install cmake (Debian/Ubuntu) or sudo dnf install cmake (Fedora)"
        ));
    }
    if !caps.compiler_available {
        return Err(anyhow!(
            "C++ compiler is required to build from source.\n\
             Linux: sudo apt install build-essential\n\
             Windows: Install Visual Studio Build Tools or MinGW (g++)"
        ));
    }

    let build_dir = tempfile::tempdir()?;
    let source_dir = build_dir.path().join("source");

    // Clone repository
    clone_repository(version, git_url, &source_dir).await?;

    let build_output = build_dir.path().join("build");
    std::fs::create_dir_all(&build_output)?;

    // Configure with CMake
    configure_cmake(options, &source_dir, &build_output).await?;

    // Build
    build_cmake(&build_output).await?;

    // Install binary
    install_binary(&build_output, options).await
}

/// Clone a git repository, with fallback logic for "latest" and "main" tags.
async fn clone_repository(version: &str, git_url: &str, source_dir: &Path) -> Result<()> {
    println!("Cloning repository (shallow)...");

    // For "latest", resolve the most recent tag first before trying branch clone
    if version == "latest" && try_clone_latest_tag(git_url, source_dir).await? {
        return Ok(());
    }

    let clone_result = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            version,
            git_url,
            &source_dir.to_string_lossy(),
        ])
        .status()
        .await?;

    if clone_result.success() {
        return Ok(());
    }

    // Only allow fallback to HEAD for "main" or "latest" (tags may not exist)
    if version != "main" && version != "latest" {
        return Err(anyhow!(
            "Tag/branch '{}' not found. Only 'main' or 'latest' are allowed for fallback.\n\
             Use an explicit version tag (e.g., 'b8407') or specify --build to build from source.",
            version
        ));
    }

    // Fallback: clone without branch tag
    tracing::warn!(
        "Tag/branch '{}' not found, cloning HEAD as fallback. Use an explicit version tag or --build flag.",
        version
    );
    println!("Tag/branch '{}' not found, cloning HEAD...", version);
    let status = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            git_url,
            &source_dir.to_string_lossy(),
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("Failed to clone repository from {}", git_url));
    }

    Ok(())
}

/// Attempt to find and clone the latest tag from a git repository.
/// Returns true if successfully cloned from a tag.
async fn try_clone_latest_tag(git_url: &str, source_dir: &Path) -> Result<bool> {
    let tags_output = tokio::process::Command::new("git")
        .args(["ls-remote", "--tags", "--sort=-v:refname", git_url])
        .output()
        .await;

    match tags_output {
        Ok(output) if output.status.success() => {
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let lines: Vec<&str> = stdout_str.lines().collect();
            // Filter out peeled refs (refs/tags/xxx^{}) which can interleave unpredictably
            let tag_lines: Vec<&str> = lines
                .iter()
                .filter(|l| !l.contains("^{}"))
                .filter(|l| !l.is_empty())
                .copied()
                .collect();
            if let Some(tag_line) = tag_lines.first() {
                // Parse ref field (second tab-separated value), strip "refs/tags/" prefix
                let ref_field: &str = tag_line.split('\t').nth(1).unwrap_or("refs/tags/unknown");
                let tag_name: &str = ref_field
                    .trim_start_matches("refs/tags/")
                    .trim_end_matches("^{}");
                println!("Resolving 'latest' to tag: {}", tag_name);
                let tag_clone = tokio::process::Command::new("git")
                    .args([
                        "clone",
                        "--depth",
                        "1",
                        "--branch",
                        tag_name,
                        git_url,
                        &source_dir.to_string_lossy(),
                    ])
                    .status()
                    .await?;
                if tag_clone.success() {
                    return Ok(true);
                }
            }
        }
        _ => {}
    }

    Ok(false)
}

/// Run CMake configuration step.
async fn configure_cmake(
    options: &InstallOptions,
    source_dir: &Path,
    build_output: &Path,
) -> Result<()> {
    let mut cmake_args = vec![
        "-B".to_string(),
        build_output.to_string_lossy().to_string(),
        "-S".to_string(),
        source_dir.to_string_lossy().to_string(),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
    ];

    // Add GPU-specific flags
    if let Some(ref gpu) = options.gpu_type {
        match gpu {
            GpuType::Cuda { .. } => {
                cmake_args.push("-DGGML_CUDA=ON".to_string());
            }
            GpuType::Vulkan => {
                cmake_args.push("-DGGML_VULKAN=ON".to_string());
            }
            GpuType::Metal => {
                cmake_args.push("-DGGML_METAL=ON".to_string());
            }
            GpuType::RocM { .. } => {
                cmake_args.push("-DGGML_HIPBLAS=ON".to_string());
            }
            GpuType::CpuOnly => {}
            GpuType::Custom => {}
        }
    }

    let status = tokio::process::Command::new("cmake")
        .args(&cmake_args)
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!(
            "CMake configuration failed. Check that all build dependencies are installed."
        ));
    }

    Ok(())
}

/// Run CMake build step with parallel jobs.
async fn build_cmake(build_output: &Path) -> Result<()> {
    let num_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    println!(
        "Building with {} parallel jobs (this may take several minutes)...",
        num_jobs
    );
    let status = tokio::process::Command::new("cmake")
        .args([
            "--build",
            &build_output.to_string_lossy(),
            "--config",
            "Release",
            "-j",
            &num_jobs.to_string(),
        ])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!("Build failed. Check the output above for errors."));
    }

    Ok(())
}

/// Copy the built binary (and shared libs) to the target directory.
async fn install_binary(build_output: &Path, options: &InstallOptions) -> Result<PathBuf> {
    println!("Installing binary...");
    let binary_src = find_backend_binary(build_output)?;

    std::fs::create_dir_all(&options.target_dir)?;
    let binary_name = binary_src
        .file_name()
        .ok_or_else(|| anyhow!("Could not determine binary filename"))?;
    let binary_dest = options.target_dir.join(binary_name);

    // Copy binary
    std::fs::copy(&binary_src, &binary_dest)?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&binary_dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&binary_dest, perms)?;
    }

    // Copy shared libraries so the backend can find them at runtime.
    // On Unix: .so / .dylib files; on Windows: .dll files (e.g. ggml-cuda.dll).
    fn copy_shared_libs(src: &std::path::Path, dest: &std::path::Path) {
        if let Ok(entries) = std::fs::read_dir(src) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_file() {
                    if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                        let is_shared = if cfg!(target_os = "windows") {
                            name.ends_with(".dll")
                        } else {
                            name.contains(".so") || name.ends_with(".dylib")
                        };
                        if is_shared {
                            let dest_path = dest.join(name);
                            if !dest_path.exists() {
                                if let Err(e) = std::fs::copy(&entry_path, &dest_path) {
                                    tracing::warn!(
                                        "Failed to copy shared library {}: {}",
                                        name,
                                        e
                                    );
                                }
                            }
                        }
                    }
                } else if entry_path.is_dir() {
                    copy_shared_libs(&entry_path, dest);
                }
            }
        }
    }
    copy_shared_libs(build_output, &options.target_dir);

    println!("Backend built and installed at: {:?}", binary_dest);
    Ok(binary_dest)
}
