use crate::use_cases::{SamplingParams, UseCase};
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
    #[serde(default)]
    pub custom_use_cases: Option<HashMap<String, SamplingParams>>,
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
    #[serde(default)]
    pub use_case: Option<UseCase>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
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

        // Append sampling params as CLI flags
        if let Some(sampling) = self.effective_sampling(profile) {
            args.extend(sampling.to_args());
        }

        args
    }

    /// Resolve effective sampling for a profile, including custom use case lookup.
    pub fn effective_sampling(&self, profile: &ProfileConfig) -> Option<SamplingParams> {
        let base = match &profile.use_case {
            Some(UseCase::Custom { name }) => {
                // Look up custom use case in config
                self.custom_use_cases
                    .as_ref()
                    .and_then(|m| m.get(name))
                    .cloned()
            }
            Some(uc) => Some(uc.params()),
            None => None,
        };

        match (base, &profile.sampling) {
            (Some(base), Some(overrides)) => Some(base.merge(overrides)),
            (Some(base), None) => Some(base),
            (None, Some(sampling)) => Some(sampling.clone()),
            (None, None) => None,
        }
    }

    pub fn service_name(profile: &str) -> String {
        format!("kronk-{}", profile)
    }

    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
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
                    "--host",
                    "0.0.0.0",
                    "-m",
                    "path/to/model.gguf",
                    "-ngl",
                    "999",
                    "-fa",
                    "1",
                    "-c",
                    "8192",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
                use_case: Some(UseCase::Coding),
                sampling: None,
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
            custom_use_cases: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_cases::{SamplingParams, UseCase};

    #[test]
    fn test_effective_sampling_use_case_only() {
        let config = Config::default();
        let profile = ProfileConfig {
            backend: "test".to_string(),
            args: vec![],
            use_case: Some(UseCase::Coding),
            sampling: None,
        };
        let params = config.effective_sampling(&profile).unwrap();
        assert_eq!(params.temperature, Some(0.3));
    }

    #[test]
    fn test_effective_sampling_override() {
        let config = Config::default();
        let profile = ProfileConfig {
            backend: "test".to_string(),
            args: vec![],
            use_case: Some(UseCase::Coding),
            sampling: Some(SamplingParams {
                temperature: Some(0.5),
                ..Default::default()
            }),
        };
        let params = config.effective_sampling(&profile).unwrap();
        assert_eq!(params.temperature, Some(0.5)); // override won
        assert_eq!(params.top_k, Some(50)); // coding preset kept
    }

    #[test]
    fn test_effective_sampling_none() {
        let config = Config::default();
        let profile = ProfileConfig {
            backend: "test".to_string(),
            args: vec![],
            use_case: None,
            sampling: None,
        };
        assert!(config.effective_sampling(&profile).is_none());
    }

    #[test]
    fn test_build_args_includes_sampling() {
        let config = Config::default();
        let (profile, backend) = config.resolve_profile("default").unwrap();
        let args = config.build_args(profile, backend);
        // Default profile has UseCase::Coding, so should include --temp
        assert!(args.contains(&"--temp".to_string()));
    }

    #[test]
    fn test_config_toml_roundtrip_with_use_case() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let loaded: Config = toml::from_str(&toml_str).unwrap();
        let profile = loaded.profiles.get("default").unwrap();
        assert_eq!(profile.use_case, Some(UseCase::Coding));
    }
}
