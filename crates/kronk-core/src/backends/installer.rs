#![allow(unused_imports)]
use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use super::registry::{BackendSource, BackendType};
use crate::gpu::GpuType;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub backend_type: BackendType,
    pub source: BackendSource,
    pub target_dir: PathBuf,
    pub gpu_type: Option<GpuType>,
    /// When true, skip the target directory existence check.
    /// Used by the update path where the directory already exists.
    pub allow_overwrite: bool,
}

/// Construct the GitHub release download URL for a pre-built binary.
///
/// Note: `gpu` is taken by reference to avoid ownership issues.
/// Note: The `tag` parameter is the release tag (e.g. "b8407"), kept
/// separate from any GPU version strings to avoid shadowing.
pub fn get_prebuilt_url(
    backend: &BackendType,
    tag: &str,
    os: &str,
    arch: &str,
    gpu: Option<&GpuType>,
) -> Result<String> {
    match backend {
        BackendType::LlamaCpp => {
            let base = format!(
                "https://github.com/ggml-org/llama.cpp/releases/download/{}/",
                tag
            );

            let filename = match (os, arch, gpu) {
                // Linux
                ("linux", "x86_64", Some(GpuType::Vulkan)) => {
                    format!("llama-{}-bin-ubuntu-vulkan-x64.tar.gz", tag)
                }
                ("linux", "x86_64", Some(GpuType::RocM { .. })) => {
                    format!("llama-{}-bin-ubuntu-rocm-7.2-x64.tar.gz", tag)
                }
                ("linux", "x86_64", _) => {
                    // CPU and CUDA both use the ubuntu-x64 build
                    // (llama.cpp doesn't ship Linux CUDA pre-built binaries)
                    format!("llama-{}-bin-ubuntu-x64.tar.gz", tag)
                }
                // Windows
                ("windows", "x86_64", Some(GpuType::Cuda { ref version })) => {
                    let cuda_ver = match version.as_str() {
                        "11" | "11.0" | "11.1" | "11.2" | "11.3" | "11.4" | "11.5" | "11.6" | "11.7" | "11.8" => "11.1",
                        "12" | "12.0" | "12.1" | "12.2" | "12.3" | "12.4" | "12.5" | "12.6" => "12.4",
                        "13" | "13.0" | "13.1" | "13.2" | "13.3" | "13.4" | "13.5" | "13.6" | "13.7" => "13.1",
                        _ => {
                            return Err(anyhow!(
                                "Unsupported CUDA version '{}'. Supported: 11.x, 12.x, 13.x.\n\
                                 llama.cpp pre-built binaries only ship with CUDA 11.1, 12.4, and 13.1.",
                                version
                            ));
                        }
                    };
                    format!("llama-{}-bin-win-cuda-{}-x64.zip", tag, cuda_ver)
                }
                ("windows", "x86_64", Some(GpuType::Vulkan)) => {
                    format!("llama-{}-bin-win-vulkan-x64.zip", tag)
                }
                ("windows", "x86_64", _) => {
                    format!("llama-{}-bin-win-cpu-x64.zip", tag)
                }
                ("windows", "aarch64", _) => {
                    format!("llama-{}-bin-win-cpu-arm64.zip", tag)
                }
                // macOS
                ("macos", "aarch64", _) => {
                    format!("llama-{}-bin-macos-arm64.tar.gz", tag)
                }
                ("macos", "x86_64", _) => {
                    format!("llama-{}-bin-macos-x64.tar.gz", tag)
                }
                _ => return Err(anyhow!("Unsupported platform: {} {}", os, arch)),
            };

            Ok(format!("{}{}", base, filename))
        }
        BackendType::IkLlama => {
            Err(anyhow!(
                "ik_llama does not provide pre-built release binaries. Use --build to build from source."
            ))
        }
        BackendType::Custom => {
            Err(anyhow!("Custom backends must be added manually"))
        }
    }
}

pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let client = Client::builder()
        .user_agent("kronk-backend-manager")
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(30))
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download from {}", url))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_size = response.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut file = tokio::fs::File::create(dest).await?;
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);
    }

    // Flush to ensure all data is written to disk before returning
    file.flush().await?;

    pb.finish_with_message("Download complete");
    Ok(())
}

/// Extract an archive (.zip or .tar.gz) to `dest` and return path to the llama-server binary.
///
/// Uses pure-Rust crates for extraction (flate2 + tar for .tar.gz, zip for .zip).
/// No external commands are required -- this works on any platform without tar in PATH.
//#[allow(unused_imports)]
#[allow(unused_imports)]
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dest)?;

    let filename = archive
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("Invalid archive path"))?;

    if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        let tar_file = std::fs::File::open(archive)
            .with_context(|| format!("Failed to open archive {:?}", archive))?;
        let gz = flate2::read::GzDecoder::new(tar_file);
        let mut tar_archive = tar::Archive::new(gz);
        tar_archive
            .unpack(dest)
            .with_context(|| "Failed to extract tar.gz archive")?;

        // Set executable permissions on extracted files (tar crate preserves
        // unix modes, but only if the archive contains them)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Recursively find and chmod all llama-* binaries
            fn chmod_recursively(path: &Path) {
                if let Ok(entries) = std::fs::read_dir(path) {
                    for entry in entries.flatten() {
                        let entry_path = entry.path();
                        if entry_path.is_file() {
                            if let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) {
                                if name.starts_with("llama-") {
                                    if let Ok(meta) = entry_path.metadata() {
                                        let mode = meta.permissions().mode();
                                        if mode & 0o111 == 0 {
                                            let mut perms = meta.permissions();
                                            perms.set_mode(0o755);
                                            let _ = std::fs::set_permissions(&entry_path, perms);
                                        }
                                    }
                                }
                            }
                        } else if entry_path.is_dir() {
                            chmod_recursively(&entry_path);
                        }
                    }
                }
            }
            chmod_recursively(dest);
        }
    } else if filename.ends_with(".zip") {
        let file = std::fs::File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file)?;

        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;

            // Reject symlinks (CVE-2025-29787: symlink-based path traversal)
            if entry.is_symlink() {
                return Err(anyhow!("Symlinks not allowed in archive: {}", entry.name()));
            }

            // Sanitize path to prevent CVE-2025-29787 (path traversal via symlinks)
            let entry_name = entry.name();
            let sanitized = entry_name.replace('\\', "/");
            if sanitized.contains("..") || sanitized.starts_with('/') || sanitized.is_empty() {
                return Err(anyhow!("Malicious path in archive: {}", entry_name));
            }
            // Reject Windows absolute paths with drive letters (e.g., "C:/...")
            if sanitized.len() >= 3
                && sanitized.chars().nth(1) == Some(':')
                && sanitized.chars().nth(2) == Some('/')
            {
                return Err(anyhow!("Windows absolute path not allowed: {}", entry_name));
            }

            let outpath = dest.join(&sanitized);

            if entry_name.ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    std::fs::create_dir_all(p)?;
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut entry, &mut outfile)?;
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = entry.unix_mode() {
                    std::fs::set_permissions(&outpath, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
    } else {
        return Err(anyhow!("Unsupported archive format: {}", filename));
    }

    find_backend_binary(dest)
}

/// Recursively search for the llama-server binary in the extracted directory.
fn find_backend_binary(dir: &Path) -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let binary_name = "llama-server.exe";
    #[cfg(not(target_os = "windows"))]
    let binary_name = "llama-server";

    // Walk the directory tree to find the binary
    fn walk_for(dir: &Path, name: &str) -> Option<PathBuf> {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.file_name().map(|n| n == name).unwrap_or(false) {
                    return Some(path);
                }
                if path.is_dir() {
                    if let Some(found) = walk_for(&path, name) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    walk_for(dir, binary_name)
        .ok_or_else(|| anyhow!("Could not find {} in extracted archive", binary_name))
}

/// Main entry point for installing a backend.
///
/// Clones `source` from `options` before matching so that `options` fields
/// remain accessible inside each arm.
pub async fn install_backend(options: InstallOptions) -> Result<PathBuf> {
    let source = options.source.clone();
    match source {
        BackendSource::Prebuilt { version } => install_prebuilt(&options, &version).await,
        BackendSource::SourceCode { version, git_url } => {
            install_from_source(&options, &version, &git_url).await
        }
    }
}

/// Prepare the target directory for installation.
///
/// If `allow_overwrite` is false and the directory exists, returns an error.
/// If `allow_overwrite` is true, removes existing contents and recreates the directory.
fn prepare_target_dir(target_dir: &Path, allow_overwrite: bool) -> Result<()> {
    if target_dir.exists() {
        if !allow_overwrite {
            return Err(anyhow!(
                "Backend directory already exists at: {}\n\
                 Use `kronk backend remove <name>` to uninstall first, or specify a different name.",
                target_dir.display()
            ));
        }
        // Overwrite: clean and recreate
        std::fs::remove_dir_all(target_dir)?;
    }
    // Always create the directory (fresh install or update)
    std::fs::create_dir_all(target_dir)?;
    Ok(())
}

async fn install_prebuilt(options: &InstallOptions, version: &str) -> Result<PathBuf> {
    tracing::info!(
        "Installing pre-built binary for {:?} version {}",
        options.backend_type,
        version
    );

    prepare_target_dir(&options.target_dir, options.allow_overwrite)?;

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let url = get_prebuilt_url(
        &options.backend_type,
        version,
        os,
        arch,
        options.gpu_type.as_ref(),
    )?;

    println!("Downloading from: {}", url);

    let download_dir = tempfile::tempdir()?;
    let archive_name = url
        .split('/')
        .last()
        .ok_or_else(|| anyhow!("Invalid download URL: {}", url))?;
    let archive_path = download_dir.path().join(archive_name);

    download_file(&url, &archive_path).await?;

    println!("Extracting archive...");
    let binary_path = extract_archive(&archive_path, &options.target_dir)?;

    println!("Backend installed at: {:?}", binary_path);
    Ok(binary_path)
}

async fn install_from_source(
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
    println!("Cloning repository (shallow)...");
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

    if !clone_result.success() {
        // For "latest", try to resolve the most recent tag first
        let mut cloned_from_tag = false;
        if version == "latest" {
            // Attempt to find the latest tag
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
                        .map(|l| *l)
                        .collect();
                    if let Some(tag_line) = tag_lines.iter().find(|l| !l.is_empty()) {
                        // Parse ref field (second tab-separated value), strip "refs/tags/" prefix and trailing "^{}"
                        let ref_field: &str =
                            tag_line.split('\t').nth(1).unwrap_or("refs/tags/unknown");
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
                            cloned_from_tag = true;
                        }
                    }
                }
                _ => {}
            }
        }

        // Only allow fallback to HEAD for "main" or "latest" (tags may not exist)
        if version != "main" && version != "latest" {
            return Err(anyhow!(
                "Tag/branch '{}' not found. Only 'main' or 'latest' are allowed for fallback.\n\
                 Use an explicit version tag (e.g., 'b8407') or specify --build to build from source.",
                version
            ));
        }

        // Skip fallback if we successfully cloned from a tag
        if cloned_from_tag {
            // Already cloned from tag, skip fallback
        } else {
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
        }
    }
    let build_output = build_dir.path().join("build");
    std::fs::create_dir_all(&build_output)?;

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

    // Build
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

    // Find and copy the binary
    println!("Installing binary...");
    let binary_src = find_backend_binary(&build_output)?;

    std::fs::create_dir_all(&options.target_dir)?;
    let binary_name = binary_src
        .file_name()
        .ok_or_else(|| anyhow!("Could not determine binary filename"))?;
    let binary_dest = options.target_dir.join(binary_name);

    std::fs::copy(&binary_src, &binary_dest)?;

    // Set executable permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&binary_dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&binary_dest, perms)?;
    }

    println!("Backend built and installed at: {:?}", binary_dest);
    Ok(binary_dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::GpuType;

    #[test]
    fn test_llama_cpp_download_url_linux_cpu() {
        let url =
            get_prebuilt_url(&BackendType::LlamaCpp, "b8407", "linux", "x86_64", None).unwrap();

        assert_eq!(
            url,
            "https://github.com/ggml-org/llama.cpp/releases/download/b8407/llama-b8407-bin-ubuntu-x64.tar.gz"
        );
    }

    #[test]
    fn test_llama_cpp_download_url_windows_cuda() {
        let url = get_prebuilt_url(
            &BackendType::LlamaCpp,
            "b8407",
            "windows",
            "x86_64",
            Some(&GpuType::Cuda {
                version: "12.4".to_string(),
            }),
        )
        .unwrap();

        assert!(url.contains("cuda-12.4"));
        assert!(url.contains("b8407"));
    }

    #[test]
    fn test_llama_cpp_download_url_windows_vulkan() {
        let url = get_prebuilt_url(
            &BackendType::LlamaCpp,
            "b8407",
            "windows",
            "x86_64",
            Some(&GpuType::Vulkan),
        )
        .unwrap();

        assert!(url.contains("vulkan"));
    }

    #[test]
    fn test_ik_llama_prebuilt_not_available() {
        let result = get_prebuilt_url(&BackendType::IkLlama, "main", "linux", "x86_64", None);
        assert!(result.is_err());
    }
}
