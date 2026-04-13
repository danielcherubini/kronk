//! Self-update functionality for the Koji binary.
//!
//! Provides the ability to check for new releases on GitHub, download and install
//! updates, and restart the process. Uses the `self_update` crate's lower-level API
//! for fine-grained progress reporting.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;

/// GitHub repository owner for Koji releases.
pub const REPO_OWNER: &str = "danielcherubini";

/// GitHub repository name for Koji releases.
pub const REPO_NAME: &str = "koji";

/// Information about an available update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub published_at: String,
    pub update_available: bool,
}

/// Result of a successful update operation.
#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub old_version: String,
    pub new_version: String,
}

/// Read the `GITHUB_TOKEN` env var for API authentication.
fn github_token() -> Option<String> {
    std::env::var("GITHUB_TOKEN").ok()
}

/// Check whether a newer version of Koji is available on GitHub Releases.
///
/// Accepts `current_version` as a parameter so the caller passes the correct
/// binary version (e.g. from koji-cli), avoiding version mismatch with
/// `env!("CARGO_PKG_VERSION")` which resolves to the crate's own version.
pub async fn check_for_update(current_version: &str) -> Result<UpdateInfo> {
    let current = current_version.to_string();

    tokio::task::spawn_blocking(move || check_for_update_sync(&current))
        .await
        .context("spawn_blocking panicked")?
}

/// Synchronous implementation of update checking.
fn check_for_update_sync(current_version: &str) -> Result<UpdateInfo> {
    let mut builder = self_update::backends::github::ReleaseList::configure();
    builder.repo_owner(REPO_OWNER);
    builder.repo_name(REPO_NAME);

    if let Some(token) = github_token() {
        builder.auth_token(&token);
    }

    let releases = builder
        .build()
        .context("Failed to configure release list")?
        .fetch()
        .context("Failed to fetch releases from GitHub")?;

    let latest = match releases.first() {
        Some(r) => r,
        None => {
            return Ok(UpdateInfo {
                current_version: current_version.to_string(),
                latest_version: current_version.to_string(),
                release_notes: String::new(),
                published_at: String::new(),
                update_available: false,
            });
        }
    };

    let current_semver = semver::Version::parse(current_version)
        .with_context(|| format!("Invalid current version: {current_version}"))?;
    let latest_semver = semver::Version::parse(&latest.version)
        .with_context(|| format!("Invalid release version: {}", latest.version))?;

    Ok(UpdateInfo {
        current_version: current_version.to_string(),
        latest_version: latest.version.clone(),
        release_notes: latest.body.clone().unwrap_or_default(),
        published_at: latest.date.clone(),
        update_available: latest_semver > current_semver,
    })
}

/// Download and install the latest Koji release, replacing the running binary.
///
/// Uses the `self_update` crate's lower-level API for fine-grained progress
/// reporting via the `on_progress` callback.
///
/// Accepts `current_version` as a parameter so the caller passes the correct
/// binary version.
pub async fn perform_update(
    current_version: &str,
    on_progress: impl Fn(String) + Send + 'static,
) -> Result<UpdateResult> {
    let current = current_version.to_string();

    tokio::task::spawn_blocking(move || perform_update_sync(&current, on_progress))
        .await
        .context("spawn_blocking panicked")?
}

/// Synchronous implementation of the update process.
fn perform_update_sync(
    current_version: &str,
    on_progress: impl Fn(String),
) -> Result<UpdateResult> {
    on_progress("Checking for latest release...".to_string());

    // 1. Fetch release list
    let mut builder = self_update::backends::github::ReleaseList::configure();
    builder.repo_owner(REPO_OWNER);
    builder.repo_name(REPO_NAME);

    if let Some(token) = github_token() {
        builder.auth_token(&token);
    }

    let releases = builder
        .build()
        .context("Failed to configure release list")?
        .fetch()
        .context("Failed to fetch releases from GitHub")?;

    let latest = releases
        .first()
        .ok_or_else(|| anyhow!("No releases found on GitHub"))?;

    // 2. Compare versions
    let current_semver = semver::Version::parse(current_version)
        .with_context(|| format!("Invalid current version: {current_version}"))?;
    let latest_semver = semver::Version::parse(&latest.version)
        .with_context(|| format!("Invalid release version: {}", latest.version))?;

    if latest_semver <= current_semver {
        bail!("Already up to date (v{current_version})");
    }

    let new_version = latest.version.clone();
    on_progress(format!("Downloading v{new_version}..."));

    // 3. Find the correct asset for this platform
    let target = self_update::get_target();
    let asset = latest
        .asset_for(target, None)
        .ok_or_else(|| anyhow!("No release asset found for target '{target}'"))?;

    tracing::info!(
        target = target,
        asset_name = %asset.name,
        download_url = %asset.download_url,
        "Found release asset for self-update"
    );

    // 4. Download to a temporary file
    let tmp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let tmp_archive = tmp_dir.path().join(&asset.name);
    let mut tmp_file = std::fs::File::create(&tmp_archive).context("Failed to create temp file")?;

    self_update::Download::from_url(&asset.download_url)
        .download_to(&mut tmp_file)
        .context("Failed to download release asset")?;

    tmp_file.flush().context("Failed to flush temp file")?;
    drop(tmp_file);

    let archive_size = std::fs::metadata(&tmp_archive)
        .map(|m| m.len())
        .unwrap_or(0);
    tracing::info!(
        archive_path = %tmp_archive.display(),
        archive_size_bytes = archive_size,
        "Downloaded release archive"
    );

    on_progress("Extracting binary...".to_string());

    // 5. Extract the binary from the archive
    let bin_name = if cfg!(target_os = "windows") {
        "koji.exe"
    } else {
        "koji"
    };

    let archive_kind = detect_archive_kind(&asset.name);
    tracing::info!(
        bin_name = bin_name,
        archive_kind = ?archive_kind,
        "Extracting binary from archive"
    );

    if let Err(extract_err) = self_update::Extract::from_source(&tmp_archive)
        .archive(archive_kind)
        .extract_file(tmp_dir.path(), bin_name)
    {
        // Log detailed diagnostic information for extraction failures
        tracing::error!(
            error = %extract_err,
            target = target,
            asset_name = %asset.name,
            archive_kind = ?archive_kind,
            archive_size_bytes = archive_size,
            bin_name = bin_name,
            tmp_dir = %tmp_dir.path().display(),
            "Failed to extract binary from archive"
        );

        // Try to list archive contents for diagnostics
        let contents = list_archive_contents(&tmp_archive, archive_kind);
        tracing::error!(archive_contents = %contents, "Archive contents at time of failure");

        bail!(
            "Failed to extract '{bin_name}' from archive '{}' \
             (target={target}, kind={archive_kind:?}, size={archive_size} bytes, \
             archive_contents=[{contents}]): {extract_err}",
            asset.name,
        );
    }

    let extracted_path = tmp_dir.path().join(bin_name);
    if !extracted_path.exists() {
        bail!(
            "Extracted binary not found at expected path: {}",
            extracted_path.display()
        );
    }

    on_progress("Replacing binary...".to_string());

    // 6. Replace the running binary
    self_update::self_replace::self_replace(&extracted_path)
        .context("Failed to replace running binary")?;

    on_progress("Update complete!".to_string());

    Ok(UpdateResult {
        old_version: current_version.to_string(),
        new_version,
    })
}

/// List the contents of an archive for diagnostic purposes.
///
/// Returns a human-readable string of entry names. On any error, returns an
/// error description instead of panicking.
fn list_archive_contents(
    archive_path: &std::path::Path,
    archive_kind: self_update::ArchiveKind,
) -> String {
    match archive_kind {
        self_update::ArchiveKind::Zip => list_zip_contents(archive_path),
        self_update::ArchiveKind::Tar(_) => list_tar_gz_contents(archive_path),
        self_update::ArchiveKind::Plain(_) => "(plain binary, no archive entries)".to_string(),
    }
}

/// List entry names inside a zip archive.
fn list_zip_contents(archive_path: &std::path::Path) -> String {
    let file = match std::fs::File::open(archive_path) {
        Ok(f) => f,
        Err(e) => return format!("<failed to open archive: {e}>"),
    };
    let archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return format!("<failed to read zip: {e}>"),
    };
    let names: Vec<&str> = (0..archive.len())
        .filter_map(|i| archive.name_for_index(i))
        .collect();
    if names.is_empty() {
        "<empty archive>".to_string()
    } else {
        names.join(", ")
    }
}

/// List entry names inside a tar.gz archive.
fn list_tar_gz_contents(archive_path: &std::path::Path) -> String {
    let file = match std::fs::File::open(archive_path) {
        Ok(f) => f,
        Err(e) => return format!("<failed to open archive: {e}>"),
    };
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => return format!("<failed to read tar entries: {e}>"),
    };
    let names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.path().ok().map(|p| p.display().to_string()))
        .collect();
    if names.is_empty() {
        "<empty archive>".to_string()
    } else {
        names.join(", ")
    }
}

/// Detect the archive kind from the filename extension.
fn detect_archive_kind(filename: &str) -> self_update::ArchiveKind {
    if filename.ends_with(".tar.gz") || filename.ends_with(".tgz") {
        self_update::ArchiveKind::Tar(Some(self_update::Compression::Gz))
    } else if filename.ends_with(".zip") {
        self_update::ArchiveKind::Zip
    } else {
        self_update::ArchiveKind::Plain(None)
    }
}

/// Detect whether the current process is running as a system service.
pub fn is_running_as_service() -> bool {
    #[cfg(target_os = "linux")]
    {
        // systemd sets INVOCATION_ID for both system and user services
        if std::env::var("INVOCATION_ID").is_ok() {
            return true;
        }
        // Fallback: JOURNAL_STREAM is also set by systemd
        if std::env::var("JOURNAL_STREAM").is_ok() {
            return true;
        }
        false
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, detect if launched via the `service-run` command
        std::env::args().any(|arg| arg == "service-run")
    }
}

/// Restart the Koji process after an update.
///
/// If running as a systemd/Windows service, uses the platform's service restart
/// mechanism. Otherwise, re-execs the current binary with the same arguments.
pub fn restart_process() -> Result<()> {
    if is_running_as_service() {
        restart_as_service()?;
    } else {
        restart_as_cli()?;
    }

    // Should not reach here — both paths call exit(0) on success
    Ok(())
}

/// Restart via the platform service manager.
fn restart_as_service() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        match crate::platform::linux::restart_service("koji") {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                tracing::warn!(
                    "Failed to restart via systemd: {e:#}. Falling back to CLI re-exec."
                );
                restart_as_cli()?;
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        match crate::platform::windows::restart_service("koji") {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                tracing::warn!(
                    "Failed to restart via Windows SCM: {e:#}. Falling back to CLI re-exec."
                );
                restart_as_cli()?;
            }
        }
    }

    Ok(())
}

/// Restart by re-execing the current binary with the same arguments.
fn restart_as_cli() -> Result<()> {
    let exe = std::env::current_exe().context("Failed to get current executable path")?;
    let args: Vec<String> = std::env::args().skip(1).collect();

    std::process::Command::new(&exe)
        .args(&args)
        .spawn()
        .with_context(|| format!("Failed to spawn new process: {}", exe.display()))?;

    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_archive_kind_tar_gz() {
        let kind = detect_archive_kind("koji-x86_64-unknown-linux-gnu.tar.gz");
        assert!(matches!(
            kind,
            self_update::ArchiveKind::Tar(Some(self_update::Compression::Gz))
        ));
    }

    #[test]
    fn test_detect_archive_kind_zip() {
        let kind = detect_archive_kind("koji-x86_64-pc-windows-msvc.zip");
        assert!(matches!(kind, self_update::ArchiveKind::Zip));
    }

    #[test]
    fn test_detect_archive_kind_plain() {
        let kind = detect_archive_kind("koji");
        assert!(matches!(kind, self_update::ArchiveKind::Plain(None)));
    }

    #[test]
    fn test_is_running_as_service_default() {
        // In a test environment, we should not be running as a service
        // (unless test runner is inside systemd, which is unlikely for unit tests)
        // This test mainly checks that the function doesn't panic.
        let _result = is_running_as_service();
    }

    #[test]
    fn test_update_info_serialization() {
        let info = UpdateInfo {
            current_version: "1.26.2".to_string(),
            latest_version: "1.27.0".to_string(),
            release_notes: "Bug fixes".to_string(),
            published_at: "2026-04-01".to_string(),
            update_available: true,
        };

        let json = serde_json::to_string(&info).unwrap();
        let deserialized: UpdateInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.current_version, "1.26.2");
        assert_eq!(deserialized.latest_version, "1.27.0");
        assert!(deserialized.update_available);
    }

    #[test]
    fn test_check_for_update_sync_invalid_version() {
        let result = check_for_update_sync("not-a-version");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Invalid current version") || err.contains("Failed to fetch"),
            "Unexpected error: {err}"
        );
    }
}
