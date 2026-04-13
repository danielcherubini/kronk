//! Archive creation and extraction for Koji backup/restore.
//!
//! **SHA-256 Contract:** The SHA-256 in the manifest covers all archive entries
//! **EXCEPT** `manifest.json` itself. This avoids the chicken-and-egg problem.
//!
//! On creation (`create_backup`):
//! 1. Stream config files + DB through a hasher
//! 2. Compute SHA-256
//! 3. Write tar.gz with manifest.json first (containing the hash)
//! 4. Then write all other entries
//!
//! On extraction (`extract_backup`):
//! 1. Read all entries except manifest.json into a hasher
//! 2. Compare computed SHA-256 against manifest.sha256
//! 3. If mismatch, delete extracted files and error

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder};

use crate::backup::manifest::{BackendEntry, BackupManifest, BackupModelEntry};

/// Result of extracting a backup archive.
#[derive(Debug)]
pub struct ExtractResult {
    /// Parsed manifest from the archive
    pub manifest: BackupManifest,
    /// Path to extracted config.toml
    pub config_path: PathBuf,
    /// Path to extracted koji.db
    pub db_path: PathBuf,
    /// Paths to extracted model card TOML files
    pub card_paths: Vec<PathBuf>,
}

/// Create a backup archive containing config.toml, configs/*.toml, and koji.db.
///
/// **SHA-256 Contract:** The returned manifest's `sha256` field covers all archive
/// entries **EXCEPT** `manifest.json` itself.
pub fn create_backup(config_dir: &Path, output_path: &Path) -> Result<BackupManifest> {
    if !config_dir.exists() {
        anyhow::bail!("Config directory does not exist: {}", config_dir.display());
    }

    // Step 1: Compute SHA-256 by streaming files through hasher
    let mut hasher = Sha256::new();

    let mut add_file_to_hasher = |path: &Path, description: &str| -> Result<()> {
        let mut file = File::open(path)
            .with_context(|| format!("Failed to open {}: {}", description, path.display()))?;
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)
            .with_context(|| format!("Failed to read {}: {}", description, path.display()))?;
        hasher.update(&contents);
        Ok(())
    };

    // Add config.toml
    let config_path = config_dir.join("config.toml");
    if !config_path.exists() {
        anyhow::bail!("config.toml not found at {}", config_path.display());
    }
    add_file_to_hasher(&config_path, "config.toml")?;

    // Add all model card TOML files (sorted for consistent hashing)
    let configs_dir = config_dir.join("configs");
    if configs_dir.exists() {
        let mut entries: Vec<_> = fs::read_dir(&configs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            add_file_to_hasher(&path, &format!("config card: {}", path.display()))?;
        }
    }

    // Add koji.db using VACUUM INTO for a clean copy
    let db_path = config_dir.join("koji.db");
    if !db_path.exists() {
        anyhow::bail!("koji.db not found at {}", db_path.display());
    }
    let temp_db =
        tempfile::NamedTempFile::new().context("Failed to create temp file for DB backup")?;
    let temp_db_path = temp_db.path().to_path_buf();
    crate::db::backup_db(config_dir, &temp_db_path).context("Failed to backup database")?;
    add_file_to_hasher(&temp_db_path, "koji.db")?;

    // Compute SHA-256
    let hash = hasher.finalize();
    let sha256_hex = format!("{:x}", hash);

    // Step 2: Build BackupManifest by querying the DB
    let conn = rusqlite::Connection::open(&db_path)
        .context("Failed to open database for manifest generation")?;
    let manifest = build_manifest_from_db(&conn, &sha256_hex)?;

    // Step 3: Create the tar.gz archive
    create_tar_gz_archive(config_dir, output_path, &manifest, &temp_db_path)
        .context("Failed to create archive")?;

    Ok(manifest)
}

fn build_manifest_from_db(conn: &rusqlite::Connection, sha256: &str) -> Result<BackupManifest> {
    use std::collections::HashMap;

    let mut stmt_pulls = conn
        .prepare("SELECT repo_id, commit_sha FROM model_pulls")
        .context("Failed to prepare model_pulls query")?;
    let pulls: Vec<(String, String)> = stmt_pulls
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;

    let mut stmt_files = conn
        .prepare("SELECT repo_id, quant, size_bytes FROM model_files")
        .context("Failed to prepare model_files query")?;
    let files: Vec<(String, Option<String>, i64)> = stmt_files
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;

    let mut files_by_repo: HashMap<String, Vec<(Option<String>, i64)>> = HashMap::new();
    for (repo_id, quant, size_bytes) in files {
        files_by_repo
            .entry(repo_id)
            .or_default()
            .push((quant, size_bytes));
    }

    let models_vec: Vec<BackupModelEntry> = pulls
        .into_iter()
        .map(|(repo_id, _)| {
            let repo_files = files_by_repo.get(&repo_id).cloned().unwrap_or_default();
            let quants: Vec<String> = repo_files.iter().filter_map(|(q, _)| q.clone()).collect();
            let total_size: i64 = repo_files.iter().map(|(_, size)| *size).sum();
            BackupModelEntry {
                repo_id,
                quants,
                total_size_bytes: total_size,
            }
        })
        .collect();

    let mut stmt_backends = conn.prepare(
        "SELECT name, version, backend_type, source FROM backend_installations WHERE is_active = 1"
    ).context("Failed to prepare backend_installations query")?;
    let backends: Vec<BackendEntry> = stmt_backends
        .query_map([], |row| {
            Ok(BackendEntry {
                name: row.get(0)?,
                version: row.get(1)?,
                backend_type: row.get(2)?,
                source: row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    let koji_version = env!("CARGO_PKG_VERSION").to_string();

    let mut manifest = BackupManifest::new(&koji_version);
    manifest.sha256 = sha256.to_string();
    manifest.models = models_vec;
    manifest.backends = backends;

    Ok(manifest)
}

fn create_tar_gz_archive(
    config_dir: &Path,
    output_path: &Path,
    manifest: &BackupManifest,
    temp_db_path: &Path,
) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;
    }

    let manifest_json =
        serde_json::to_string_pretty(manifest).context("Failed to serialize manifest to JSON")?;

    let file = File::create(output_path)
        .with_context(|| format!("Failed to create archive file: {}", output_path.display()))?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = Builder::new(encoder);

    let manifest_name = "manifest.json";
    let manifest_data = manifest_json.as_bytes();
    let mut header = tar::Header::new_gnu();
    header
        .set_path(manifest_name)
        .context("Failed to set manifest.json path")?;
    header.set_size(manifest_data.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();
    tar.append(&header, manifest_json.as_bytes())
        .context("Failed to append manifest.json to archive")?;

    let config_path = config_dir.join("config.toml");
    add_file_to_archive(&mut tar, &config_path, "config.toml")
        .context("Failed to add config.toml to archive")?;

    let configs_dir = config_dir.join("configs");
    if configs_dir.exists() {
        let mut entries: Vec<_> = fs::read_dir(&configs_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            add_file_to_archive(&mut tar, &path, &format!("configs/{}", name))
                .context("Failed to add config card to archive")?;
        }
    }

    add_file_to_archive(&mut tar, temp_db_path, "koji.db")
        .context("Failed to add koji.db to archive")?;

    tar.into_inner()?
        .finish()
        .context("Failed to finalize archive")?;

    Ok(())
}

fn add_file_to_archive(
    tar: &mut Builder<flate2::write::GzEncoder<File>>,
    path: &Path,
    name: &str,
) -> Result<()> {
    let mut file =
        File::open(path).with_context(|| format!("Failed to open {}: {}", name, path.display()))?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .with_context(|| format!("Failed to read {}: {}", name, path.display()))?;

    let mut header = tar::Header::new_gnu();
    header
        .set_path(name)
        .with_context(|| format!("Failed to set path for {}: {}", name, path.display()))?;
    header.set_size(contents.len() as u64);
    header.set_mode(0o644);
    header.set_mtime(chrono::Utc::now().timestamp() as u64);
    header.set_cksum();
    tar.append(&header, contents.as_slice())
        .with_context(|| format!("Failed to append {} to archive", name))?;

    Ok(())
}

pub fn extract_manifest(archive_path: &Path) -> Result<BackupManifest> {
    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.to_string_lossy() == "manifest.json" {
            let mut contents = String::new();
            entry
                .read_to_string(&mut contents)
                .context("Failed to read manifest.json from archive")?;
            return serde_json::from_str(&contents)
                .context("Failed to parse manifest.json from archive");
        }
    }

    anyhow::bail!("manifest.json not found in archive")
}

pub fn extract_backup(archive_path: &Path, target_dir: &Path) -> Result<ExtractResult> {
    let manifest =
        extract_manifest(archive_path).context("Failed to extract or parse manifest.json")?;

    fs::create_dir_all(target_dir).with_context(|| {
        format!(
            "Failed to create target directory: {}",
            target_dir.display()
        )
    })?;

    let file = File::open(archive_path)
        .with_context(|| format!("Failed to open archive: {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    let mut hasher = Sha256::new();
    let mut extracted_config: Option<PathBuf> = None;
    let mut extracted_db: Option<PathBuf> = None;
    let mut extracted_cards: Vec<PathBuf> = Vec::new();

    for entry_result in archive.entries()? {
        let mut entry = entry_result?;
        let entry_name_str = entry.path()?;
        let entry_name_owned = entry_name_str.to_string_lossy().to_string();
        let needs_hashing = entry_name_owned != "manifest.json";

        if needs_hashing {
            let mut contents = Vec::new();
            entry
                .read_to_end(&mut contents)
                .with_context(|| format!("Failed to read entry: {}", entry_name_owned))?;
            hasher.update(&contents);

            let dest_path = target_dir.join(entry_name_owned.trim_start_matches("/"));

            // Validate path to prevent traversal attacks
            let canonical_target = target_dir.canonicalize().with_context(|| {
                format!(
                    "Failed to canonicalize target directory: {}",
                    target_dir.display()
                )
            })?;

            // Check for path traversal before creating directories
            if dest_path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                anyhow::bail!(
                    "Path traversal detected in archive entry: {}",
                    entry_name_owned
                );
            }

            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directory: {}", parent.display())
                })?;
            }

            // Double-check the resolved path is within target_dir
            if let Ok(canonical_dest) = dest_path.canonicalize() {
                if !canonical_dest.starts_with(&canonical_target) {
                    anyhow::bail!(
                        "Extracted path escapes target directory: {}",
                        dest_path.display()
                    );
                }
            } else {
                // Path doesn't exist yet, check relative path
                let relative = dest_path.strip_prefix(&canonical_target).map_err(|_| {
                    anyhow::anyhow!("Path escapes target directory: {}", dest_path.display())
                })?;
                if relative
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    anyhow::bail!(
                        "Extracted path escapes target directory: {}",
                        dest_path.display()
                    );
                }
            }

            let mut file = File::create(&dest_path)
                .with_context(|| format!("Failed to create file: {}", dest_path.display()))?;
            file.write_all(&contents)
                .with_context(|| format!("Failed to write file: {}", dest_path.display()))?;

            match entry_name_owned.as_str() {
                "config.toml" => extracted_config = Some(dest_path),
                "koji.db" => extracted_db = Some(dest_path),
                name if name.starts_with("configs/") => extracted_cards.push(dest_path),
                _ => {}
            }
        } else {
            let mut _contents = Vec::new();
            entry
                .read_to_end(&mut _contents)
                .with_context(|| format!("Failed to read manifest.json: {}", entry_name_owned))?;
        }
    }

    let computed_hash = hasher.finalize();
    let computed_hex = format!("{:x}", computed_hash);
    if computed_hex != manifest.sha256 {
        fs::remove_dir_all(target_dir).ok();
        anyhow::bail!(
            "SHA-256 integrity check failed! Expected: {}, Computed: {}",
            manifest.sha256,
            computed_hex
        );
    }

    Ok(ExtractResult {
        manifest,
        config_path: extracted_config.context("config.toml not found in archive")?,
        db_path: extracted_db.context("koji.db not found in archive")?,
        card_paths: extracted_cards,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::BACKUP_FORMAT_VERSION;
    use rusqlite::Connection;
    use std::fs;

    #[test]
    fn test_create_and_extract_backup_roundtrip() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = temp_dir.path().join("config");
        let output_path = temp_dir.path().join("backup.tar.gz");
        let extract_dir = temp_dir.path().join("extracted");

        fs::create_dir_all(config_dir.join("configs")).expect("create dirs");

        let config_content = r#"
[general]
log_level = "info"

[backends.llama_cpp]
health_check_url = "http://localhost:8080/health"

[models.test]
backend = "llama_cpp"
enabled = true
"#;
        fs::write(config_dir.join("config.toml"), config_content).expect("write config");

        let db_path = config_dir.join("koji.db");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(
            "CREATE TABLE model_pulls (id INTEGER PRIMARY KEY AUTOINCREMENT, repo_id TEXT NOT NULL, commit_sha TEXT NOT NULL, pulled_at TEXT NOT NULL, UNIQUE(repo_id));
             CREATE TABLE model_files (id INTEGER PRIMARY KEY AUTOINCREMENT, repo_id TEXT NOT NULL, filename TEXT NOT NULL, quant TEXT, lfs_oid TEXT, size_bytes INTEGER NOT NULL, downloaded_at TEXT NOT NULL, last_verified_at TEXT, verified_ok INTEGER, verify_error TEXT, UNIQUE(repo_id, filename));
             CREATE TABLE backend_installations (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, backend_type TEXT NOT NULL, version TEXT NOT NULL, path TEXT NOT NULL, installed_at INTEGER NOT NULL, gpu_type TEXT, source TEXT, is_active INTEGER NOT NULL DEFAULT 0, UNIQUE(name, version));"
        ).expect("create tables");
        conn.execute(
            "INSERT INTO model_pulls (repo_id, commit_sha, pulled_at) VALUES ('test/repo', 'abc123', '2024-01-01T00:00:00Z');",
            [],
        ).expect("insert model pull");
        conn.execute(
            "INSERT INTO model_files (repo_id, filename, quant, size_bytes, downloaded_at) VALUES ('test/repo', 'model.gguf', 'Q4_K_M', 1000, '2024-01-01T00:00:00Z');",
            [],
        ).expect("insert model file");
        conn.execute(
            "INSERT INTO backend_installations (name, backend_type, version, path, installed_at, gpu_type, source, is_active) VALUES ('llama_cpp', 'llama_cpp', 'v1.0', '/tmp/llama', 1234567890, NULL, 'prebuilt', 1);",
            [],
        ).expect("insert backend");

        let result = create_backup(&config_dir, &output_path);
        assert!(
            result.is_ok(),
            "create_backup should succeed: {:?}",
            result.err()
        );

        let extract_result = extract_backup(&output_path, &extract_dir);
        assert!(
            extract_result.is_ok(),
            "extract_backup should succeed: {:?}",
            extract_result.err()
        );

        let extracted = extract_result.unwrap();
        assert!(extracted.config_path.exists(), "config.toml should exist");
        assert!(extracted.db_path.exists(), "koji.db should exist");

        let original_config = fs::read_to_string(config_dir.join("config.toml")).unwrap();
        let extracted_config = fs::read_to_string(&extracted.config_path).unwrap();
        assert_eq!(original_config, extracted_config);
    }

    #[test]
    fn test_backup_db() {
        use crate::db::backup_db;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = temp_dir.path().join("config");
        let dest = temp_dir.path().join("backup.db");

        let db_path = config_dir.join("koji.db");
        fs::create_dir_all(&config_dir).expect("create dirs");

        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch("CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);")
            .expect("create table");
        conn.execute("INSERT INTO test (value) VALUES ('hello');", [])
            .expect("insert data");

        let result = backup_db(&config_dir, &dest);
        assert!(
            result.is_ok(),
            "backup_db should succeed: {:?}",
            result.err()
        );

        let backup_conn = Connection::open(&dest).expect("open backup");
        let count: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM test", [], |row: &rusqlite::Row| {
                row.get(0)
            })
            .expect("count rows");
        assert_eq!(count, 1, "backup should have 1 row");

        let value: String = backup_conn
            .query_row(
                "SELECT value FROM test WHERE id = 1",
                [],
                |row: &rusqlite::Row| row.get(0),
            )
            .expect("get value");
        assert_eq!(value, "hello");
    }
}
