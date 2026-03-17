use crate::profiles::{Profile, SamplingParams};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: General,
    pub backends: HashMap<String, BackendConfig>,
    pub servers: HashMap<String, ServerConfig>,
    pub supervisor: Supervisor,
    #[serde(default)]
    pub custom_profiles: Option<HashMap<String, SamplingParams>>,
    /// The directory this config was loaded from. Used to resolve models_dir
    /// when running as a service (where %APPDATA% differs from the installing user).
    #[serde(skip)]
    pub loaded_from: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    pub path: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// Health check endpoint URL. Overrides backend's health_check_url.
    #[serde(default)]
    pub url: Option<String>,
    /// Polling interval in milliseconds. Overrides supervisor.health_check_interval_ms.
    #[serde(default)]
    pub interval_ms: Option<u64>,
    /// HTTP timeout in milliseconds per health check request (default: 3000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub profile: Option<Profile>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
    /// Custom port for this server (None = backend default)
    #[serde(default)]
    pub port: Option<u16>,
    /// Per-server health check overrides.
    #[serde(default)]
    pub health_check: Option<HealthCheck>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
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

            // Generate default profile TOML files
            let profiles_dir = config_dir.join("profiles.d");
            if !profiles_dir.exists() {
                if let Err(e) = crate::profiles::generate_default_profiles(&profiles_dir) {
                    tracing::warn!("Failed to generate default profiles: {}", e);
                }
            }

            default
        };

        config.loaded_from = Some(config_dir.to_path_buf());
        migrate_model_cards_to_configs_d(&config)?;
        Ok(config)
    }

    pub fn resolve_server(&self, name: &str) -> Result<(&ServerConfig, &BackendConfig)> {
        let server = self
            .servers
            .get(name)
            .with_context(|| format!("Server '{}' not found in config", name))?;

        let backend = self.backends.get(&server.backend).with_context(|| {
            format!(
                "Backend '{}' referenced by server '{}' not found in config",
                server.backend, name
            )
        })?;

        Ok((server, backend))
    }

    /// Resolve the health check URL for a server, taking into account:
    /// 1. Backend's health_check_url if set
    /// 2. Server's custom port if set
    /// 3. Fallback to http://localhost:{port}/health
    pub fn resolve_health_url(&self, server: &ServerConfig) -> Option<String> {
        let backend = self.backends.get(&server.backend)?;
        let backend_url = backend.health_check_url.as_ref()?;

        // If server has a custom port, replace it in the URL
        if let Some(port) = server.port {
            let mut url = url::Url::parse(backend_url).ok()?;
            url.set_port(Some(port)).ok()?;
            return Some(url.to_string());
        }

        // No custom port, use backend's URL as-is
        Some(backend_url.clone())
    }

    /// Resolve the effective health check config for a server.
    /// Merges: server.health_check → backend.health_check_url → supervisor defaults.
    pub fn resolve_health_check(&self, server: &ServerConfig) -> HealthCheck {
        let server_hc = server.health_check.as_ref();

        HealthCheck {
            url: server_hc
                .and_then(|h| h.url.clone())
                .or_else(|| self.resolve_health_url(server)),
            interval_ms: Some(
                server_hc
                    .and_then(|h| h.interval_ms)
                    .unwrap_or(self.supervisor.health_check_interval_ms),
            ),
            timeout_ms: Some(server_hc.and_then(|h| h.timeout_ms).unwrap_or(3000)),
        }
    }

    pub fn build_args(&self, server: &ServerConfig, backend: &BackendConfig) -> Vec<String> {
        let mut args = backend.default_args.clone();
        args.extend(server.args.clone());

        // Append sampling params as CLI flags, filtering out any duplicates
        // that may already be in server.args
        if let Some(sampling) = self.effective_sampling(server) {
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

    /// Resolve effective sampling for a server, including custom profile lookup.
    pub fn effective_sampling(&self, server: &ServerConfig) -> Option<SamplingParams> {
        let base = match &server.profile {
            Some(Profile::Custom { name }) => {
                // Look up custom profile in config, then profiles.d/
                self.custom_profiles
                    .as_ref()
                    .and_then(|m| m.get(name))
                    .cloned()
                    .or_else(|| {
                        self.profiles_dir()
                            .ok()
                            .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                            .and_then(|map| map.get(name).cloned())
                    })
            }
            Some(profile) => {
                // Try profiles.d/ first, fall back to built-in
                let from_disk = self
                    .profiles_dir()
                    .ok()
                    .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                    .and_then(|map| map.get(&profile.to_string()).cloned());
                Some(from_disk.unwrap_or_else(|| profile.params()))
            }
            None => None,
        };

        match (base, &server.sampling) {
            (Some(base), Some(overrides)) => Some(base.merge(overrides)),
            (Some(base), None) => Some(base),
            (None, Some(sampling)) => Some(sampling.clone()),
            (None, None) => None,
        }
    }

    /// Resolve effective sampling with the 3-layer merge chain:
    /// 1. Profile built-in defaults
    /// 2. Model card per-profile sampling overrides
    /// 3. Server-level sampling overrides
    pub fn effective_sampling_with_card(
        &self,
        server: &ServerConfig,
        card: Option<&crate::models::card::ModelCard>,
    ) -> Option<SamplingParams> {
        // Layer 1: Profile base params
        let base = match &server.profile {
            Some(Profile::Custom { name }) => self
                .custom_profiles
                .as_ref()
                .and_then(|m| m.get(name))
                .cloned()
                .or_else(|| {
                    self.profiles_dir()
                        .ok()
                        .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                        .and_then(|map| map.get(name).cloned())
                }),
            Some(profile) => {
                // Try profiles.d/ first, fall back to built-in
                let from_disk = self
                    .profiles_dir()
                    .ok()
                    .and_then(|dir| crate::profiles::load_profiles_d(&dir).ok())
                    .and_then(|map| map.get(&profile.to_string()).cloned());
                Some(from_disk.unwrap_or_else(|| profile.params()))
            }
            None => None,
        };

        // Layer 2: Model card sampling overrides for this profile
        let profile_name = server.profile.as_ref().map(|p| p.to_string());
        let with_model = match (base, card, profile_name) {
            (Some(base), Some(card), Some(ref pname)) => {
                if let Some(model_sampling) = card.sampling_for(pname) {
                    Some(base.merge(model_sampling))
                } else {
                    Some(base)
                }
            }
            (Some(base), _, _) => Some(base),
            (None, Some(card), Some(ref pname)) => card.sampling_for(pname).cloned(),
            (None, _, _) => None,
        };

        // Layer 3: Server-level overrides
        match (with_model, &server.sampling) {
            (Some(base), Some(overrides)) => Some(base.merge(overrides)),
            (Some(base), None) => Some(base),
            (None, Some(sampling)) => Some(sampling.clone()),
            (None, None) => None,
        }
    }

    pub fn service_name(server_name: &str) -> String {
        format!("kronk-{}", server_name)
    }

    /// Resolve the profiles.d directory for sampling presets.
    /// `<base_dir>/profiles.d/`
    pub fn profiles_dir(&self) -> Result<PathBuf> {
        if let Some(ref loaded) = self.loaded_from {
            Ok(loaded.join("profiles.d"))
        } else {
            Ok(Self::base_dir()?.join("profiles.d"))
        }
    }

    /// Resolve the configs.d directory for model cards.
    /// `<base_dir>/configs.d/`
    pub fn configs_dir(&self) -> Result<PathBuf> {
        if let Some(ref loaded) = self.loaded_from {
            Ok(loaded.join("configs.d"))
        } else {
            Ok(Self::base_dir()?.join("configs.d"))
        }
    }

    /// Resolve the models directory path.
    /// Uses `general.models_dir` if set, otherwise defaults to `<base_dir>/models/`.
    /// On Windows: `%APPDATA%\kronk\models\`
    /// On Linux: `~/.config/kronk/models/`
    pub fn models_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.general.models_dir {
            Ok(PathBuf::from(dir))
        } else if let Some(ref loaded) = self.loaded_from {
            Ok(loaded.join("models"))
        } else {
            Ok(Self::base_dir()?.join("models"))
        }
    }

    /// Resolve the logs directory path.
    /// Uses `general.logs_dir` if set, otherwise defaults to `~/.kronk/logs/`.
    pub fn logs_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.general.logs_dir {
            Ok(PathBuf::from(dir))
        } else if let Some(ref loaded) = self.loaded_from {
            Ok(loaded.join("logs"))
        } else {
            let home =
                directories::UserDirs::new().context("Failed to determine home directory")?;
            Ok(home.home_dir().join(".kronk").join("logs"))
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

/// Migrate model cards from the old `models/<company>/<model>/model.toml` layout
/// to the new `configs.d/<company>-<model>.toml` layout.
/// This is a one-time migration: if `configs.d/` already exists, it's a no-op.
pub fn migrate_model_cards_to_configs_d(config: &Config) -> Result<()> {
    let configs_dir = config.configs_dir()?;
    if configs_dir.exists() {
        return Ok(()); // already migrated
    }
    let models_dir = config.models_dir()?;
    if !models_dir.exists() {
        return Ok(());
    }
    let mut migrated = false;
    for company_entry in std::fs::read_dir(&models_dir)? {
        let company_entry = company_entry?;
        if !company_entry.path().is_dir() {
            continue;
        }
        let company = company_entry.file_name().to_string_lossy().to_string();
        for model_entry in std::fs::read_dir(company_entry.path())? {
            let model_entry = model_entry?;
            let old_card = model_entry.path().join("model.toml");
            if old_card.exists() {
                let model_name = model_entry.file_name().to_string_lossy().to_string();
                let new_filename = format!("{}--{}.toml", company, model_name);
                std::fs::create_dir_all(&configs_dir)?;
                std::fs::copy(&old_card, configs_dir.join(&new_filename))?;
                std::fs::remove_file(&old_card)?;
                migrated = true;
            }
        }
    }
    if migrated {
        tracing::info!("Migrated model cards to {}", configs_dir.display());
    }
    Ok(())
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

        let mut servers = HashMap::new();
        servers.insert(
            "default".to_string(),
            ServerConfig {
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
            },
        );

        Config {
            general: General {
                log_level: "info".to_string(),
                models_dir: None,
                logs_dir: None,
            },
            backends,
            servers,
            supervisor: Supervisor {
                restart_policy: "always".to_string(),
                max_restarts: 10,
                restart_delay_ms: 3000,
                health_check_interval_ms: 5000,
            },
            custom_profiles: None,
            loaded_from: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{Profile, SamplingParams};

    #[test]
    fn test_effective_sampling_profile_only() {
        let config = Config::default();
        let server = ServerConfig {
            backend: "test".to_string(),
            args: vec![],
            profile: Some(Profile::Coding),
            sampling: None,
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
        };
        let params = config.effective_sampling(&server).unwrap();
        assert_eq!(params.temperature, Some(0.3));
    }

    #[test]
    fn test_effective_sampling_override() {
        let config = Config::default();
        let server = ServerConfig {
            backend: "test".to_string(),
            args: vec![],
            profile: Some(Profile::Coding),
            sampling: Some(SamplingParams {
                temperature: Some(0.5),
                ..Default::default()
            }),
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
        };
        let params = config.effective_sampling(&server).unwrap();
        assert_eq!(params.temperature, Some(0.5)); // override won
        assert_eq!(params.top_k, Some(50)); // coding preset kept
    }

    #[test]
    fn test_effective_sampling_none() {
        let config = Config::default();
        let server = ServerConfig {
            backend: "test".to_string(),
            args: vec![],
            profile: None,
            sampling: None,
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
        };
        assert!(config.effective_sampling(&server).is_none());
    }

    #[test]
    fn test_build_args_includes_sampling() {
        let config = Config::default();
        let (server, backend) = config.resolve_server("default").unwrap();
        let args = config.build_args(server, backend);
        // Default server has Profile::Coding, so should include --temp
        assert!(args.contains(&"--temp".to_string()));
    }

    #[test]
    fn test_config_toml_roundtrip_with_profile() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let loaded: Config = toml::from_str(&toml_str).unwrap();
        let server = loaded.servers.get("default").unwrap();
        assert_eq!(server.profile, Some(Profile::Coding));
    }

    #[test]
    fn test_server_with_model_fields_roundtrip() {
        let server = ServerConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            profile: Some(Profile::Coding),
            sampling: None,
            model: Some("bartowski/OmniCoder".to_string()),
            quant: Some("Q4_K_M".to_string()),
            port: None,
            health_check: None,
            enabled: true,
        };
        let toml_str = toml::to_string_pretty(&server).unwrap();
        let loaded: ServerConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(loaded.model, Some("bartowski/OmniCoder".to_string()));
        assert_eq!(loaded.quant, Some("Q4_K_M".to_string()));
    }

    #[test]
    fn test_server_without_model_fields_still_works() {
        let toml_str = r#"
backend = "llama_cpp"
args = ["--host", "0.0.0.0"]
"#;
        let server: ServerConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(server.model, None);
        assert_eq!(server.quant, None);
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

        let server = ServerConfig {
            backend: "test".to_string(),
            args: vec![],
            profile: Some(Profile::Coding),
            sampling: Some(SamplingParams {
                top_p: Some(0.85),
                ..Default::default()
            }),
            model: Some("test/model".to_string()),
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
        };

        // 3-layer merge: Profile::Coding (temp=0.3) -> model card (temp=0.2, top_k=40) -> server (top_p=0.85)
        let params = config
            .effective_sampling_with_card(&server, Some(&card))
            .unwrap();
        assert_eq!(params.temperature, Some(0.2)); // model card override won over profile default
        assert_eq!(params.top_k, Some(40)); // model card override
        assert_eq!(params.top_p, Some(0.85)); // server override won over everything
        assert_eq!(params.min_p, Some(0.05)); // from Profile::Coding base (not overridden)
    }

    #[test]
    fn test_effective_sampling_backward_compat() {
        let config = Config::default();
        let server = ServerConfig {
            backend: "test".to_string(),
            args: vec![],
            profile: Some(Profile::Coding),
            sampling: Some(SamplingParams {
                temperature: Some(0.5),
                ..Default::default()
            }),
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
        };
        let params = config.effective_sampling_with_card(&server, None).unwrap();
        assert_eq!(params.temperature, Some(0.5)); // server override
        assert_eq!(params.top_k, Some(50)); // from Profile::Coding
    }

    #[test]
    fn test_health_check_roundtrip() {
        let toml_str = r#"
backend = "llama_cpp"
args = []

[health_check]
url = "http://localhost:9090/health"
interval_ms = 3000
timeout_ms = 5000
"#;
        let server: ServerConfig = toml::from_str(toml_str).unwrap();
        let hc = server.health_check.unwrap();
        assert_eq!(hc.url, Some("http://localhost:9090/health".to_string()));
        assert_eq!(hc.interval_ms, Some(3000));
        assert_eq!(hc.timeout_ms, Some(5000));
    }

    #[test]
    fn test_server_without_health_check_still_works() {
        let toml_str = r#"
backend = "llama_cpp"
args = []
"#;
        let server: ServerConfig = toml::from_str(toml_str).unwrap();
        assert!(server.health_check.is_none());
    }

    #[test]
    fn test_resolve_health_check_defaults() {
        let config = Config::default();
        let server = config.servers.get("default").unwrap();
        let hc = config.resolve_health_check(server);
        assert_eq!(hc.url, Some("http://localhost:8080/health".to_string()));
        assert_eq!(hc.interval_ms, Some(5000)); // from supervisor default
        assert_eq!(hc.timeout_ms, Some(3000));
    }

    #[test]
    fn test_resolve_health_check_server_override() {
        let config = Config::default();
        let mut server = config.servers.get("default").unwrap().clone();
        server.health_check = Some(HealthCheck {
            url: Some("http://localhost:9090/health".to_string()),
            interval_ms: Some(3000),
            timeout_ms: Some(5000),
        });
        let hc = config.resolve_health_check(&server);
        assert_eq!(hc.url, Some("http://localhost:9090/health".to_string()));
        assert_eq!(hc.interval_ms, Some(3000));
        assert_eq!(hc.timeout_ms, Some(5000));
    }
}
