use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: General,
    pub backends: HashMap<String, BackendConfig>,
    pub profiles: HashMap<String, ProfileConfig>,
    pub supervisor: Supervisor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub path: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Supervisor {
    pub restart_policy: String,
    pub max_restarts: u32,
    pub restart_delay_ms: u64,
    pub health_check_interval_ms: u64,
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let proj = directories::ProjectDirs::from("", "", "kronk")
            .context("Failed to determine config directory")?;
        Ok(proj.config_dir().to_path_buf())
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let config_dir = Self::config_dir()?;
        fs::create_dir_all(&config_dir).context("Failed to create config directory")?;

        let config_path = config_dir.join("config.toml");

        if config_path.exists() {
            let contents =
                fs::read_to_string(&config_path).context("Failed to read config file")?;
            toml::from_str(&contents).context("Failed to parse config file")
        } else {
            let default = Self::default();
            let toml_str =
                toml::to_string_pretty(&default).context("Failed to serialize default config")?;
            fs::write(&config_path, &toml_str).context("Failed to write default config")?;
            tracing::info!("Created default config at {}", config_path.display());
            Ok(default)
        }
    }

    pub fn resolve_profile(&self, name: &str) -> Result<(&ProfileConfig, &BackendConfig)> {
        let profile = self
            .profiles
            .get(name)
            .with_context(|| format!("Profile '{}' not found in config", name))?;

        let backend = self.backends.get(&profile.backend).with_context(|| {
            format!(
                "Backend '{}' referenced by profile '{}' not found in config",
                profile.backend, name
            )
        })?;

        Ok((profile, backend))
    }

    pub fn build_args(&self, profile: &ProfileConfig, backend: &BackendConfig) -> Vec<String> {
        let mut args = backend.default_args.clone();
        args.extend(profile.args.clone());
        args
    }

    pub fn service_name(profile: &str) -> String {
        format!("kronk-{}", profile)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        let toml_str =
            toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, &toml_str).context("Failed to write config")?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut backends = HashMap::new();
        backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: r"C:\llama.cpp\llama-server.exe".to_string(),
                default_args: vec![],
                health_check_url: Some("http://localhost:8080/health".to_string()),
            },
        );

        let mut profiles = HashMap::new();
        profiles.insert(
            "default".to_string(),
            ProfileConfig {
                backend: "llama_cpp".to_string(),
                args: vec![
                    "--host", "0.0.0.0",
                    "-m", "path/to/model.gguf",
                    "-ngl", "999",
                    "-fa", "1",
                    "-c", "8192",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
        );

        Config {
            general: General {
                log_level: "info".to_string(),
            },
            backends,
            profiles,
            supervisor: Supervisor {
                restart_policy: "always".to_string(),
                max_restarts: 10,
                restart_delay_ms: 3000,
                health_check_interval_ms: 5000,
            },
        }
    }
}
