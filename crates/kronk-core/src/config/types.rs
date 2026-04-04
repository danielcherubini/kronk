use crate::profiles::{Profile, SamplingParams};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: General,
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub supervisor: Supervisor,
    #[serde(default)]
    pub sampling_templates: HashMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// The directory this config was loaded from. Used to resolve models_dir
    /// when running as a service (where %APPDATA% differs from the installing user).
    #[serde(skip)]
    pub loaded_from: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_proxy_enabled")]
    pub enabled: bool,
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default = "default_proxy_timeout")]
    pub idle_timeout_secs: u64,
    #[serde(default = "default_startup_timeout")]
    pub startup_timeout_secs: u64,
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    #[serde(default = "default_circuit_breaker_cooldown")]
    pub circuit_breaker_cooldown_seconds: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: default_proxy_enabled(),
            host: default_proxy_host(),
            port: default_proxy_port(),
            idle_timeout_secs: default_proxy_timeout(),
            startup_timeout_secs: default_startup_timeout(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            circuit_breaker_cooldown_seconds: default_circuit_breaker_cooldown(),
        }
    }
}

impl Config {
    /// Get the configs directory for model cards.
    /// Returns `<loaded_from>/configs/`.
    pub fn configs_dir(&self) -> anyhow::Result<std::path::PathBuf> {
        self.loaded_from
            .as_deref()
            .map(|p| p.join("configs"))
            .ok_or_else(|| anyhow::anyhow!("Config has no loaded_from path"))
    }

    /// Get the models directory for this config.
    /// Uses `general.models_dir` if set, otherwise `<loaded_from>/models/`.
    pub fn models_dir(&self) -> anyhow::Result<std::path::PathBuf> {
        if let Some(models_dir) = &self.general.models_dir {
            return Ok(std::path::PathBuf::from(models_dir));
        }
        self.loaded_from
            .as_deref()
            .map(|p| p.join("models"))
            .ok_or_else(|| anyhow::anyhow!("Config has no loaded_from path"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
    /// Optional version pin. When set, resolve_backend_path looks up this
    /// specific version in the DB instead of the currently-active version.
    #[serde(default)]
    pub version: Option<String>,
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
pub struct ModelConfig {
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
    /// Context length for this model
    #[serde(default)]
    pub context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Supervisor {
    #[serde(default = "default_restart_policy")]
    pub restart_policy: String,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_delay_ms")]
    pub restart_delay_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            restart_policy: default_restart_policy(),
            max_restarts: default_max_restarts(),
            restart_delay_ms: default_restart_delay_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
        }
    }
}

fn default_proxy_enabled() -> bool {
    false
}

fn default_proxy_host() -> String {
    "0.0.0.0".to_string()
}

pub const DEFAULT_PROXY_PORT: u16 = 11434;

fn default_proxy_port() -> u16 {
    DEFAULT_PROXY_PORT
}

fn default_proxy_timeout() -> u64 {
    300
}

fn default_startup_timeout() -> u64 {
    120
}

fn default_circuit_breaker_threshold() -> u32 {
    3
}

fn default_circuit_breaker_cooldown() -> u64 {
    60
}

fn default_enabled() -> bool {
    true
}

fn default_restart_policy() -> String {
    "always".to_string()
}

fn default_max_restarts() -> u32 {
    10
}

fn default_restart_delay_ms() -> u64 {
    3000
}

fn default_health_check_interval_ms() -> u64 {
    5000
}

/// Maximum request body size in bytes (16 MB)
pub const MAX_REQUEST_BODY_SIZE: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_sampling_templates() {
        let config = Config::default();
        let templates = &config.sampling_templates;

        // Verify all 4 built-in profiles are present
        assert!(templates.contains_key("coding"));
        assert!(templates.contains_key("chat"));
        assert!(templates.contains_key("analysis"));
        assert!(templates.contains_key("creative"));

        // Verify coding template has expected values
        let coding = templates.get("coding").unwrap();
        assert_eq!(coding.temperature, Some(0.3));
        assert_eq!(coding.top_p, Some(0.9));

        // Verify creative template has expected values
        let creative = templates.get("creative").unwrap();
        assert_eq!(creative.temperature, Some(0.9));
        assert_eq!(creative.top_p, Some(0.95));
    }

    #[test]
    fn test_sampling_templates_toml_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();

        let loaded: Config = toml::from_str(&toml_str).unwrap();
        let loaded_templates = &loaded.sampling_templates;
        let default_templates = &config.sampling_templates;

        // Verify all profile values match after round-trip
        let profile_names = vec![
            "coding".to_string(),
            "chat".to_string(),
            "analysis".to_string(),
            "creative".to_string(),
        ];
        for profile_name in profile_names {
            let default = default_templates.get(&profile_name).unwrap();
            let loaded = loaded_templates.get(&profile_name).unwrap();
            assert_eq!(default, loaded);
        }
    }

    #[test]
    fn test_sampling_templates_serde_custom() {
        let mut templates = HashMap::new();
        let custom = SamplingParams {
            temperature: Some(0.5),
            top_k: Some(100),
            ..Default::default()
        };
        templates.insert("custom".to_string(), custom.clone());

        let config = Config {
            sampling_templates: templates,
            ..Default::default()
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let loaded: Config = toml::from_str(&toml_str).unwrap();

        let loaded_custom = loaded.sampling_templates.get("custom").unwrap();
        assert_eq!(loaded_custom.temperature, Some(0.5));
        assert_eq!(loaded_custom.top_k, Some(100));
    }
}
