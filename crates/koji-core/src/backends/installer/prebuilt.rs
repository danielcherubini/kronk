use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};

use super::download::download_file;
use super::extract::extract_archive;
use super::urls::get_prebuilt_url;
use super::InstallOptions;
use super::ProgressSink;

/// Emit a log line through the progress sink, or println if no sink is provided.
fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    match sink {
        Some(s) => s.log(&line),
        None => println!("{line}"),
    }
}

/// Prepare the target directory for installation.
///
/// If `allow_overwrite` is false and the directory exists, returns an error.
/// If `allow_overwrite` is true, removes existing contents and recreates the directory.
pub fn prepare_target_dir(target_dir: &Path, allow_overwrite: bool) -> Result<()> {
    if target_dir.exists() {
        if !allow_overwrite {
            return Err(anyhow!(
                "Backend directory already exists at: {}\n\
                 Use `koji backend remove <name>` to uninstall first, or specify a different name.",
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

/// Install a pre-built backend binary from GitHub releases.
pub async fn install_prebuilt(
    options: &InstallOptions,
    version: &str,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<PathBuf> {
    emit(
        progress,
        format!(
            "Installing pre-built binary for {:?} version {}",
            options.backend_type, version
        ),
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

    emit(progress, format!("Downloading from: {}", url));

    let download_dir = tempfile::tempdir()?;
    let archive_name = url
        .split('/')
        .next_back()
        .ok_or_else(|| anyhow!("Invalid download URL: {}", url))?;
    let archive_path = download_dir.path().join(archive_name);

    download_file(&url, &archive_path, progress).await?;

    emit(progress, "Extracting archive...");
    let binary_path = extract_archive(&archive_path, &options.target_dir)?;

    emit(progress, format!("Backend installed at: {:?}", binary_path));
    Ok(binary_path)
}
