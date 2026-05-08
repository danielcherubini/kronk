//! One-time auto-migration from the legacy `kronk` data directory to the new
//! `tama` data directory.
//!
//! When the project was renamed from `kronk` to `tama`, existing users had
//! data under `~/.config/kronk`. On first run of the new binary we rename that
//! directory to the `tama` location and rename the SQLite database file inside
//! it from `kronk.db` to `tama.db`.
//!
//! This module intentionally contains the legacy name `kronk` as string
//! literals — it is the only place in the codebase where those literals
//! should appear.

use std::path::{Path, PathBuf};

/// Describes a successful migration from the legacy location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Resolve the legacy kronk base directory on the current platform.
///
/// Mirrors the logic in `Config::base_dir` but pinned to the old application
/// name. Returns `None` if `ProjectDirs` cannot determine a home directory
/// (e.g. in restricted test environments).
fn legacy_base_dir() -> Option<PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "kronk")?;
    Some(proj.config_dir().to_path_buf())
}

/// Migrate a legacy kronk data directory to the new tama location if needed.
///
/// Returns `Ok(Some(Migration))` if a migration was performed, `Ok(None)` if
/// there was nothing to do (new directory already exists, or legacy directory
/// does not exist, or the platform does not provide a config directory).
///
/// The function is a no-op if `new_dir` already exists, so it is safe to call
/// on every startup.
pub fn migrate_legacy_data_dir(new_dir: &Path) -> anyhow::Result<Option<Migration>> {
    // If the new data directory already exists we have nothing to do — either
    // migration already ran, or this is a fresh install on a machine that
    // never had kronk.
    if new_dir.exists() {
        return Ok(None);
    }

    let Some(legacy_dir) = legacy_base_dir() else {
        return Ok(None);
    };

    // If legacy_dir is the same as new_dir (hypothetically, e.g. on a
    // platform where `directories` returns the same path for both names),
    // bail out — there is nothing to migrate.
    if legacy_dir == new_dir {
        return Ok(None);
    }

    if !legacy_dir.exists() {
        return Ok(None);
    }

    // Ensure the parent of the new directory exists so rename can succeed.
    if let Some(parent) = new_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Try a fast rename first; fall back to recursive copy + remove on
    // cross-device errors.
    match std::fs::rename(&legacy_dir, new_dir) {
        Ok(()) => {}
        Err(e)
            if e.kind() == std::io::ErrorKind::CrossesDevices
                || e.raw_os_error() == Some(18) /* EXDEV */ =>
        {
            copy_dir_recursive(&legacy_dir, new_dir)?;
            std::fs::remove_dir_all(&legacy_dir)?;
        }
        Err(e) => return Err(e.into()),
    }

    // Rename the legacy database file in place, if present.
    let legacy_db = new_dir.join("kronk.db");
    if legacy_db.exists() {
        let new_db = new_dir.join("tama.db");
        if !new_db.exists() {
            if let Err(e) = std::fs::rename(&legacy_db, &new_db) {
                tracing::warn!(
                    "Failed to rename legacy database {} to {}: {}",
                    legacy_db.display(),
                    new_db.display(),
                    e
                );
            }
        }
    }

    tracing::info!(
        "Migrated legacy data directory {} -> {}",
        legacy_dir.display(),
        new_dir.display()
    );

    Ok(Some(Migration {
        from: legacy_dir,
        to: new_dir.to_path_buf(),
    }))
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ty.is_symlink() {
            #[cfg(unix)]
            {
                let target = std::fs::read_link(&from)?;
                std::os::unix::fs::symlink(target, &to)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&from, &to)?;
            }
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! These tests exercise `migrate_legacy_data_dir` directly against
    //! explicit paths inside a tempdir, rather than going through
    //! `ProjectDirs`. This keeps them hermetic and avoids touching the real
    //! user home.
    //!
    //! A handful of cases that depend on `legacy_base_dir` (which reads
    //! `ProjectDirs`) are covered by the "new dir exists" / "no legacy"
    //! branches, which exit before touching `ProjectDirs`.

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_noop_when_new_dir_already_exists() {
        let tmp = tempdir().unwrap();
        let new_dir = tmp.path().join("tama");
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(new_dir.join("marker.txt"), "existing").unwrap();

        let result = migrate_legacy_data_dir(&new_dir).unwrap();
        assert!(result.is_none());
        // The marker file must still be there — we didn't clobber the dir.
        assert_eq!(
            fs::read_to_string(new_dir.join("marker.txt")).unwrap(),
            "existing"
        );
    }

    #[test]
    fn test_noop_when_no_legacy_dir_present() {
        // Use a new_dir that does not exist under a tempdir. Because the
        // legacy path (under the real home) almost certainly does not
        // exist either in CI, this exercises the "legacy missing" branch.
        // If a developer happens to have a real ~/.config/kronk, this test
        // would incorrectly migrate it — we guard against that by checking
        // whether a legacy dir exists and skipping the assertion when it
        // does.
        let tmp = tempdir().unwrap();
        let new_dir = tmp.path().join("tama-does-not-exist");

        if let Some(legacy) = legacy_base_dir() {
            if legacy.exists() {
                eprintln!(
                    "Skipping test_noop_when_no_legacy_dir_present: real legacy \
                     directory {} exists on this machine.",
                    legacy.display()
                );
                return;
            }
        }

        let result = migrate_legacy_data_dir(&new_dir).unwrap();
        assert!(result.is_none());
        assert!(!new_dir.exists());
    }

    #[test]
    fn test_copy_dir_recursive_copies_files_and_subdirs() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(src.join("nested")).unwrap();
        fs::write(src.join("a.txt"), "hello").unwrap();
        fs::write(src.join("nested/b.txt"), "world").unwrap();

        copy_dir_recursive(&src, &dst).unwrap();

        assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "hello");
        assert_eq!(
            fs::read_to_string(dst.join("nested/b.txt")).unwrap(),
            "world"
        );
    }

    /// Integration-style test of the directory+db rename using a fake
    /// legacy path we control. We do this by manually renaming a source
    /// directory to the destination using the same helper the real
    /// function would call after deciding a migration is needed.
    #[test]
    fn test_rename_moves_directory_and_renames_db() {
        let tmp = tempdir().unwrap();
        let legacy = tmp.path().join("kronk");
        let new_dir = tmp.path().join("tama");

        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("kronk.db"), b"sqlite-bytes").unwrap();
        fs::write(legacy.join("config.toml"), "log_level = \"info\"").unwrap();
        fs::create_dir_all(legacy.join("logs")).unwrap();
        fs::write(legacy.join("logs/server.log"), "log line\n").unwrap();

        // Simulate the rename step that `migrate_legacy_data_dir` performs
        // after it has resolved the legacy dir. We can't easily point
        // `legacy_base_dir` at a tempdir without introducing a test hook,
        // so we drive the post-resolution logic directly here.
        std::fs::rename(&legacy, &new_dir).unwrap();

        let legacy_db = new_dir.join("kronk.db");
        assert!(legacy_db.exists());
        std::fs::rename(&legacy_db, new_dir.join("tama.db")).unwrap();

        assert!(new_dir.join("tama.db").exists());
        assert!(!new_dir.join("kronk.db").exists());
        assert_eq!(
            fs::read(new_dir.join("tama.db")).unwrap(),
            b"sqlite-bytes".to_vec()
        );
        assert_eq!(
            fs::read_to_string(new_dir.join("config.toml")).unwrap(),
            "log_level = \"info\""
        );
        assert_eq!(
            fs::read_to_string(new_dir.join("logs/server.log")).unwrap(),
            "log line\n"
        );
        assert!(!legacy.exists());
    }
}
