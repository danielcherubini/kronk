//! Backup manifest types for Koji backup/restore.

use serde::{Deserialize, Serialize};

/// Backup format version. Increment when breaking changes are made.
pub const BACKUP_FORMAT_VERSION: u32 = 1;

/// Entry representing a model in the backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupModelEntry {
    /// HuggingFace repo ID (e.g., "bartowski/OmniCoder-8B-GGUF")
    pub repo_id: String,
    /// List of quantization names available for this model
    pub quants: Vec<String>,
    /// Total size of all GGUF files for this model in bytes
    pub total_size_bytes: i64,
}

/// Entry representing a backend in the backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendEntry {
    /// Backend key (e.g., "llama_cpp", "ik_llama")
    pub name: String,
    /// Version string (e.g., "b8407", "main@abc1234")
    pub version: String,
    /// Backend type as string (e.g., "llama_cpp", "ik_llama", "custom")
    pub backend_type: String,
    /// Source as string (e.g., "prebuilt", "source")
    pub source: String,
}

/// Full backup manifest describing what's in the archive.
///
/// **SHA-256 contract:** The `sha256` field covers all archive entries
/// **EXCEPT** `manifest.json` itself. This avoids the chicken-and-egg problem
/// of needing the hash to create the manifest that contains the hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Backup format version (currently 1)
    pub version: u32,
    /// ISO 8601 timestamp when backup was created
    pub created_at: String,
    /// Koji version that created this backup
    pub koji_version: String,
    /// SHA-256 hash of all archive entries EXCEPT manifest.json
    pub sha256: String,
    /// List of models included in the backup
    pub models: Vec<BackupModelEntry>,
    /// List of backends included in the backup
    pub backends: Vec<BackendEntry>,
}

impl BackupManifest {
    /// Create a new manifest with the current timestamp and version.
    pub fn new(koji_version: &str) -> Self {
        Self {
            version: BACKUP_FORMAT_VERSION,
            created_at: chrono::Utc::now().to_rfc3339(),
            koji_version: koji_version.to_string(),
            sha256: String::new(), // Will be filled in after computing
            models: Vec::new(),
            backends: Vec::new(),
        }
    }

    /// Validate that this manifest's version matches the expected backup format version.
    ///
    /// Returns `Ok(())` if the version is compatible, or an error describing
    /// the mismatch. Call this after deserializing a manifest from an archive
    /// to reject archives created by incompatible Koji versions.
    pub fn validate_version(&self) -> anyhow::Result<()> {
        if self.version != BACKUP_FORMAT_VERSION {
            anyhow::bail!(
                "Incompatible backup format version: expected {}, got {}",
                BACKUP_FORMAT_VERSION,
                self.version
            );
        }
        Ok(())
    }
}
