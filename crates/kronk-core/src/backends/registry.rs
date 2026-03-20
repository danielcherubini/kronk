use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::config::Config;
use crate::gpu::GpuType;

impl BackendRegistry {
    /// Canonicalize a path, handling both existing and non-existing files
    fn canonicalize_path(path: &Path) -> Result<PathBuf> {
        if path.exists() {
            std::fs::canonicalize(path)
                .with_context(|| format!("Failed to canonicalize registry path {:?}", path))
        } else {
            // For new files, canonicalize the parent directory
            if let Some(parent) = path.parent() {
                std::fs::canonicalize(parent)
                    .with_context(|| {
                        format!("Failed to canonicalize parent directory {:?}", parent)
                    })
                    .map(|p| p.join(path.file_name().unwrap_or_default()))
            } else {
                Err(anyhow!("Registry path {:?} has no parent directory", path))
            }
        }
    }

    /// Canonicalize base directory
    fn canonicalize_base_dir(base_dir: Option<&Path>) -> Result<PathBuf> {
        base_dir
            .map(|b| {
                std::fs::canonicalize(b).with_context(|| "Failed to canonicalize base directory")
            })
            .unwrap_or_else(|| {
                std::fs::canonicalize(Config::base_dir()?)
                    .with_context(|| "Failed to canonicalize base directory")
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackendType {
    LlamaCpp,
    IkLlama,
    Custom,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::LlamaCpp => write!(f, "llama_cpp"),
            BackendType::IkLlama => write!(f, "ik_llama"),
            BackendType::Custom => write!(f, "custom"),
        }
    }
}

impl FromStr for BackendType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "llama_cpp" | "llamacpp" => Ok(BackendType::LlamaCpp),
            "ik_llama" | "ik-llama" | "ikllama" => Ok(BackendType::IkLlama),
            "custom" => Ok(BackendType::Custom),
            _ => Err(format!(
                "Unknown backend type '{}'. Supported: llama_cpp, ik_llama, custom",
                s
            )),
        }
    }
}

/// Metadata for an installed backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_type: Option<GpuType>,
    #[serde(default)]
    pub source: Option<BackendSource>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryData {
    #[serde(default)]
    backends: HashMap<String, BackendInfo>,
}

/// Source of a backend installation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", content = "content")]
pub enum BackendSource {
    Prebuilt { version: String },
    SourceCode { version: String, git_url: String },
}

#[derive(Debug)]
pub struct BackendRegistry {
    path: PathBuf,
    base_dir: PathBuf,
    data: RegistryData,
}

impl BackendRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_base_dir(path, None)
    }

    pub fn load_with_base_dir(path: &Path, base_dir: Option<&Path>) -> Result<Self> {
        // Validate path is within safe directory (prevent symlink attacks)
        let canonical_path = Self::canonicalize_path(path)?;
        let base_dir = Self::canonicalize_base_dir(base_dir)?;

        // Ensure the path is within the base directory
        if !canonical_path.starts_with(&base_dir) {
            return Err(anyhow!(
                "Registry path {:?} is outside base directory {:?}",
                path,
                base_dir
            ));
        }

        let data = if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read registry at {:?}", path))?;
            toml::from_str(&content).with_context(|| "Failed to parse registry")?
        } else {
            RegistryData::default()
        };

        Ok(Self {
            path: path.to_path_buf(),
            base_dir,
            data,
        })
    }

    pub fn save(&self) -> Result<()> {
        let canonical_path = Self::canonicalize_path(&self.path)?;

        if !canonical_path.starts_with(&self.base_dir) {
            return Err(anyhow!(
                "Registry path {:?} is outside base directory {:?}",
                self.path,
                self.base_dir
            ));
        }

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&self.data)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    /// Load registry without path validation (for tests)
    #[cfg(test)]
    pub fn load_unchecked(path: &Path) -> Result<Self> {
        let data = if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read registry at {:?}", path))?;
            toml::from_str(&content).with_context(|| "Failed to parse registry")?
        } else {
            RegistryData::default()
        };

        Ok(Self {
            path: path.to_path_buf(),
            base_dir: path.parent().unwrap_or(Path::new("/")).to_path_buf(),
            data,
        })
    }

    /// Save without path validation (for tests)
    #[cfg(test)]
    pub fn save_unchecked(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(&self.data)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    /// Load registry without path validation (for tests) with base_dir
    #[cfg(test)]
    pub fn load_unchecked_with_base_dir(path: &Path, base_dir: &Path) -> Result<Self> {
        let data = if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read registry at {:?}", path))?;
            toml::from_str(&content).with_context(|| "Failed to parse registry")?
        } else {
            RegistryData::default()
        };

        Ok(Self {
            path: path.to_path_buf(),
            base_dir: base_dir.to_path_buf(),
            data,
        })
    }

    pub fn add(&mut self, backend: BackendInfo) -> Result<()> {
        // Use transactional pattern: modify on scratch copy, only replace after save succeeds
        let original_backends = std::mem::take(&mut self.data.backends);
        let mut new_backends = original_backends.clone();
        new_backends.insert(backend.name.clone(), backend);
        self.data.backends = new_backends;
        if let Err(e) = self.save() {
            // Rollback: restore original state
            self.data.backends = original_backends;
            return Err(e);
        }
        Ok(())
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        // Use transactional pattern: modify on scratch copy, only replace after save succeeds
        let original_backends = std::mem::take(&mut self.data.backends);
        let mut new_backends = original_backends.clone();
        new_backends.remove(name);
        self.data.backends = new_backends;
        if let Err(e) = self.save() {
            // Rollback: restore original state
            self.data.backends = original_backends;
            return Err(e);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn add_unchecked(&mut self, backend: BackendInfo) {
        let original_backends = std::mem::take(&mut self.data.backends);
        let mut new_backends = original_backends.clone();
        new_backends.insert(backend.name.clone(), backend);
        self.data.backends = new_backends;
        if let Err(_) = self.save_unchecked() {
            self.data.backends = original_backends;
        }
    }

    #[cfg(test)]
    pub fn remove_unchecked(&mut self, name: &str) {
        let original_backends = std::mem::take(&mut self.data.backends);
        let mut new_backends = original_backends.clone();
        new_backends.remove(name);
        self.data.backends = new_backends;
        if let Err(_) = self.save_unchecked() {
            self.data.backends = original_backends;
        }
    }

    pub fn get(&self, name: &str) -> Option<&BackendInfo> {
        self.data.backends.get(name)
    }

    pub fn list(&self) -> Vec<&BackendInfo> {
        self.data.backends.values().collect()
    }

    pub fn update_version(
        &mut self,
        name: &str,
        new_version: String,
        new_binary_path: PathBuf,
        new_source: Option<BackendSource>,
    ) -> Result<()> {
        // Validate new_binary_path is within managed backends directory
        let new_binary_path_canonical = Self::canonicalize_path(&new_binary_path)?;
        if !new_binary_path_canonical.starts_with(&self.base_dir) {
            return Err(anyhow!(
                "Backend binary path {:?} is outside managed directory {:?}",
                new_binary_path,
                self.base_dir
            ));
        }

        // Use transactional pattern: modify on scratch copy, only replace after save succeeds
        let original_backends = std::mem::take(&mut self.data.backends);
        let mut new_backends = original_backends.clone();
        if let Some(info) = new_backends.get_mut(name) {
            info.version = new_version;
            info.path = new_binary_path;
            info.installed_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            info.source = new_source;
        } else {
            // Rollback: restore original state
            self.data.backends = original_backends;
            return Err(anyhow!("Backend '{}' not found", name));
        }
        self.data.backends = new_backends;
        if let Err(e) = self.save() {
            // Rollback: restore original state
            self.data.backends = original_backends;
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_registry_add_and_list() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path().join("test_registry.toml");
        let base_dir = temp_dir.path().to_path_buf();
        let mut registry =
            BackendRegistry::load_unchecked_with_base_dir(&registry_path, &base_dir).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        registry.add_unchecked(BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b8407".to_string(),
            path: "/path/to/llama-server".into(),
            installed_at: now,
            gpu_type: None,
            source: Some(BackendSource::Prebuilt {
                version: "b8407".to_string(),
            }),
        });

        let backends = registry.list();
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].name, "llama_cpp");
    }

    #[test]
    fn test_registry_remove() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path().join("test_registry_remove.toml");
        let base_dir = temp_dir.path().to_path_buf();
        let mut registry =
            BackendRegistry::load_unchecked_with_base_dir(&registry_path, &base_dir).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        registry.add_unchecked(BackendInfo {
            name: "llama_cpp".to_string(),
            backend_type: BackendType::LlamaCpp,
            version: "b8407".to_string(),
            path: "/path/to/llama-server".into(),
            installed_at: now,
            gpu_type: None,
            source: None,
        });

        registry.remove_unchecked("llama_cpp");
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_registry_roundtrip_serialization() {
        let temp_dir = TempDir::new().unwrap();
        let registry_path = temp_dir.path().join("test_registry_roundtrip.toml");
        let base_dir = temp_dir.path().to_path_buf();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Write
        {
            let mut registry =
                BackendRegistry::load_unchecked_with_base_dir(&registry_path, &base_dir).unwrap();
            registry.add_unchecked(BackendInfo {
                name: "test".to_string(),
                backend_type: BackendType::LlamaCpp,
                version: "b1234".to_string(),
                path: "/tmp/test".into(),
                installed_at: now,
                gpu_type: Some(crate::gpu::GpuType::Cuda {
                    version: "12.4".to_string(),
                }),
                source: None,
            });
        }

        // Read back
        let registry = BackendRegistry::load_unchecked(&registry_path).unwrap();
        let backend = registry.get("test").unwrap();
        assert_eq!(backend.version, "b1234");
    }
}
