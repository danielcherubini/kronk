use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use reqwest::Client;

use super::download::download_with_client;
use super::extract::extract_archive;
use super::urls::get_prebuilt_url;
use super::InstallOptions;
use super::ProgressSink;

/// Emit a log line to both the progress sink and the tracing subsystem.
fn emit(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    tracing::info!(target: "tama_core::backends::installer", "{}", line);
    match sink {
        Some(s) => s.log(&line),
        None => println!("{line}"),
    }
}

/// Emit an error to both the progress sink and the tracing subsystem.
fn emit_error(sink: Option<&Arc<dyn ProgressSink>>, line: impl Into<String>) {
    let line = line.into();
    tracing::error!(target: "tama_core::backends::installer", "{}", line);
    match sink {
        Some(s) => s.log(&line),
        None => eprintln!("{line}"),
    }
}

/// Prepare the target directory for installation.
///
/// With the versioned path structure (`backends/<type>/<variant>/<version>/`):
/// - Always creates parent directories (`backends/<type>/<variant>/`) if missing.
/// - Only checks existence of the final version directory (`<version>/`).
/// - When `allow_overwrite` is true, removes only the version directory.
/// - When `allow_overwrite` is false and the version directory exists, returns an error.
pub fn prepare_target_dir(target_dir: &Path, allow_overwrite: bool) -> Result<()> {
    // Always create parent directories (backends/<type>/<variant>/)
    if let Some(parent) = target_dir.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create parent directories: {}", parent.display())
        })?;
    }

    // Only check existence of the final version directory (<version>/)
    if target_dir.exists() {
        if !allow_overwrite {
            let msg = format!(
                "Backend directory already exists at: {}\n\
                 Use `tama backend remove <name>` to uninstall first, or specify a different name.",
                target_dir.display()
            );
            tracing::error!(target: "tama_core::backends::installer", "{}", msg);
            return Err(anyhow!("{}", msg));
        }
        tracing::info!(
            target: "tama_core::backends::installer",
            "Overwriting existing backend directory: {}",
            target_dir.display()
        );
        // Overwrite: remove only the version directory
        std::fs::remove_dir_all(target_dir)?;
    }
    // Create the version directory (fresh install or after overwrite)
    std::fs::create_dir_all(target_dir)?;
    Ok(())
}

/// Install a pre-built backend binary from GitHub releases.
pub async fn install_prebuilt(
    options: &InstallOptions,
    version: &str,
    progress: Option<&Arc<dyn ProgressSink>>,
    client: Option<&Client>,
) -> Result<PathBuf> {
    emit(
        progress,
        format!(
            "Installing pre-built binary for {:?} version {}",
            options.backend_type, version
        ),
    );

    if let Err(e) = prepare_target_dir(&options.target_dir, options.allow_overwrite) {
        emit_error(
            progress,
            format!("Failed to prepare target directory: {}", e),
        );
        return Err(e);
    }

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let url = match get_prebuilt_url(
        &options.backend_type,
        version,
        os,
        arch,
        options.gpu_type.as_ref(),
    ) {
        Ok(u) => u,
        Err(e) => {
            emit_error(progress, format!("Failed to construct download URL: {}", e));
            return Err(e);
        }
    };

    emit(progress, format!("Downloading from: {}", url));

    let download_dir = tempfile::tempdir()?;
    let archive_name = url
        .split('/')
        .next_back()
        .ok_or_else(|| anyhow!("Invalid download URL: {}", url))?;
    let archive_path = download_dir.path().join(archive_name);

    if let Err(e) = download_with_client(&url, &archive_path, progress, client).await {
        emit_error(progress, format!("Download failed: {}", e));
        return Err(e);
    }

    emit(progress, "Extracting archive...");
    let binary_path = match extract_archive(&archive_path, &options.target_dir) {
        Ok(p) => p,
        Err(e) => {
            emit_error(progress, format!("Extraction failed: {}", e));
            return Err(e);
        }
    };

    emit(progress, format!("Backend installed at: {:?}", binary_path));
    Ok(binary_path)
}
