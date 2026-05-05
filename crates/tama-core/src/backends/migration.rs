use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::Connection;

use crate::backends::get_backend_install_path;
use crate::backends::registry::BackendType;
use crate::gpu::GpuType;

const MIGRATION_MARKER: &str = ".tama-migration-v2-done";

/// Migrate legacy backend installations from flat structure to variant structure.
/// Idempotent: safe to call multiple times, already-migrated records are skipped.
pub fn migrate_legacy_backends(conn: &Connection, backends_dir: &Path) -> anyhow::Result<()> {
    // Check marker file - if migration already completed, skip
    let marker_path = backends_dir.join(MIGRATION_MARKER);
    if marker_path.exists() {
        tracing::debug!("Migration marker found, skipping legacy migration");
        return Ok(());
    }

    // Ensure backends_dir exists
    fs::create_dir_all(backends_dir).context("Failed to create backends_dir")?;

    // Get all backend records from DB
    let all_versions = list_all_backend_records(conn)?;

    if all_versions.is_empty() {
        // No backends to migrate, still write marker
        fs::write(&marker_path, "").context("Failed to write migration marker")?;
        return Ok(());
    }

    let mut migrated_count = 0;

    for record in &all_versions {
        let backend_type = parse_backend_type(&record.backend_type);
        let old_path = PathBuf::from(&record.path);

        // Derive gpu_variant
        let gpu_variant = derive_gpu_variant(&record.gpu_type, &old_path);

        // Compute new path
        let new_path_dir =
            get_backend_install_path(backends_dir, &backend_type, &gpu_variant, &record.version);

        // Compute the new binary path
        let new_binary_path = compute_new_binary_path(&old_path, &new_path_dir, &backend_type);

        // Check if already migrated (path matches new pattern)
        if is_new_pattern_path(&old_path, backends_dir, &backend_type) {
            tracing::debug!(
                "Backend {} {} already in new pattern, skipping",
                record.name,
                record.version
            );
            continue;
        }

        tracing::info!(
            "Migrating backend {} {} to variant '{}' ({} -> {})",
            record.name,
            record.version,
            gpu_variant,
            old_path.display(),
            new_binary_path.display()
        );

        // Try to move files
        if migrate_files(&old_path, &new_path_dir, &new_binary_path, &backend_type)? {
            // Update DB record
            update_backend_path_and_variant(
                conn,
                &record.name,
                &record.version,
                &gpu_variant,
                &new_binary_path.to_string_lossy(),
            )
            .context("Failed to update DB record")?;
            migrated_count += 1;
        }
    }

    // Write marker file
    fs::write(&marker_path, "").context("Failed to write migration marker")?;

    tracing::info!(
        "Legacy migration complete: {} backends migrated, marker written",
        migrated_count
    );
    Ok(())
}

fn list_all_backend_records(conn: &Connection) -> anyhow::Result<Vec<BackendRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, backend_type, version, path, installed_at, \
         COALESCE(gpu_type, ''), COALESCE(gpu_variant, 'cpu'), \
         COALESCE(source, ''), is_active \
         FROM backend_installations",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(BackendRecord {
            _id: row.get(0)?,
            name: row.get(1)?,
            backend_type: row.get(2)?,
            version: row.get(3)?,
            path: row.get(4)?,
            _installed_at: row.get(5)?,
            gpu_type: row.get(6)?,
            _gpu_variant: row.get(7)?,
            _source: row.get(8)?,
            _is_active: row.get::<_, i64>(9)? != 0,
        })
    })?;

    let records: Vec<BackendRecord> = rows
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(records)
}

fn update_backend_path_and_variant(
    conn: &Connection,
    name: &str,
    version: &str,
    gpu_variant: &str,
    new_path: &str,
) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE backend_installations \
         SET path = ?, gpu_variant = ? \
         WHERE name = ? AND version = ?",
        (new_path, gpu_variant, name, version),
    )?;
    Ok(())
}

fn derive_gpu_variant(gpu_type_str: &str, binary_path: &Path) -> String {
    if let Some(gpu_type) = parse_gpu_type(gpu_type_str) {
        return gpu_type.variant_folder().to_string();
    }

    // Heuristic: check binary name for hints
    if let Some(stem) = binary_path.file_stem().and_then(|s| s.to_str()) {
        let lower = stem.to_lowercase();
        if lower.contains("cuda") {
            tracing::warn!("Heuristic: detected 'cuda' in binary name {}", stem);
            return "cuda".to_string();
        }
        if lower.contains("rocm") || lower.contains("hip") {
            tracing::warn!("Heuristic: detected 'rocm' in binary name {}", stem);
            return "rocm".to_string();
        }
        if lower.contains("vulkan") {
            tracing::warn!("Heuristic: detected 'vulkan' in binary name {}", stem);
            return "vulkan".to_string();
        }
    }

    tracing::warn!(
        "No gpu_type info for {}, defaulting to 'cpu'",
        binary_path.display()
    );
    "cpu".to_string()
}

fn is_new_pattern_path(path: &Path, backends_dir: &Path, backend_type: &BackendType) -> bool {
    // New pattern: backends/<type>/<variant>/<version>/binary
    // Check if path starts with backends_dir/<type>/ and has at least 2 more components after type
    if let Ok(relative) = path.strip_prefix(backends_dir) {
        let components: Vec<_> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        if components.first() == Some(&backend_type.to_string()) && components.len() >= 3 {
            return true;
        }
    }
    false
}

fn compute_new_binary_path(
    old_path: &Path,
    new_path_dir: &Path,
    backend_type: &BackendType,
) -> PathBuf {
    let binary_name = old_path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| "llama-server".into());

    match backend_type {
        BackendType::TtsKokoro => {
            // TTS is a directory, not a binary
            old_path
                .parent()
                .map(|p| {
                    let dir_name = p.file_name().unwrap_or_default();
                    new_path_dir.join(dir_name)
                })
                .unwrap_or_else(|| new_path_dir.to_path_buf())
        }
        _ => new_path_dir.join(binary_name),
    }
}

fn migrate_files(
    old_path: &Path,
    new_path_dir: &Path,
    new_binary_path: &Path,
    backend_type: &BackendType,
) -> anyhow::Result<bool> {
    match backend_type {
        BackendType::TtsKokoro => {
            // TTS: the old_path is the base_dir itself
            // Move the entire directory
            if !old_path.exists() {
                // Check if new path exists (files moved but DB not updated)
                if new_binary_path.exists() || new_path_dir.exists() {
                    tracing::info!("TTS dir already at new location, just updating DB");
                    return Ok(true);
                }
                tracing::warn!(
                    "TTS path {} not found, skipping migration",
                    old_path.display()
                );
                return Ok(false);
            }

            fs::create_dir_all(new_path_dir).context("Failed to create new path dir")?;

            // Move the kokoro directory
            let src_dir = old_path;
            let dst_dir = new_binary_path;

            if let Some(parent) = dst_dir.parent() {
                fs::create_dir_all(parent).context("Failed to create parent dir")?;
            }

            fs::rename(src_dir, dst_dir).context("Failed to rename TTS directory")?;
            Ok(true)
        }
        _ => {
            // Binary backend: move the binary file
            if !old_path.exists() {
                // Check if new path exists (files moved but DB not updated)
                if new_binary_path.exists() {
                    tracing::info!(
                        "Binary already at new location {}, just updating DB",
                        new_binary_path.display()
                    );
                    return Ok(true);
                }
                tracing::warn!(
                    "Binary {} not found, skipping migration",
                    old_path.display()
                );
                return Ok(false);
            }

            fs::create_dir_all(new_path_dir).context("Failed to create new path dir")?;
            fs::rename(old_path, new_binary_path).with_context(|| {
                format!(
                    "Failed to rename {} to {}",
                    old_path.display(),
                    new_binary_path.display()
                )
            })?;

            // Also move any .so/.dylib/.dll files in the same directory
            if let Some(old_dir) = old_path.parent() {
                if old_dir.is_dir() {
                    if let Ok(entries) = fs::read_dir(old_dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                                if matches!(ext, "so" | "dylib" | "dll") || ext.starts_with("so.") {
                                    let new_lib_path = new_path_dir.join(path.file_name().unwrap());
                                    fs::rename(&path, &new_lib_path).with_context(|| {
                                        format!("Failed to move library {}", path.display())
                                    })?;
                                }
                            }
                        }
                    }
                }
            }

            Ok(true)
        }
    }
}

fn parse_backend_type(type_str: &str) -> BackendType {
    match type_str {
        "llama_cpp" => BackendType::LlamaCpp,
        "ik_llama" => BackendType::IkLlama,
        "tts_kokoro" => BackendType::TtsKokoro,
        _ => BackendType::Custom,
    }
}

fn parse_gpu_type(type_str: &str) -> Option<GpuType> {
    if type_str.is_empty() {
        return None;
    }
    // Try to deserialize from JSON string
    serde_json::from_str(type_str).ok()
}

#[derive(Debug)]
struct BackendRecord {
    _id: i64,
    name: String,
    backend_type: String,
    version: String,
    path: String,
    _installed_at: i64,
    gpu_type: String,
    _gpu_variant: String,
    _source: String,
    _is_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_is_new_pattern_path() {
        let backends_dir = Path::new("/home/user/.local/share/tama/backends");
        let backend_type = BackendType::LlamaCpp;

        // New pattern paths should return true
        assert!(is_new_pattern_path(
            Path::new("/home/user/.local/share/tama/backends/llama_cpp/cpu/b8407/llama-server"),
            backends_dir,
            &backend_type,
        ));
        assert!(is_new_pattern_path(
            Path::new("/home/user/.local/share/tama/backends/llama_cpp/cuda/b8407/llama-server"),
            backends_dir,
            &backend_type,
        ));

        // Old pattern paths should return false
        assert!(!is_new_pattern_path(
            Path::new("/home/user/.local/share/tama/backends/llama_cpp/llama-server"),
            backends_dir,
            &backend_type,
        ));
    }

    #[test]
    fn test_derive_gpu_variant_from_gpu_type() {
        // GpuType uses default serde repr: {"CpuOnly":null}, {"Vulkan":null}, etc.
        assert_eq!(
            derive_gpu_variant("{\"CpuOnly\":null}", Path::new("/tmp/test")),
            "cpu"
        );
        assert_eq!(
            derive_gpu_variant("{\"Vulkan\":null}", Path::new("/tmp/test")),
            "vulkan"
        );
        assert_eq!(
            derive_gpu_variant("{\"Metal\":null}", Path::new("/tmp/test")),
            "metal"
        );
        assert_eq!(
            derive_gpu_variant("{\"Cuda\":{\"version\":\"12.4\"}}", Path::new("/tmp/test")),
            "cuda"
        );
        assert_eq!(
            derive_gpu_variant("{\"RocM\":{\"version\":\"6.0\"}}", Path::new("/tmp/test")),
            "rocm"
        );
    }

    #[test]
    fn test_derive_gpu_variant_heuristic() {
        assert_eq!(
            derive_gpu_variant("", Path::new("/tmp/llama-server-cuda")),
            "cuda"
        );
        assert_eq!(
            derive_gpu_variant("", Path::new("/tmp/llama-server-rocm")),
            "rocm"
        );
        assert_eq!(
            derive_gpu_variant("", Path::new("/tmp/llama-server-vulkan")),
            "vulkan"
        );
        assert_eq!(
            derive_gpu_variant("", Path::new("/tmp/llama-server")),
            "cpu"
        );
    }

    #[test]
    fn test_parse_backend_type() {
        assert_eq!(parse_backend_type("llama_cpp"), BackendType::LlamaCpp);
        assert_eq!(parse_backend_type("ik_llama"), BackendType::IkLlama);
        assert_eq!(parse_backend_type("tts_kokoro"), BackendType::TtsKokoro);
        assert_eq!(parse_backend_type("unknown"), BackendType::Custom);
    }
}
