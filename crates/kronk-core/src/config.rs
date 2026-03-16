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
    #[serde(default)]
    pub models_dir: Option<String>,
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
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Supervisor {
    pub restart_policy: String,
    pub max_restarts: u32,
    pub restart_delay_ms: u64,
    pub health_check_interval_ms: u64,
}

impl Config {
    /// Base directory for all kronk data.
    /// Windows: `%APPDATA%\kronk`
    /// Linux: `~/.config/kronk`
    pub fn base_dir() -> Result<PathBuf> {
        let proj = directories::ProjectDirs::from("", "", "kronk")
            .context("Failed to determine config directory")?;
        // config_dir() on Windows = %APPDATA%\kronk\config, we want the parent
        // On Linux config_dir() = ~/.config/kronk which is already the base
        #[cfg(target_os = "windows")]
        {
            Ok(proj
                .config_dir()
                .parent()
                .unwrap_or(proj.config_dir())
                .to_path_buf())
        }
        #[cfg(not(target_os = "windows"))]
        {
            Ok(proj.config_dir().to_path_buf())
        }
    }

    pub fn config_dir() -> Result<PathBuf> {
        Self::base_dir()
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let config_dir = Self::config_dir()?;
        Self::load_from(&config_dir)
    }

    /// Load config from an explicit directory path.
    /// Used by the Windows service which runs as SYSTEM and needs
    /// the installing user's config directory.
    pub fn load_from(config_dir: &std::path::Path) -> Result<Self> {
        fs::create_dir_all(config_dir).context("Failed to create config directory")?;

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

        // Append sampling params as CLI flags, filtering out any duplicates
        // that may already be in profile.args
        if let Some(sampling) = self.effective_sampling(profile) {
            let sampling_args = sampling.to_args();
            let sampling_flags: std::collections::HashSet<&str> = sampling_args
                .iter()
                .filter(|a| a.starts_with("--"))
                .map(|a| a.as_str())
                .collect();

            // Remove existing sampling flags and their values from args
            if !sampling_flags.is_empty() {
                let mut filtered = Vec::with_capacity(args.len());
                let mut skip_next = false;
                for arg in &args {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if sampling_flags.contains(arg.as_str()) {
                        skip_next = true; // skip the flag and its following value
                        continue;
                    }
                    filtered.push(arg.clone());
                }
                args = filtered;
            }

            args.extend(sampling_args);
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

    /// Resolve effective sampling with the 3-layer merge chain:
    /// 1. UseCase built-in defaults
    /// 2. Model card per-use-case sampling overrides
    /// 3. Profile-level sampling overrides
    pub fn effective_sampling_with_card(
        &self,
        profile: &ProfileConfig,
        card: Option<&crate::models::card::ModelCard>,
    ) -> Option<SamplingParams> {
        // Layer 1: Use case base params
        let base = match &profile.use_case {
            Some(UseCase::Custom { name }) => self
                .custom_use_cases
                .as_ref()
                .and_then(|m| m.get(name))
                .cloned(),
            Some(uc) => Some(uc.params()),
            None => None,
        };

        // Layer 2: Model card sampling overrides for this use case
        let use_case_name = profile.use_case.as_ref().map(|uc| uc.to_string());
        let with_model = match (base, card, use_case_name) {
            (Some(base), Some(card), Some(ref uc_name)) => {
                if let Some(model_sampling) = card.sampling_for(uc_name) {
                    Some(base.merge(model_sampling))
                } else {
                    Some(base)
                }
            }
            (Some(base), _, _) => Some(base),
            (None, Some(card), Some(ref uc_name)) => card.sampling_for(uc_name).cloned(),
            (None, _, _) => None,
        };

        // Layer 3: Profile-level overrides
        match (with_model, &profile.sampling) {
            (Some(base), Some(overrides)) => Some(base.merge(overrides)),
            (Some(base), None) => Some(base),
            (None, Some(sampling)) => Some(sampling.clone()),
            (None, None) => None,
        }
    }

    pub fn service_name(profile: &str) -> String {
        format!("kronk-{}", profile)
    }

    /// Resolve the models directory path.
    /// Uses `general.models_dir` if set, otherwise defaults to `<base_dir>/models/`.
    /// On Windows: `%APPDATA%\kronk\models\`
    /// On Linux: `~/.config/kronk/models/`
    pub fn models_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.general.models_dir {
            Ok(PathBuf::from(dir))
        } else {
            Ok(Self::base_dir()?.join("models"))
        }
    }

    pub fn with_models_dir(&self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            general: General {
                models_dir: Some(dir.to_string_lossy().to_string()),
                ..self.general.clone()
            },
            ..self.clone()
        }
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
                model: None,
                quant: None,
            },
        );

        Config {
            general: General {
                log_level: "info".to_string(),
                models_dir: None,
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
            model: None,
            quant: None,
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
            model: None,
            quant: None,
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
            model: None,
            quant: None,
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

    #[test]
    fn test_profile_with_model_fields_roundtrip() {
        let profile = ProfileConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            use_case: Some(UseCase::Coding),
            sampling: None,
            model: Some("bartowski/OmniCoder".to_string()),
            quant: Some("Q4_K_M".to_string()),
        };
        let toml_str = toml::to_string_pretty(&profile).unwrap();
        let loaded: ProfileConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(loaded.model, Some("bartowski/OmniCoder".to_string()));
        assert_eq!(loaded.quant, Some("Q4_K_M".to_string()));
    }

    #[test]
    fn test_profile_without_model_fields_still_works() {
        let toml_str = r#"
backend = "llama_cpp"
args = ["--host", "0.0.0.0"]
"#;
        let profile: ProfileConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(profile.model, None);
        assert_eq!(profile.quant, None);
    }

    #[test]
    fn test_effective_sampling_with_model_card() {
        use crate::models::card::{ModelCard, ModelMeta};

        let config = Config::default();

        let mut sampling = HashMap::new();
        sampling.insert(
            "coding".to_string(),
            SamplingParams {
                temperature: Some(0.2),
                top_k: Some(40),
                ..Default::default()
            },
        );

        let card = ModelCard {
            model: ModelMeta {
                name: "TestModel".to_string(),
                source: "test/model".to_string(),
                default_context_length: None,
                default_gpu_layers: None,
            },
            sampling,
            quants: HashMap::new(),
        };

        let profile = ProfileConfig {
            backend: "test".to_string(),
            args: vec![],
            use_case: Some(UseCase::Coding),
            sampling: Some(SamplingParams {
                top_p: Some(0.85),
                ..Default::default()
            }),
            model: Some("test/model".to_string()),
            quant: None,
        };

        // 3-layer merge: UseCase::Coding (temp=0.3) -> model card (temp=0.2, top_k=40) -> profile (top_p=0.85)
        let params = config
            .effective_sampling_with_card(&profile, Some(&card))
            .unwrap();
        assert_eq!(params.temperature, Some(0.2)); // model card override won over use case default
        assert_eq!(params.top_k, Some(40)); // model card override
        assert_eq!(params.top_p, Some(0.85)); // profile override won over everything
        assert_eq!(params.min_p, Some(0.05)); // from UseCase::Coding base (not overridden)
    }

    #[test]
    fn test_effective_sampling_backward_compat() {
        let config = Config::default();
        let profile = ProfileConfig {
            backend: "test".to_string(),
            args: vec![],
            use_case: Some(UseCase::Coding),
            sampling: Some(SamplingParams {
                temperature: Some(0.5),
                ..Default::default()
            }),
            model: None,
            quant: None,
        };
        let params = config.effective_sampling_with_card(&profile, None).unwrap();
        assert_eq!(params.temperature, Some(0.5)); // profile override
        assert_eq!(params.top_k, Some(50)); // from UseCase::Coding
    }
}
