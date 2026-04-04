use super::migrate::{migrate_profiles_to_model_cards, rename_legacy_directories};
use super::types::{BackendConfig, Config, General, ModelConfig, ProxyConfig, Supervisor};
use crate::profiles::Profile;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

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

        // Rename legacy .d directories if they exist
        let _ = rename_legacy_directories(config_dir);

        let config_path = config_dir.join("config.toml");

        let mut config = if config_path.exists() {
            let contents =
                fs::read_to_string(&config_path).context("Failed to read config file")?;
            let c: Config = toml::from_str(&contents).context("Failed to parse config file")?;
            c
        } else {
            let default = Self::default();
            let toml_str =
                toml::to_string_pretty(&default).context("Failed to serialize default config")?;
            fs::write(&config_path, &toml_str).context("Failed to write default config")?;
            tracing::info!("Created default config at {}", config_path.display());
            default
        };

        config.loaded_from = Some(config_dir.to_path_buf());
        migrate_profiles_to_model_cards(&mut config)?;
        Ok(config)
    }

    /// Save config to the location it was loaded from, or the default location.
    pub fn save(&self) -> Result<()> {
        if let Some(ref loaded) = self.loaded_from {
            return self.save_to(loaded);
        }
        let config_path = Self::config_path()?;
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, &toml_str).context("Failed to write config")?;
        Ok(())
    }

    /// Save config to a specific directory path.
    /// Used by tests and Windows service which need to save to non-standard locations.
    pub fn save_to(&self, config_dir: &std::path::Path) -> Result<()> {
        let config_path = config_dir.join("config.toml");
        fs::create_dir_all(config_dir).context("Failed to create config directory")?;
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, &toml_str).context("Failed to write config")?;
        Ok(())
    }

    /// Resolve the logs directory path.
    /// Uses `general.logs_dir` if set, otherwise defaults to `<base_dir>/logs/`.
    /// On Windows this is `%APPDATA%\kronk\logs\`, on Linux `~/.config/kronk/logs/`.
    pub fn logs_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.general.logs_dir {
            Ok(PathBuf::from(dir))
        } else if let Some(ref loaded) = self.loaded_from {
            Ok(loaded.join("logs"))
        } else {
            Ok(Self::base_dir()?.join("logs"))
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
}

impl Default for Config {
    fn default() -> Self {
        let mut backends = HashMap::new();
        backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: Some("http://localhost:8080/health".to_string()),
                version: None,
            },
        );
        backends.insert(
            "ik_llama".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: Some("http://localhost:8080/health".to_string()),
                version: None,
            },
        );

        let mut models = HashMap::new();
        models.insert(
            "default".to_string(),
            ModelConfig {
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
                profile: Some(Profile::Coding),
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
            },
        );

        // Built-in sampling templates for all profiles
        let mut sampling_templates = HashMap::new();
        for (_, _, profile) in Profile::all() {
            let params = match profile {
                Profile::Coding => crate::profiles::SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(50),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.1),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Chat => crate::profiles::SamplingParams {
                    temperature: Some(0.7),
                    top_p: Some(0.95),
                    top_k: Some(40),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Analysis => crate::profiles::SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(20),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Creative => crate::profiles::SamplingParams {
                    temperature: Some(0.9),
                    top_p: Some(0.95),
                    top_k: Some(50),
                    min_p: Some(0.02),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
            };
            sampling_templates.insert(profile.to_string(), params);
        }

        Config {
            general: General {
                log_level: "info".to_string(),
                models_dir: None,
                logs_dir: None,
            },
            backends,
            models,
            supervisor: Supervisor::default(),
            proxy: ProxyConfig::default(),
            sampling_templates,
            loaded_from: None,
        }
    }
}
