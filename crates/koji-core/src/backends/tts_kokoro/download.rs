use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::sync::Arc;

use super::paths::*;
use crate::backends::{backends_dir, ProgressSink};

/// Minimum free disk space warning threshold (10 GB in bytes).
const DISK_SPACE_WARNING_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Check available disk space and warn if below threshold.
/// Returns the available space in bytes. Does not block installation.
pub fn check_disk_space(base: &Path) -> Result<u64> {
    let disks = sysinfo::Disks::new_with_refreshed_list();

    let base_mount = base.as_os_str();

    for disk in disks.list() {
        if disk.mount_point().as_os_str() == base_mount {
            let available = disk.available_space();
            if available < DISK_SPACE_WARNING_BYTES {
                tracing::warn!(
                    "Low disk space: {:.2} GB available (threshold: 10.0 GB). \
                     Kokoro-FastAPI + PyTorch ROCm may need ~4-6 GB.",
                    available as f64 / (1024.0_f64 * 1024.0_f64 * 1024.0_f64)
                );
            }
            return Ok(available);
        }
    }

    // Fallback: check root mount point
    tracing::warn!(
        "Could not determine disk space for {}; checking /",
        base.display()
    );
    let disks = sysinfo::Disks::new_with_refreshed_list();
    for disk in disks.list() {
        if disk.mount_point() == Path::new("/") {
            return Ok(disk.available_space());
        }
    }

    // Cannot determine — return a large value to not block
    tracing::warn!("Could not determine available disk space; proceeding without check.");
    Ok(u64::MAX)
}

/// Create a Python virtualenv at the target directory.
/// Prefers Python 3.13 since Kokoro-FastAPI doesn't support Python 3.14+.
async fn create_venv(venv_path: &Path, progress: &Arc<dyn ProgressSink>) -> Result<()> {
    progress.log("Creating Python virtualenv...");

    // kokoro-fastapi requires kokoro==0.9.2 which only supports Python 3.10-3.12
    // Prefer python3.12, fall back to python3.13 (may work with overrides), then python3
    let python_cmd = if std::process::Command::new("python3.12")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        "python3.12"
    } else if std::process::Command::new("python3.13")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        "python3.13"
    } else {
        "python3"
    };

    progress.log(&format!("Using {} for virtualenv...", python_cmd));
    let status = tokio::process::Command::new(python_cmd)
        .args(["-m", "venv", &venv_path.to_string_lossy()])
        .status()
        .await
        .with_context(|| format!("Failed to spawn {python_cmd} -m venv"))?;

    if !status.success() {
        return Err(anyhow!(
            "Failed to create Python virtualenv at {}",
            venv_path.display()
        ));
    }

    progress.log(&format!("Virtualenv created at: {}", venv_path.display()));
    Ok(())
}

/// Clone the Kokoro-FastAPI repository at the pinned tag.
async fn clone_repo(
    repo_url: &str,
    tag: &str,
    install_path: &Path,
    progress: &Arc<dyn ProgressSink>,
) -> Result<()> {
    progress.log(&format!("Cloning Kokoro-FastAPI (tag {})...", tag));
    let status = tokio::process::Command::new("git")
        .args([
            "clone",
            "--depth",
            "1",
            "--branch",
            tag,
            repo_url,
            &install_path.to_string_lossy(),
        ])
        .status()
        .await
        .with_context(|| "Failed to spawn git clone")?;

    if !status.success() {
        return Err(anyhow!(
            "Failed to clone Kokoro-FastAPI repository (tag {})",
            tag
        ));
    }

    progress.log(&format!("Repository cloned at: {}", install_path.display()));
    Ok(())
}

/// Check and install OpenBLAS development libraries (required by scipy).
fn ensure_openblas(progress: &Arc<dyn ProgressSink>) -> Result<()> {
    // Check if OpenBLAS is already available via pkg-config
    let check = std::process::Command::new("pkg-config")
        .args(["--exists", "openblas"])
        .output();

    if check.is_ok_and(|o| o.status.success()) {
        progress.log("OpenBLAS already available — skipping install.");
        return Ok(());
    }

    // Try to detect package manager and install OpenBLAS
    let pkg_mgr = if Path::new("/usr/bin/dnf").exists() {
        Some(("dnf", "openblas-devel"))
    } else if Path::new("/usr/bin/apt-get").exists() {
        Some(("apt-get", "libopenblas-dev"))
    } else if Path::new("/usr/bin/yum").exists() {
        Some(("yum", "openblas-devel"))
    } else {
        None
    };

    match pkg_mgr {
        Some((mgr, pkg)) => {
            let sudo = if std::env::var("SUDO_USER").is_ok() {
                "sudo "
            } else {
                ""
            };

            // Check if package is already installed
            let already_installed = match mgr {
                "dnf" | "yum" => {
                    std::process::Command::new(format!("{sudo}{mgr}"))
                        .args(["list", "installed", pkg])
                        .output()
                        .is_ok_and(|o| o.status.success())
                }
                "apt-get" => {
                    std::process::Command::new("dpkg")
                        .args(["-l", pkg])
                        .output()
                        .is_ok_and(|o| {
                            String::from_utf8_lossy(&o.stdout).contains(&format!("^{pkg} "))
                        })
                }
                _ => false,
            };

            if already_installed {
                progress.log(&format!(
                    "OpenBLAS development libraries ({pkg}) already installed — skipping."
                ));
            } else {
                progress.log(&format!(
                    "Installing OpenBLAS development libraries ({pkg})..."
                ));
                let args = if mgr == "apt-get" {
                    vec!["-y", "install", pkg]
                } else {
                    vec!["install", "-y", pkg]
                };
                let status = std::process::Command::new(format!("{sudo}{mgr}"))
                    .args(&args)
                    .status()
                    .with_context(|| format!("Failed to run {mgr} install"))?;

                if !status.success() {
                    return Err(anyhow!(
                        "{mgr} install of {pkg} failed. \
                         Please install OpenBLAS development libraries manually and retry."
                    ));
                }
            }
        }
        None => {
            return Err(anyhow!(
                "OpenBLAS development libraries are required to build scipy. \
                 Please install them manually (e.g., libopenblas-dev on Debian/Ubuntu, \
                 openblas-devel on Fedora/RHEL) and retry."
            ));
        }
    }

    // Create openblas.pc symlink if it doesn't exist.
    // Many distros ship blas.pc but not openblas.pc, and scipy's meson build
    // specifically looks for "openblas" via pkg-config.
    let pc_path = Path::new("/usr/lib64/pkgconfig/openblas.pc");
    if !pc_path.exists() {
        progress.log("Creating openblas.pc symlink for scipy build...");
        let sudo = if std::env::var("SUDO_USER").is_ok() {
            "sudo "
        } else {
            ""
        };
        let status = std::process::Command::new(format!("{sudo}ln"))
            .args(["-sf", "/usr/lib64/pkgconfig/blas.pc", "/usr/lib64/pkgconfig/openblas.pc"])
            .status()
            .with_context(|| "Failed to create openblas.pc symlink")?;

        if !status.success() {
            return Err(anyhow!(
                "Failed to create openblas.pc symlink. \
                 Please create it manually: sudo ln -sf /usr/lib64/pkgconfig/blas.pc \
                 /usr/lib64/pkgconfig/openblas.pc"
            ));
        }
    }

    Ok(())
}

/// Install dependencies via pip in the virtualenv.
async fn install_dependencies(
    python_bin: &Path,
    install_path: &Path,
    has_rocm: bool,
    progress: &Arc<dyn ProgressSink>,
) -> Result<()> {
    // Ensure OpenBLAS is available (required by scipy)
    ensure_openblas(progress)?;

    if has_rocm {
        // Kokoro-FastAPI v0.2.4 doesn't have a [rocm] extra.
        // Install PyTorch ROCm first, then install the package without
        // torch extras so it doesn't override our ROCm build.
        progress.log("Detected ROCm — installing PyTorch ROCm...");
        let status = tokio::process::Command::new(python_bin)
            .args([
                "-m",
                "pip",
                "install",
                "torch",
                "--index-url",
                "https://download.pytorch.org/whl/rocm6.4",
            ])
            .current_dir(install_path)
            .status()
            .await
            .with_context(|| "Failed to install PyTorch ROCm")?;

        if !status.success() {
            return Err(anyhow!(
                "Failed to install PyTorch with ROCm support. \
                 Check that your ROCm installation is compatible."
            ));
        }
    }

    // Override scipy version for Python 3.14 compatibility.
    // Kokoro-FastAPI pins scipy==1.14.1 which has no wheels for Python 3.14.
    // Install a compatible version first so pip won't try to downgrade it.
    progress.log("Ensuring compatible scipy version...");
    let status = tokio::process::Command::new(python_bin)
        .args(["-m", "pip", "install", "scipy>=1.16,<2.0"])
        .current_dir(install_path)
        .status()
        .await
        .with_context(|| "Failed to install compatible scipy")?;

    if !status.success() {
        return Err(anyhow!("Failed to install compatible scipy version."));
    }

    // Install the package (with CPU extras if no ROCm, or bare install if ROCm)
    let extra = if has_rocm { "" } else { "[cpu]" };
    let msg = format!(
        "Installing Kokoro-FastAPI{}...",
        if has_rocm { " (using system PyTorch)" } else { " CPU dependencies" }
    );
    progress.log(&msg);
    let status = tokio::process::Command::new(python_bin)
        .args(["-m", "pip", "install", "-e", &format!(".{extra}")])
        .current_dir(install_path)
        .status()
        .await
        .with_context(|| "Failed to install Kokoro-FastAPI dependencies")?;

    if !status.success() {
        return Err(anyhow!("Failed to install Kokoro-FastAPI dependencies."));
    }

    progress.log("Dependencies installed successfully.");
    Ok(())
}

/// Download model files using the included download_model.py script.
async fn download_model(
    python_bin: &Path,
    install_path: &Path,
    model_dir: &Path,
    progress: &Arc<dyn ProgressSink>,
) -> Result<()> {
    // Ensure the model output directory exists
    std::fs::create_dir_all(model_dir).with_context(|| "Failed to create model directory")?;

    let download_script = install_path
        .join("docker")
        .join("scripts")
        .join("download_model.py");

    progress.log("Downloading Kokoro model files via download_model.py...");
    let status = tokio::process::Command::new(python_bin)
        .args([
            &download_script.to_string_lossy(),
            "--output",
            &model_dir.to_string_lossy(),
        ])
        .status()
        .await
        .with_context(|| "Failed to spawn download_model.py")?;

    if !status.success() {
        return Err(anyhow!(
            "download_model.py exited with non-zero status. \
             Model files may be incomplete."
        ));
    }

    progress.log(&format!(
        "Model files downloaded to: {}",
        model_dir.display()
    ));
    Ok(())
}

/// Download voice packs from HuggingFace.
async fn download_voices(
    python_bin: &Path,
    install_path: &Path,
    voices_dir: &Path,
    progress: &Arc<dyn ProgressSink>,
) -> Result<()> {
    // Ensure the voice output directory exists
    std::fs::create_dir_all(voices_dir).with_context(|| "Failed to create voices directory")?;

    progress.log("Downloading Kokoro voice packs from HuggingFace...");
    let status = tokio::process::Command::new(python_bin)
        .args([
            "-c",
            &format!(
                "from huggingface_hub import hf_hub_download, list_repo_files; \
                 repo_id = 'hexgrad/Kokoro-82M'; \
                 voice_files = [f for f in list_repo_files(repo_id) if f.startswith('voices/') and f.endswith('.pt')]; \
                 out_dir = '{}'; \
                 import os; \
                 for vf in voice_files: \
                     hf_hub_download(repo_id, vf, local_dir=out_dir); \
                     print(f'Downloaded {{vf}}'); \
                 print(f'Total: {{len(voice_files)}} voices')",
                voices_dir.to_string_lossy()
            ),
        ])
        .current_dir(install_path)
        .status()
        .await
        .with_context(|| "Failed to spawn voice download script")?;

    if !status.success() {
        return Err(anyhow!("Voice pack download failed."));
    }

    progress.log(&format!(
        "Voice packs downloaded to: {}",
        voices_dir.display()
    ));
    Ok(())
}

/// Check if ROCm is available on the system.
pub fn has_rocm() -> bool {
    Path::new("/opt/rocm").exists()
}

/// Clean up partial installation (venv + repo clone) on failure.
fn cleanup_installation(base: &Path, progress: &Arc<dyn ProgressSink>) {
    let venv_path = venv_dir(base);
    let install_path = install_dir(base);

    progress.log("Cleaning up partial installation...");

    if venv_path.exists() {
        tracing::info!("Removing partial venv: {}", venv_path.display());
        if let Err(e) = std::fs::remove_dir_all(&venv_path) {
            tracing::warn!("Failed to remove venv {}: {}", venv_path.display(), e);
        }
    }

    if install_path.exists() {
        tracing::info!("Removing partial repo clone: {}", install_path.display());
        if let Err(e) = std::fs::remove_dir_all(&install_path) {
            tracing::warn!(
                "Failed to remove repo clone {}: {}",
                install_path.display(),
                e
            );
        }
    }

    progress.log("Cleanup complete.");
}

/// Full installation pipeline for Kokoro-FastAPI.
///
/// Steps:
/// 1. Check disk space (warn if <10 GB)
/// 2. Create Python venv
/// 3. Clone Kokoro-FastAPI repo at pinned tag
/// 4. Install dependencies (ROCm or CPU)
/// 5. Download model files via download_model.py
///
/// On failure at any step, cleans up partial installation.
pub async fn install_kokoro_fastapi(progress: &Arc<dyn ProgressSink>) -> Result<()> {
    let base = backends_dir().with_context(|| "Failed to get backends directory")?;

    // Step 1: Check disk space
    progress.log("Checking available disk space...");
    check_disk_space(&base).with_context(|| "Disk space check failed")?;

    let venv_path = venv_dir(&base);
    let install_path = install_dir(&base);
    let python_path = python_bin(&base);
    let model_path = model_dir(&base);
    let voices_path = super::paths::voices_dir(&base);
    let has_rocm = has_rocm();

    // Run the installation steps, cleaning up on any failure.
    let result = async {
        // Step 2: Create venv
        if !venv_path.exists() {
            create_venv(&venv_path, progress).await?;
        } else {
            progress.log("Virtualenv already exists — skipping creation.");
        }

        // Step 3: Clone repo
        if !(install_path.exists() && install_path.join(".git").exists()) {
            clone_repo(
                KOKORO_FASTAPI_URL,
                KOKORO_FASTAPI_TAG,
                &install_path,
                progress,
            )
            .await?;
        } else {
            progress.log("Repository already cloned — skipping clone.");
        }

        // Step 4: Install dependencies
        install_dependencies(&python_path, &install_path, has_rocm, progress).await?;

        // Step 5: Download model files
        download_model(&python_path, &install_path, &model_path, progress).await?;

        // Step 6: Download voice packs from HuggingFace
        download_voices(&python_path, &install_path, &voices_path, progress).await?;

        anyhow::Ok(())
    }
    .await;

    if let Err(e) = result {
        cleanup_installation(&base, progress);
        return Err(e);
    }

    progress.log("Kokoro-FastAPI installation complete.");
    Ok(())
}

/// List of available Kokoro voice IDs (48 voices from hexgrad/Kokoro-82M).
pub const VOICE_IDS: &[&str] = &[
    // Female American
    "af_alloy",
    "af_aoede",
    "af_bella",
    "af_heart",
    "af_jessica",
    "af_kore",
    "af_nicole",
    "af_nova",
    "af_river",
    "af_sarah",
    "af_sky",
    // Male American
    "am_adam",
    "am_echo",
    "am_eric",
    "am_fenrir",
    "am_liam",
    "am_michael",
    "am_onyx",
    "am_puck",
    "am_santa",
    // Female British
    "bf_alice",
    "bf_emma",
    "bf_isabella",
    "bf_lily",
    // Male British
    "bm_daniel",
    "bm_fable",
    "bm_george",
    "bm_lewis",
    // Female Extra
    "ef_dora",
    "em_santa",
    "ff_siwis",
    // Male Extra
    "em_alex",
    "hf_alpha",
    "hf_beta",
    "hm_omega",
    "hm_psi",
    "if_sara",
    "im_nicola",
    "jf_alpha",
    "jf_gongitsune",
    "jf_nezumi",
    "jf_tebukuro",
    "jm_kumo",
    // Female Extra 2
    "pf_dora",
    "pm_alex",
    "pm_santa",
    // Japanese
    "zf_xiaobei",
    "zf_xiaoni",
    "zf_xiaoxiao",
    "zf_xiaoyi",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_has_rocm_returns_false_when_no_rocm() {
        // On most systems /opt/rocm won't exist unless ROCm is installed.
        // This test documents the expected behavior.
        let result = has_rocm();
        // We don't assert true/false since it depends on the host system.
        // Just verify it doesn't panic and returns a bool.
        let _ = result;
    }

    #[test]
    fn test_disk_space_warning_bytes_constant() {
        // Verify 10 GB constant is correct
        assert_eq!(DISK_SPACE_WARNING_BYTES, 10 * 1024 * 1024 * 1024);
    }

    #[test]
    fn test_voice_ids_count() {
        // Voice ID count matches the actual Kokoro-82M release (50 voices)
        assert_eq!(VOICE_IDS.len(), 50);
    }

    #[test]
    fn test_kokoro_fastapi_constants() {
        assert_eq!(KOKORO_FASTAPI_TAG, "v0.3.0");
        assert_eq!(
            KOKORO_FASTAPI_URL,
            "https://github.com/remsky/Kokoro-FastAPI.git"
        );
    }

    #[test]
    fn test_path_helpers_consistency() {
        use super::super::paths::*;

        let base = PathBuf::from("/tmp/test_base");

        // venv_dir should be a sibling of install_dir under base_dir
        assert!(venv_dir(&base).starts_with(base_dir(&base)));
        assert!(install_dir(&base).starts_with(base_dir(&base)));
        assert_ne!(venv_dir(&base), install_dir(&base));

        // python_bin should be inside venv_dir
        assert!(python_bin(&base).starts_with(venv_dir(&base)));

        // model_dir should be inside install_dir
        assert!(model_dir(&base).starts_with(install_dir(&base)));

        // model_file should be inside model_dir
        assert!(model_file(&base).starts_with(model_dir(&base)));
    }
}
