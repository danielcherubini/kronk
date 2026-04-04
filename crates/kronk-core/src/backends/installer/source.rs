use anyhow::{anyhow, Result};
#[cfg(target_os = "windows")]
use anyhow::Context;
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

    // Clone repository
    clone_repository(version, git_url, &source_dir).await?;

    // Configure with CMake
    configure_cmake(options, &source_dir, &build_output).await?;

    // Build
    build_cmake(&build_output).await?;

    // Install binary
    let result = install_binary(&build_output, options).await;

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
async fn clone_repository(version: &str, git_url: &str, source_dir: &Path) -> Result<()> {
    println!("Cloning repository (shallow)...");

    // For "latest", resolve the most recent tag first before trying branch clone
    if version == "latest" && try_clone_latest_tag(git_url, source_dir).await? {
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

/// Build the CMake argument list for the configure step.
///
/// Extracted for testability — callers can verify flags without invoking cmake.
fn build_cmake_args(options: &InstallOptions, source_dir: &Path, build_output: &Path) -> Vec<String> {
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

    // Explicitly enable all IQK FlashAttention KV cache quant types for ik_llama.
    // This defaults to ON in current ik_llama.cpp main, but we set it explicitly
    // to guard against any future default change. Without it, sub-q8_0 KV cache
    // types cause NaN crashes on hybrid Mamba/attention models (e.g. Qwen3.5).
    // Note: this is GGML_IQK_FA_ALL_QUANTS (CPU IQK kernels), distinct from
    // GGML_CUDA_FA_ALL_QUANTS (CUDA FlashAttention kernels, defaults OFF).
    if matches!(
        options.backend_type,
        super::super::registry::BackendType::IkLlama
    ) {
        cmake_args.push("-DGGML_IQK_FA_ALL_QUANTS=ON".to_string());

        // On Windows, use the Ninja + clang-cl approach recommended by the
        // ik_llama.cpp official build docs. This sidesteps all MSVC AVX2
        // detection issues: clang-cl accepts -march=native directly (via
        // /clang:-march=native), which reliably defines __AVX2__ and activates
        // the IQK optimized CPU kernels required by hybrid Mamba/attention
        // models like Qwen3.5. Without these kernels, SSM layers produce
        // inf/NaN logits and crash on the first token.
        if cfg!(target_os = "windows") {
            // Use Ninja generator so clang-cl works correctly (it doesn't
            // integrate well with the Visual Studio MSBuild generator).
            cmake_args.push("-GNinja".to_string());
            // clang-cl: LLVM's CL-compatible driver. Supports -march=native
            // unlike MSVC cl.exe, so __AVX2__ is reliably defined.
            cmake_args.push("-DCMAKE_C_COMPILER=clang-cl".to_string());
            cmake_args.push("-DCMAKE_CXX_COMPILER=clang-cl".to_string());
            // Pass -march=native through clang-cl's /clang: prefix so it
            // reaches the underlying clang compiler. This enables all CPU
            // features the host supports (AVX2, FMA, etc.) and defines
            // __AVX2__, which activates the IQK optimized CPU kernels.
            let cpu_flags =
                "/clang:-march=native /clang:-fvectorize /clang:-ffp-model=fast /clang:-fno-finite-math-only";
            cmake_args.push(format!("-DCMAKE_C_FLAGS={}", cpu_flags));
            cmake_args.push(format!("-DCMAKE_CXX_FLAGS=/EHsc {}", cpu_flags));
            // CUDA arch: "native" lets nvcc detect the installed GPU at compile
            // time. Required because without it CUDA 13.x would use the fallback
            // list which includes compute_50 (dropped in CUDA 13.x).
            cmake_args.push("-DCMAKE_CUDA_ARCHITECTURES=native".to_string());
        }
    }

    cmake_args
}

/// Find the vcvarsall.bat script for MSVC environment setup.
/// Searches known Visual Studio Build Tools installation paths.
#[cfg(target_os = "windows")]
fn find_vcvarsall() -> Option<std::path::PathBuf> {
    // VS year-named installs (2022, 2019, ...) and numeric (18, 17, ...)
    let vs_base = std::path::Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio");
    let editions = ["BuildTools", "Enterprise", "Professional", "Community"];
    let subdirs = ["18", "2026", "17", "2022", "16", "2019"];

    for subdir in &subdirs {
        for edition in &editions {
            let candidate = vs_base
                .join(subdir)
                .join(edition)
                .join(r"VC\Auxiliary\Build\vcvarsall.bat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// On Windows, run cmake inside a vcvars-activated environment so that
/// nvcc can locate the MSVC host compiler headers and libs.
///
/// We write a temporary .bat file containing the vcvarsall + cmake calls,
/// then execute it with `cmd /c <bat_path>`. This avoids all cmd.exe inline
/// quoting complexity (the "network path not found" class of errors that
/// occur when trying to inline a quoted UNC-like path after `cmd /c`).
#[cfg(target_os = "windows")]
async fn configure_cmake_windows(cmake_args: &[String], build_output: &Path) -> Result<()> {
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

    let bat_contents = match find_vcvarsall() {
        Some(vcvarsall) => {
            tracing::info!("Using vcvarsall: {:?}", vcvarsall);
            format!(
                "@echo off\r\ncall \"{vcvarsall}\" x64\r\nif errorlevel 1 exit /b 1\r\n{cmake}\r\n",
                vcvarsall = vcvarsall.to_string_lossy(),
                cmake = cmake_invocation,
            )
        }
        None => {
            tracing::warn!(
                "vcvarsall.bat not found; running cmake without MSVC environment. \
                 CUDA builds may fail if MSVC headers are not already on PATH."
            );
            format!("@echo off\r\n{}\r\n", cmake_invocation)
        }
    };

    let bat_path = build_output.join("kronk_cmake_configure.bat");
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

/// Run CMake configuration step.
async fn configure_cmake(
    options: &InstallOptions,
    source_dir: &Path,
    build_output: &Path,
) -> Result<()> {
    let cmake_args = build_cmake_args(options, source_dir, build_output);

    #[cfg(target_os = "windows")]
    {
        return configure_cmake_windows(&cmake_args, build_output).await;
    }

    #[cfg(not(target_os = "windows"))]
    {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::registry::{BackendSource, BackendType};
    use std::path::PathBuf;

    fn make_options(backend_type: BackendType, gpu_type: Option<GpuType>) -> InstallOptions {
        InstallOptions {
            backend_type,
            source: BackendSource::SourceCode {
                version: "main".to_string(),
                git_url: "https://example.com/repo.git".to_string(),
            },
            target_dir: PathBuf::from("/tmp/test"),
            gpu_type,
            allow_overwrite: false,
        }
    }

    /// ik_llama source builds must explicitly set GGML_IQK_FA_ALL_QUANTS=ON.
    /// It defaults to ON in current ik_llama.cpp main, but we set it explicitly
    /// to guard against any future default change. Without it, sub-q8_0 KV cache
    /// causes NaN crashes on hybrid Mamba/attention models (e.g. Qwen3.5).
    #[test]
    fn test_ik_llama_includes_iqk_fa_all_quants() {
        let opts = make_options(BackendType::IkLlama, None);
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"));
        assert!(
            args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()),
            "ik_llama build must include -DGGML_IQK_FA_ALL_QUANTS=ON, got: {:?}",
            args
        );
    }

    /// llama.cpp builds must NOT include the ik_llama-specific flag.
    #[test]
    fn test_llama_cpp_excludes_iqk_fa_all_quants() {
        let opts = make_options(BackendType::LlamaCpp, None);
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"));
        assert!(
            !args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()),
            "llama.cpp build must not include -DGGML_IQK_FA_ALL_QUANTS=ON"
        );
    }

    /// ik_llama + CUDA should have both the CUDA flag and the quants flag.
    #[test]
    fn test_ik_llama_cuda_includes_both_flags() {
        let opts = make_options(
            BackendType::IkLlama,
            Some(GpuType::Cuda { version: "12".to_string() }),
        );
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"));
        assert!(args.contains(&"-DGGML_CUDA=ON".to_string()));
        assert!(args.contains(&"-DGGML_IQK_FA_ALL_QUANTS=ON".to_string()));
    }

    /// On Windows, ik_llama builds must use the Ninja + clang-cl approach so
    /// that -march=native is reliably passed via /clang:-march=native. This
    /// defines __AVX2__ and activates the IQK optimized CPU kernels required by
    /// hybrid Mamba/attention models (e.g. Qwen3.5). Without these kernels, SSM
    /// layers produce inf/NaN logits and crash on the first token.
    #[test]
    #[cfg(target_os = "windows")]
    fn test_ik_llama_windows_uses_ninja_clang_cl() {
        let opts = make_options(BackendType::IkLlama, None);
        let args = build_cmake_args(&opts, Path::new("/src"), Path::new("/build"));
        assert!(
            args.contains(&"-GNinja".to_string()),
            "Windows ik_llama build must use -GNinja, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DCMAKE_C_COMPILER=clang-cl".to_string()),
            "Windows ik_llama build must set CMAKE_C_COMPILER=clang-cl, got: {:?}",
            args
        );
        assert!(
            args.contains(&"-DCMAKE_CXX_COMPILER=clang-cl".to_string()),
            "Windows ik_llama build must set CMAKE_CXX_COMPILER=clang-cl, got: {:?}",
            args
        );
        // CPU flags must include /clang:-march=native to enable AVX2/FMA
        let has_march = args.iter().any(|a| a.contains("/clang:-march=native"));
        assert!(
            has_march,
            "Windows ik_llama build must include /clang:-march=native in C/CXX flags, got: {:?}",
            args
        );
    }
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

    println!("Backend built and installed at: {:?}", binary_dest);
    Ok(binary_dest)
}
