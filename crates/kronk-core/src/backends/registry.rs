use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
///
/// `installed_at` is a unix epoch timestamp (i64) because `SystemTime`
/// cannot be serialized by the `toml` crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfo {
    pub name: String,
    pub backend_type: BackendType,
    pub version: String,
    pub path: PathBuf,
    pub installed_at: i64,
    #[serde(default)]
    pub gpu_type: Option<GpuType>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryData {
    #[serde(default)]
    backends: HashMap<String, BackendInfo>,
}

pub struct BackendRegistry {
    path: PathBuf,
    data: RegistryData,
}

impl BackendRegistry {
    pub fn load(path: &Path) -> Result<Self> {
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
        new_path: PathBuf,
    ) -> Result<()> {
        if let Some(backend) = self.data.backends.get_mut(name) {
            backend.version = new_version;
            backend.path = new_path;
            self.save()?;
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
        let tmp = TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.toml");
        let mut registry = BackendRegistry::load(&registry_path).unwrap();

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
            })
            .unwrap();

        let backends = registry.list();
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].name, "llama_cpp");
    }

    #[test]
    fn test_registry_remove() {
        let tmp = TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.toml");
        let mut registry = BackendRegistry::load(&registry_path).unwrap();

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
            })
            .unwrap();

        registry.remove("llama_cpp").unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_registry_roundtrip_serialization() {
        let tmp = TempDir::new().unwrap();
        let registry_path = tmp.path().join("registry.toml");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Write
        {
            let mut registry = BackendRegistry::load(&registry_path).unwrap();
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
                })
                .unwrap();
        }

        // Read back
        let registry = BackendRegistry::load(&registry_path).unwrap();
        let backend = registry.get("test").unwrap();
        assert_eq!(backend.version, "b1234");
    }
}
