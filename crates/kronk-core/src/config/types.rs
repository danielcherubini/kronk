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
    pub custom_profiles: Option<HashMap<String, SamplingParams>>,
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
    /// Source identifier (e.g. "nvidia/Nemotron-Mini-4B-Instruct")
    #[serde(default)]
    pub source: Option<String>,
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
