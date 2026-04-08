//! Mirror types for Config that can be used from WASM.
//!
//! These types mirror the koji-core config types but use BTreeMap instead of HashMap
//! for deterministic JSON serialization. They are designed to be serialized/deserialized
//! with serde_json for the WASM frontend.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Import types from koji_core
use crate::api::StructuredConfigBody;
use koji_core::config::{
    BackendConfig as CoreBackendConfig, Config as CoreConfig, General as CoreGeneral,
    ModelConfig as CoreModelConfig, ProxyConfig as CoreProxyConfig, Supervisor as CoreSupervisor,
};
use koji_core::config::{
    HealthCheck as CoreHealthCheck, QuantEntry as CoreQuantEntry, QuantKind as CoreQuantKind,
};
use koji_core::profiles::SamplingParams as CoreSamplingParams;

/// What kind of file a quant entry represents.
///
/// Used to distinguish regular GGUF model quants from auxiliary files like
/// vision projectors (mmproj).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    /// A regular GGUF model quantization (Q4_K_M, Q8_0, F16, etc.).
    #[default]
    Model,
    /// A vision projector (mmproj-*.gguf). Passed via `--mmproj` to llama.cpp.
    Mmproj,
}

/// A quantization entry for a model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantEntry {
    pub file: String,
    /// What kind of file this is. Defaults to `Model` for backward compat.
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

/// Health check configuration for a model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

/// Sampling parameters for LLM inference.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}

/// General configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
}

/// Backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
    /// Optional version pin.
    #[serde(default)]
    pub version: Option<String>,
}

/// Model configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    /// Model card reference in "company/modelname" format.
    #[serde(default)]
    pub model: Option<String>,
    /// Which quant to use from the model card (e.g. "Q4_K_M").
    #[serde(default)]
    pub quant: Option<String>,
    /// Which mmproj (vision projector) to use, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
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
    /// DEPRECATED — kept for migration deserialization only.
    #[serde(default, skip_serializing)]
    pub profile: Option<String>,
    /// Display name for UI
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Default GPU layers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// Available quantizations
    #[serde(default, skip_serializing_if = "is_btreemap_empty")]
    pub quants: BTreeMap<String, QuantEntry>,
    /// Forward-compatibility: preserve unknown fields
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Supervisor configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Supervisor {
    #[serde(default = "default_restart_policy")]
    pub restart_policy: String,
    #[serde(default = "default_max_restarts")]
    pub max_restarts: u32,
    #[serde(default = "default_restart_delay_ms")]
    pub restart_delay_ms: u64,
    #[serde(default = "default_health_check_interval_ms")]
    pub health_check_interval_ms: u64,
    #[serde(default = "default_health_check_timeout_ms")]
    pub health_check_timeout_ms: u64,
    #[serde(default = "default_health_check_retries")]
    pub health_check_retries: u32,
}

/// Proxy configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    #[serde(default = "default_metrics_retention")]
    pub metrics_retention_secs: u64,
}

/// Main configuration struct.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub general: General,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
    #[serde(default)]
    pub supervisor: Supervisor,
    #[serde(default)]
    pub sampling_templates: BTreeMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// The directory this config was loaded from.
    /// Skipped in serialization (managed separately by backend).
    #[serde(skip)]
    pub loaded_from: Option<std::path::PathBuf>,
}

/// Default helper functions for Config fields.
fn default_proxy_enabled() -> bool {
    false
}

fn default_proxy_host() -> String {
    "0.0.0.0".to_string()
}

const DEFAULT_PROXY_PORT: u16 = 11434;

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

fn default_metrics_retention() -> u64 {
    86_400
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

fn default_health_check_timeout_ms() -> u64 {
    30000
}

fn default_health_check_retries() -> u32 {
    3
}

fn is_btreemap_empty<K, V>(map: &BTreeMap<K, V>) -> bool {
    map.is_empty()
}

// ── Conversions between koji_core::Config and mirror types ───────────────────

/// Convert from koji_core::config::QuantEntry to mirror type.
impl From<CoreQuantEntry> for QuantEntry {
    fn from(q: CoreQuantEntry) -> Self {
        Self {
            file: q.file,
            kind: q.kind.into(),
            size_bytes: q.size_bytes,
            context_length: q.context_length,
        }
    }
}

/// Convert from mirror QuantEntry to koji_core::config::QuantEntry.
impl From<QuantEntry> for CoreQuantEntry {
    fn from(q: QuantEntry) -> Self {
        Self {
            file: q.file,
            kind: q.kind.into(),
            size_bytes: q.size_bytes,
            context_length: q.context_length,
        }
    }
}

/// Convert from koji_core::config::QuantKind to mirror type.
impl From<CoreQuantKind> for QuantKind {
    fn from(q: CoreQuantKind) -> Self {
        match q {
            CoreQuantKind::Model => QuantKind::Model,
            CoreQuantKind::Mmproj => QuantKind::Mmproj,
        }
    }
}

/// Convert from mirror QuantKind to koji_core::config::QuantKind.
impl From<QuantKind> for CoreQuantKind {
    fn from(q: QuantKind) -> Self {
        match q {
            QuantKind::Model => CoreQuantKind::Model,
            QuantKind::Mmproj => CoreQuantKind::Mmproj,
        }
    }
}

/// Convert from koji_core::config::HealthCheck to mirror type.
impl From<koji_core::config::HealthCheck> for HealthCheck {
    fn from(h: koji_core::config::HealthCheck) -> Self {
        Self {
            url: h.url,
            interval_ms: h.interval_ms,
            timeout_ms: h.timeout_ms,
        }
    }
}

/// Convert from mirror HealthCheck to koji_core::config::HealthCheck.
impl From<HealthCheck> for CoreHealthCheck {
    fn from(h: HealthCheck) -> Self {
        Self {
            url: h.url,
            interval_ms: h.interval_ms,
            timeout_ms: h.timeout_ms,
        }
    }
}

/// Convert from koji_core::profiles::SamplingParams to mirror type.
impl From<CoreSamplingParams> for SamplingParams {
    fn from(s: CoreSamplingParams) -> Self {
        Self {
            temperature: s.temperature,
            top_k: s.top_k,
            top_p: s.top_p,
            min_p: s.min_p,
            presence_penalty: s.presence_penalty,
            frequency_penalty: s.frequency_penalty,
            repeat_penalty: s.repeat_penalty,
        }
    }
}

/// Convert from mirror SamplingParams to koji_core::profiles::SamplingParams.
impl From<SamplingParams> for CoreSamplingParams {
    fn from(s: SamplingParams) -> Self {
        Self {
            temperature: s.temperature,
            top_k: s.top_k,
            top_p: s.top_p,
            min_p: s.min_p,
            presence_penalty: s.presence_penalty,
            frequency_penalty: s.frequency_penalty,
            repeat_penalty: s.repeat_penalty,
        }
    }
}

/// Convert from CoreGeneral to mirror type.
impl From<CoreGeneral> for General {
    fn from(g: CoreGeneral) -> Self {
        Self {
            log_level: g.log_level,
            models_dir: g.models_dir,
            logs_dir: g.logs_dir,
        }
    }
}

/// Convert from mirror General to CoreGeneral.
impl From<General> for CoreGeneral {
    fn from(g: General) -> Self {
        Self {
            log_level: g.log_level,
            models_dir: g.models_dir,
            logs_dir: g.logs_dir,
        }
    }
}

/// Convert from CoreBackendConfig to mirror type.
impl From<CoreBackendConfig> for BackendConfig {
    fn from(b: CoreBackendConfig) -> Self {
        Self {
            path: b.path,
            default_args: b.default_args,
            health_check_url: b.health_check_url,
            version: b.version,
        }
    }
}

/// Convert from mirror BackendConfig to CoreBackendConfig.
impl From<BackendConfig> for CoreBackendConfig {
    fn from(b: BackendConfig) -> Self {
        Self {
            path: b.path,
            default_args: b.default_args,
            health_check_url: b.health_check_url,
            version: b.version,
        }
    }
}

/// Convert from CoreModelConfig to mirror type.
impl From<CoreModelConfig> for ModelConfig {
    fn from(m: CoreModelConfig) -> Self {
        Self {
            backend: m.backend,
            args: m.args,
            sampling: m.sampling.map(Into::into),
            model: m.model,
            quant: m.quant,
            mmproj: m.mmproj,
            port: m.port,
            health_check: m.health_check.map(Into::into),
            enabled: m.enabled,
            context_length: m.context_length,
            profile: None, // Skip serializing - deprecated field
            display_name: m.display_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants.into_iter().map(|(k, v)| (k, v.into())).collect(),
            extra: None, // Forward-compat field - preserve unknown fields on POST
        }
    }
}

/// Convert from mirror ModelConfig to CoreModelConfig.
impl From<ModelConfig> for CoreModelConfig {
    fn from(m: ModelConfig) -> Self {
        Self {
            backend: m.backend,
            args: m.args,
            sampling: m.sampling.map(Into::into),
            model: m.model,
            quant: m.quant,
            mmproj: m.mmproj,
            port: m.port,
            health_check: m.health_check.map(Into::into),
            enabled: m.enabled,
            context_length: m.context_length,
            profile: None, // Skip serializing - deprecated field
            display_name: m.display_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants.into_iter().map(|(k, v)| (k, v.into())).collect(),
        }
    }
}

/// Convert from CoreSupervisor to mirror type.
impl From<CoreSupervisor> for Supervisor {
    fn from(s: CoreSupervisor) -> Self {
        Self {
            restart_policy: s.restart_policy,
            max_restarts: s.max_restarts,
            restart_delay_ms: s.restart_delay_ms,
            health_check_interval_ms: s.health_check_interval_ms,
            health_check_timeout_ms: s.health_check_timeout_ms,
            health_check_retries: s.health_check_retries,
        }
    }
}

/// Convert from mirror Supervisor to CoreSupervisor.
impl From<Supervisor> for CoreSupervisor {
    fn from(s: Supervisor) -> Self {
        Self {
            restart_policy: s.restart_policy,
            max_restarts: s.max_restarts,
            restart_delay_ms: s.restart_delay_ms,
            health_check_interval_ms: s.health_check_interval_ms,
            health_check_timeout_ms: s.health_check_timeout_ms,
            health_check_retries: s.health_check_retries,
        }
    }
}

/// Convert from CoreProxyConfig to mirror type.
impl From<CoreProxyConfig> for ProxyConfig {
    fn from(p: CoreProxyConfig) -> Self {
        Self {
            enabled: p.enabled,
            host: p.host,
            port: p.port,
            idle_timeout_secs: p.idle_timeout_secs,
            startup_timeout_secs: p.startup_timeout_secs,
            circuit_breaker_threshold: p.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: p.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: p.metrics_retention_secs,
        }
    }
}

/// Convert from mirror ProxyConfig to CoreProxyConfig.
impl From<ProxyConfig> for CoreProxyConfig {
    fn from(p: ProxyConfig) -> Self {
        Self {
            enabled: p.enabled,
            host: p.host,
            port: p.port,
            idle_timeout_secs: p.idle_timeout_secs,
            startup_timeout_secs: p.startup_timeout_secs,
            circuit_breaker_threshold: p.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: p.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: p.metrics_retention_secs,
        }
    }
}

/// Convert from CoreConfig to mirror type.
impl From<CoreConfig> for Config {
    fn from(c: CoreConfig) -> Self {
        Self {
            general: c.general.into(),
            backends: c.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
            models: c.models.into_iter().map(|(k, v)| (k, v.into())).collect(),
            supervisor: c.supervisor.into(),
            sampling_templates: c
                .sampling_templates
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            proxy: c.proxy.into(),
            loaded_from: c.loaded_from, // Preserved for internal use, not serialized
        }
    }
}

/// Convert from mirror Config to CoreConfig.
impl From<StructuredConfigBody> for CoreConfig {
    fn from(b: StructuredConfigBody) -> Self {
        Self {
            general: b.general.into(),
            backends: b.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
            models: b.models.into_iter().map(|(k, v)| (k, v.into())).collect(),
            supervisor: b.supervisor.into(),
            sampling_templates: b
                .sampling_templates
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            proxy: b.proxy.into(),
            loaded_from: None, // Will be restored from proxy config before save
        }
    }
}

/// Convert from mirror Config to CoreConfig.
impl From<Config> for CoreConfig {
    fn from(c: Config) -> Self {
        Self {
            general: c.general.into(),
            backends: c.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
            models: c.models.into_iter().map(|(k, v)| (k, v.into())).collect(),
            supervisor: c.supervisor.into(),
            sampling_templates: c
                .sampling_templates
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect(),
            proxy: c.proxy.into(),
            loaded_from: c.loaded_from, // Preserved for internal use
        }
    }
}
