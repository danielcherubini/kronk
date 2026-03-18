use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::config::Config;
use crate::gpu::GpuType;

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

pub struct BackendRegistry {
    path: PathBuf,
    data: RegistryData,
}

impl BackendRegistry {
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_base_dir(path, None)
    }

    pub fn load_with_base_dir(path: &Path, base_dir: Option<&Path>) -> Result<Self> {
        // Validate path is within safe directory (prevent symlink attacks)
        // Canonicalize parent directory first (works even if file doesn't exist yet)
        let canonical_path = if path.exists() {
            std::fs::canonicalize(path)
                .with_context(|| format!("Failed to canonicalize registry path {:?}", path))?
        } else {
            // For new files, canonicalize the parent directory
            if let Some(parent) = path.parent() {
                std::fs::canonicalize(parent)
                    .with_context(|| {
                        format!("Failed to canonicalize parent directory {:?}", parent)
                    })?
                    .join(path.file_name().unwrap_or_default())
            } else {
                return Err(anyhow!("Registry path {:?} has no parent directory", path));
            }
        };

        // Get the canonical base directory if provided
        let base_dir = if let Some(base) = base_dir {
            std::fs::canonicalize(base).with_context(|| "Failed to canonicalize base directory")?
        } else {
            std::fs::canonicalize(Config::base_dir()?)
                .with_context(|| "Failed to canonicalize base directory")?
        };

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
            data,
        })
    }

    pub fn save(&self) -> Result<()> {
        // Validate path is within safe directory
        // Canonicalize parent directory first (works even if file doesn't exist yet)
        let canonical_path = if self.path.exists() {
            std::fs::canonicalize(&self.path)
                .with_context(|| format!("Failed to canonicalize registry path {:?}", self.path))?
        } else {
            // For new files, canonicalize the parent directory
            if let Some(parent) = self.path.parent() {
                std::fs::canonicalize(parent)
                    .with_context(|| {
                        format!("Failed to canonicalize parent directory {:?}", parent)
                    })?
                    .join(self.path.file_name().unwrap_or_default())
            } else {
                return Err(anyhow!(
                    "Registry path {:?} has no parent directory",
                    self.path
                ));
            }
        };

        let base_dir = std::fs::canonicalize(Config::base_dir()?)
            .with_context(|| "Failed to canonicalize base directory")?;

        if !canonical_path.starts_with(&base_dir) {
            return Err(anyhow!(
                "Registry path {:?} is outside base directory {:?}",
                self.path,
                base_dir
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

    pub fn add(&mut self, backend: BackendInfo) -> Result<()> {
        self.data.backends.insert(backend.name.clone(), backend);
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        self.data.backends.remove(name);
        self.save()
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
        if let Some(info) = self.data.backends.get_mut(name) {
            info.version = new_version;
            info.path = new_binary_path;
            info.installed_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            info.source = new_source;
        } else {
            return Err(anyhow!("Backend '{}' not found", name));
        }
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_add_and_list() {
        let base_dir = Config::base_dir().unwrap();
        let registry_path = base_dir.join("test_registry.toml");
        std::fs::create_dir_all(registry_path.parent().unwrap()).unwrap();
        let mut registry = BackendRegistry::load_unchecked(&registry_path).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        registry
            .add(BackendInfo {
                name: "llama_cpp".to_string(),
                backend_type: BackendType::LlamaCpp,
                version: "b8407".to_string(),
                path: "/path/to/llama-server".into(),
                installed_at: now,
                gpu_type: None,
                source: Some(BackendSource::Prebuilt {
                    version: "b8407".to_string(),
                }),
            })
            .unwrap();

        registry.save_unchecked().unwrap();

        let backends = registry.list();
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].name, "llama_cpp");
    }

    #[test]
    fn test_registry_remove() {
        let base_dir = Config::base_dir().unwrap();
        let registry_path = base_dir.join("test_registry_remove.toml");
        // Create parent directory so canonicalize can work
        std::fs::create_dir_all(registry_path.parent().unwrap()).unwrap();
        let mut registry = BackendRegistry::load_unchecked(&registry_path).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        registry
            .add(BackendInfo {
                name: "llama_cpp".to_string(),
                backend_type: BackendType::LlamaCpp,
                version: "b8407".to_string(),
                path: "/path/to/llama-server".into(),
                installed_at: now,
                gpu_type: None,
                source: None,
            })
            .unwrap();

        registry.save_unchecked().unwrap();

        registry.remove("llama_cpp").unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_registry_roundtrip_serialization() {
        let base_dir = Config::base_dir().unwrap();
        let registry_path = base_dir.join("test_registry_roundtrip.toml");
        // Create parent directory so canonicalize can work
        std::fs::create_dir_all(registry_path.parent().unwrap()).unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Write
        {
            let mut registry = BackendRegistry::load_unchecked(&registry_path).unwrap();
            registry
                .add(BackendInfo {
                    name: "test".to_string(),
                    backend_type: BackendType::LlamaCpp,
                    version: "b1234".to_string(),
                    path: "/tmp/test".into(),
                    installed_at: now,
                    gpu_type: Some(crate::gpu::GpuType::Cuda {
                        version: "12.4".to_string(),
                    }),
                    source: None,
                })
                .unwrap();
            registry.save_unchecked().unwrap();
        }

        // Read back
        let registry = BackendRegistry::load_unchecked(&registry_path).unwrap();
        let backend = registry.get("test").unwrap();
        assert_eq!(backend.version, "b1234");
    }
}
