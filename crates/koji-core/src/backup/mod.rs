//! Backup and restore functionality for Koji.
//!
//! This module provides:
//! - Archive creation (`create_backup`) - creates a .tar.gz of config + DB
//! - Archive extraction (`extract_backup`) - validates and extracts with SHA-256 check
//! - Manifest reading (`extract_manifest`) - reads just the manifest for preview
//! - Database backup (`backup_db`) - creates a clean DB copy using VACUUM INTO

pub mod archive;
pub mod manifest;
pub mod merge;

// Re-export main functions and types
pub use archive::{create_backup, extract_backup, extract_manifest, ExtractResult};
pub use manifest::{BackendEntry, BackupManifest, BackupModelEntry, BACKUP_FORMAT_VERSION};
pub use merge::{merge_config, merge_database, merge_model_cards, DbMergeStats, MergeStats};
