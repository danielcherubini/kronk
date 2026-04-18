use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(target_os = "windows")]
use anyhow::Context;

use anyhow::{anyhow, Result};

use super::build::build_cmake_args;
use super::build::emit;
#[cfg(not(target_os = "windows"))]
use super::detect::detect_hip_env;
#[cfg(target_os = "windows")]
use super::detect::find_llvm_bin;
#[cfg(target_os = "windows")]
use super::detect::find_vcvarsall;
use crate::backends::installer::extract::find_backend_binary;
use crate::backends::installer::prebuilt::prepare_target_dir;
use crate::backends::registry::BackendType;
use crate::backends::InstallOptions;
use crate::backends::ProgressSink;
use crate::gpu::{detect_amdgpu_targets, GpuType};

/// Build and install a backend from source using git + cmake.
pub async fn install_from_source(
    options: &InstallOptions,
    version: &str,
    git_url: &str,
    commit: Option<&str>,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<PathBuf> {
    emit(
        progress,
        format!("Building from source: {} version {}", git_url, version),
    );

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

    // Use a persistent build directory inside the target dir so that debug
    // symbols in the compiled binary point to real paths (not a temp dir that
    // gets deleted). This also lets users inspect the source if a crash log
    // references a file path.
    let build_root = options.target_dir.join("build");
    let source_dir = build_root.join("source");
    let build_output = build_root.join("cmake");

    // Clean any previous build attempt
    if build_root.exists() {
        std::fs::remove_dir_all(&build_root)?;
    }
    std::fs::create_dir_all(&build_output)?;

    // ik_llama doesn't publish real release tags (only a stale pre-release).
    // For "latest", always clone main HEAD instead of attempting tag resolution.
    let use_tag_resolution = !matches!(options.backend_type, BackendType::IkLlama);

    // Clone repository
    clone_repository(
        version,
        git_url,
        &source_dir,
        commit,
        use_tag_resolution,
        progress,
    )
    .await?;

    // Configure with CMake
    configure_cmake(options, &source_dir, &build_output, progress).await?;

    // Build
    build_cmake(&build_output, progress).await?;

    // Install binary
    let result = install_binary(&build_output, options, progress).await;

    // Clean up build artifacts on success — the binary is installed and the
    // multi-GB build tree is no longer needed. On failure, leave it in place
    // so the source paths in any crash logs remain valid for debugging.
    if result.is_ok() {
        if let Err(e) = std::fs::remove_dir_all(&build_root) {
            tracing::warn!("Failed to clean up build directory: {}", e);
        }
    }

    result
}

/// Clone a git repository, with fallback logic for "latest" and "main" tags.
///
/// When `commit` is `Some`, clones the `main` branch with a sufficient depth
/// to reach the target commit, then runs `git checkout <commit>`.
///
/// `use_tag_resolution`: when true and `version == "latest"`, try to find the
/// most recent git tag first. Set to false for backends like ik_llama that do
/// not publish proper release tags.
async fn clone_repository(
    version: &str,
    git_url: &str,
    source_dir: &Path,
    commit: Option<&str>,
    use_tag_resolution: bool,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    // When a specific commit is requested, do a deeper clone of main then checkout.
    if let Some(commit_hash) = commit {
        emit(
            progress,
            format!(
                "Cloning repository for commit {} (depth 500)...",
                commit_hash
            ),
        );
        let clone_status = tokio::process::Command::new("git")
            .args([
                "clone",
                "--depth",
                "500",
                "--branch",
                "main",
                git_url,
                &source_dir.to_string_lossy(),
            ])
            .status()
            .await?;

        if !clone_status.success() {
            return Err(anyhow!(
                "Failed to clone repository from {} (depth 500)",
                git_url
            ));
        }

        emit(progress, format!("Checking out commit {}...", commit_hash));
        let checkout_status = tokio::process::Command::new("git")
            .args(["-C", &source_dir.to_string_lossy(), "checkout", commit_hash])
            .status()
            .await?;

        if !checkout_status.success() {
            return Err(anyhow!(
                "Failed to checkout commit {}. \
                 The commit may be older than the clone depth (500). \
                 Try a more recent commit.",
                commit_hash
            ));
        }

        emit(progress, format!("Checked out commit {}.", commit_hash));
        return Ok(());
    }

    emit(progress, "Cloning repository (shallow)...");

    // For "latest", resolve the most recent tag first before trying branch clone.
    // Skip tag resolution for backends that don't publish proper release tags (e.g. ik_llama).
    if version == "latest"
        && use_tag_resolution
        && try_clone_latest_tag(git_url, source_dir, progress).await?
    {
        return Ok(());
    }

    // Versions like "main@abc12345" mean "clone the main branch"
    let branch = if version.starts_with("main@") {
        "main"
    } else {
        version
    };

    let clone_result = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            branch,
            git_url,
            &source_dir.to_string_lossy(),
        ])
        .status()
        .await?;

    if clone_result.success() {
        return Ok(());
    }

    // Only allow fallback to HEAD for "main" or "latest" (tags may not exist)
    if !version.starts_with("main") && version != "latest" {
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
    emit(
        progress,
        format!("Tag/branch '{}' not found, cloning HEAD...", version),
    );
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
async fn try_clone_latest_tag(
    git_url: &str,
    source_dir: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<bool> {
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
                emit(progress, format!("Resolving 'latest' to tag: {}", tag_name));
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
    #[allow(unused_variables)] progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    let amdgpu_targets = if matches!(options.gpu_type, Some(GpuType::RocM { .. })) {
        let targets = detect_amdgpu_targets();
        if targets.is_empty() {
            tracing::warn!(
                "No AMDGPU_TARGETS detected (rocminfo missing or returned no gfx entries). \
                 Falling back to llama.cpp's default target list — this may exclude newer archs. \
                 Set KOJI_AMDGPU_TARGETS=gfxNNNN to override."
            );
        } else {
            tracing::info!("Detected AMDGPU_TARGETS: {}", targets.join(";"));
        }
        targets
    } else {
        Vec::new()
    };
    let cmake_args = build_cmake_args(options, source_dir, build_output, &amdgpu_targets);

    #[cfg(target_os = "windows")]
    {
        return configure_cmake_windows(&cmake_args, build_output, progress).await;
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut cmd = tokio::process::Command::new("cmake");
        cmd.args(&cmake_args);
        if matches!(options.gpu_type, Some(GpuType::RocM { .. })) {
            if let Some((hipcxx, hip_path)) = detect_hip_env() {
                tracing::info!("Using HIPCXX={}, HIP_PATH={}", hipcxx, hip_path);
                cmd.env("HIPCXX", hipcxx);
                cmd.env("HIP_PATH", hip_path);
            } else {
                tracing::warn!(
                    "hipconfig not found or returned empty output. \
                     Falling back to PATH-based HIP discovery. \
                     Ensure /opt/rocm/bin is on PATH if the build fails."
                );
            }
        }
        let status = cmd.status().await?;

        if !status.success() {
            return Err(anyhow!(
                "CMake configuration failed. Check that all build dependencies are installed."
            ));
        }

        Ok(())
    }
}

/// On Windows, run cmake inside a vcvars-activated environment so that
/// nvcc can locate the MSVC host compiler headers and libs.
///
/// We write a temporary .bat file containing the vcvarsall + cmake calls,
/// then execute it with `cmd /c <bat_path>`. This avoids all cmd.exe inline
/// quoting complexity (the "network path not found" class of errors that
/// occur when trying to inline a quoted UNC-like path after `cmd /c`).
#[cfg(target_os = "windows")]
async fn configure_cmake_windows(
    cmake_args: &[String],
    build_output: &Path,
    #[allow(unused_variables)] progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    // Build the cmake invocation line for inside the .bat file.
    // Each arg containing spaces gets double-quoted (safe in .bat context).
    let cmake_invocation = std::iter::once("cmake".to_string())
        .chain(cmake_args.iter().cloned())
        .map(|a| {
            if a.contains(' ') {
                format!("\"{}\"", a)
            } else {
                a
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Prepend LLVM bin dir to PATH if found, so clang-cl is discoverable by cmake.
    let llvm_path_line = match find_llvm_bin() {
        Some(llvm_bin) => {
            tracing::info!("Found LLVM bin: {:?}", llvm_bin);
            format!("set PATH={};%PATH%\r\n", llvm_bin.to_string_lossy())
        }
        None => {
            tracing::warn!(
                "LLVM bin dir not found; clang-cl may not be on PATH. \
                 Install LLVM from https://releases.llvm.org/"
            );
            String::new()
        }
    };

    let bat_contents = match find_vcvarsall() {
        Some(vcvarsall) => {
            tracing::info!("Using vcvarsall: {:?}", vcvarsall);
            format!(
                "@echo off\r\n{llvm_path}call \"{vcvarsall}\" x64\r\nif errorlevel 1 exit /b 1\r\n{cmake}\r\n",
                llvm_path = llvm_path_line,
                vcvarsall = vcvarsall.to_string_lossy(),
                cmake = cmake_invocation,
            )
        }
        None => {
            tracing::warn!(
                "vcvarsall.bat not found; running cmake without MSVC environment. \
                 CUDA builds may fail if MSVC headers are not already on PATH."
            );
            format!("@echo off\r\n{}{}\r\n", llvm_path_line, cmake_invocation)
        }
    };

    let bat_path = build_output.join("koji_cmake_configure.bat");
    std::fs::write(&bat_path, &bat_contents)
        .with_context(|| format!("Failed to write cmake bat file: {:?}", bat_path))?;

    let status = tokio::process::Command::new("cmd")
        .args(["/c", &bat_path.to_string_lossy()])
        .status()
        .await?;

    if !status.success() {
        return Err(anyhow!(
            "CMake configuration failed. Check that all build dependencies are installed \
             (clang-cl, ninja, CUDA toolkit, Visual Studio Build Tools)."
        ));
    }

    Ok(())
}

/// Run CMake build step with parallel jobs.
async fn build_cmake(build_output: &Path, progress: Option<&Arc<dyn ProgressSink>>) -> Result<()> {
    let num_jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    emit(
        progress,
        format!(
            "Building with {} parallel jobs (this may take several minutes)...",
            num_jobs
        ),
    );

    // On Windows, nvcc requires cl.exe to be on PATH (it's the CUDA host
    // compiler). vcvarsall.bat was sourced during configure, but each
    // Command::new() spawns a fresh process that doesn't inherit that
    // environment. Wrap the build step in the same .bat-file approach.
    #[cfg(target_os = "windows")]
    {
        let cmake_build_cmd = format!(
            "cmake --build \"{}\" --config Release -j {}",
            build_output.to_string_lossy(),
            num_jobs
        );
        let llvm_path_line = match find_llvm_bin() {
            Some(llvm_bin) => format!("set PATH={};%PATH%\r\n", llvm_bin.to_string_lossy()),
            None => String::new(),
        };
        let bat_contents = match find_vcvarsall() {
            Some(vcvarsall) => format!(
                "@echo off\r\n{llvm_path}call \"{vcvarsall}\" x64\r\nif errorlevel 1 exit /b 1\r\n{cmake}\r\n",
                llvm_path = llvm_path_line,
                vcvarsall = vcvarsall.to_string_lossy(),
                cmake = cmake_build_cmd,
            ),
            None => format!("@echo off\r\n{}{}\r\n", llvm_path_line, cmake_build_cmd),
        };
        let bat_path = build_output.join("koji_cmake_build.bat");
        std::fs::write(&bat_path, &bat_contents)
            .with_context(|| format!("Failed to write build bat file: {:?}", bat_path))?;
        let status = tokio::process::Command::new("cmd")
            .args(["/c", &bat_path.to_string_lossy()])
            .status()
            .await?;
        if !status.success() {
            return Err(anyhow!("Build failed. Check the output above for errors."));
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
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
}

/// Copy the built binary (and shared libs) to the target directory.
async fn install_binary(
    build_output: &Path,
    options: &InstallOptions,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<PathBuf> {
    emit(progress, "Installing binary...");
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
                                    tracing::warn!("Failed to copy shared library {}: {}", name, e);
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

    emit(
        progress,
        format!("Backend built and installed at: {:?}", binary_dest),
    );
    Ok(binary_dest)
}
