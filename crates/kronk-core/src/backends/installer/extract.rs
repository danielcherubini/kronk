use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

/// Extract an archive (.zip or .tar.gz) to `dest` and return path to the llama-server binary.
///
/// Uses pure-Rust crates for extraction (flate2 + tar for .tar.gz, zip for .zip).
/// No external commands are required -- this works on any platform without tar in PATH.
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

            // Use zip crate's built-in path validation to prevent Zip Slip
            // entry.enclosed_name() returns None if path escapes the destination
            let entry_name = entry
                .enclosed_name()
                .ok_or_else(|| anyhow!("Invalid path in archive: {}", entry.name()))?;

            // Reject symlinks (CVE-2025-29787: symlink-based path traversal)
            if entry.is_symlink() {
                return Err(anyhow!("Symlinks not allowed in archive: {}", entry.name()));
            }

            let outpath = dest.join(&entry_name);

            if entry.is_dir() {
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
pub fn find_backend_binary(dir: &Path) -> Result<PathBuf> {
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
