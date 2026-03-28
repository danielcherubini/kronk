use anyhow::Context;
use std::path::{Path, PathBuf};

use super::BackendRegistry;

impl BackendRegistry {
    /// Backup the registry to a specified directory
    pub fn backup(&self, backup_dir: &Path) -> anyhow::Result<PathBuf> {
        let backup_path = backup_dir.join("registry.toml.bak");

        // Ensure backup directory exists
        std::fs::create_dir_all(backup_dir)
            .with_context(|| format!("Failed to create backup directory {:?}", backup_dir))?;

        // Read current registry content
        let content = std::fs::read_to_string(self.path())
            .with_context(|| format!("Failed to read registry at {:?}", self.path()))?;

        // Write to backup location
        std::fs::write(&backup_path, content)
            .with_context(|| format!("Failed to write backup to {:?}", backup_path))?;

        Ok(backup_path)
    }

    /// Restore from a backup file
    pub fn restore(&mut self, backup_path: &Path) -> anyhow::Result<()> {
        let content = std::fs::read_to_string(backup_path)
            .with_context(|| format!("Failed to read backup file {:?}", backup_path))?;

        let data: super::RegistryData =
            toml::from_str(&content).with_context(|| "Failed to parse backup registry")?;

        self.data_mut().backends = data.backends;
        self.save()
    }
}
