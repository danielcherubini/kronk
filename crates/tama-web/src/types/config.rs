//! Mirror types for Config that can be used from WASM.
//!
//! These types mirror the tama-core config types but use BTreeMap instead of HashMap
//! for deterministic JSON serialization. They are designed to be serialized/deserialized
//! with serde_json for the WASM frontend.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Import types from tama_core
use crate::api::StructuredConfigBody;
use tama_core::config::{
    BackendConfig as CoreBackendConfig, Config as CoreConfig, General as CoreGeneral,
    ModelConfig as CoreModelConfig, ModelModalities as CoreModelModalities,
    ProxyConfig as CoreProxyConfig, Supervisor as CoreSupervisor,
};
use tama_core::config::{
    HealthCheck as CoreHealthCheck, QuantEntry as CoreQuantEntry, QuantKind as CoreQuantKind,
};
use tama_core::profiles::SamplingParams as CoreSamplingParams;

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
    /// HuggingFace API token for downloading gated models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "default_update_check_interval")]
    pub update_check_interval: u32,
}

fn default_update_check_interval() -> u32 {
    12
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
    /// Number of parallel slots for this model
    #[serde(default)]
    pub num_parallel: Option<u32>,
    /// DEPRECATED — kept for migration deserialization only.
    #[serde(default, skip_serializing)]
    pub profile: Option<String>,
    /// API name for model identifier in OpenAI API responses
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_name: Option<String>,
    /// Default GPU layers
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// Available quantizations
    #[serde(default, skip_serializing_if = "is_btreemap_empty")]
    pub quants: BTreeMap<String, QuantEntry>,
    /// Model modalities (input/output types)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelModalities>,
    /// Pretty display name for UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Whether all parallel slots share a single unified KV cache pool.
    #[serde(default)]
    pub kv_unified: bool,
    /// Forward-compatibility: preserve unknown fields
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Model modality configuration (input/output types like "text", "image").
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

/// Convert from CoreModelModalities to mirror type.
impl From<CoreModelModalities> for ModelModalities {
    fn from(m: CoreModelModalities) -> Self {
        Self {
            input: m.input,
            output: m.output,
        }
    }
}

/// Convert from mirror ModelModalities to core type.
impl From<ModelModalities> for CoreModelModalities {
    fn from(m: ModelModalities) -> Self {
        Self {
            input: m.input,
            output: m.output,
        }
    }
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
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default)]
    pub auto_unload: bool,
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
    /// How often the download queue processor checks for new items (in seconds).
    #[serde(default = "default_download_queue_poll_interval")]
    pub download_queue_poll_interval_secs: u64,
    /// Maximum number of models that can be loaded simultaneously.
    /// When a new model is requested and the limit is reached, the
    /// least-recently-used (LRU) model is automatically unloaded first.
    /// Set to 0 for unlimited (disabled). Default: 1.
    #[serde(default = "default_max_loaded_models")]
    pub max_loaded_models: u32,
}

/// Main configuration struct.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub general: General,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
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

fn default_download_queue_poll_interval() -> u64 {
    2
}

fn default_max_loaded_models() -> u32 {
    1
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

// ── Conversions between tama_core::Config and mirror types ───────────────────

/// Convert from tama_core::config::QuantEntry to mirror type.
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

/// Convert from mirror QuantEntry to tama_core::config::QuantEntry.
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

/// Convert from tama_core::config::QuantKind to mirror type.
impl From<CoreQuantKind> for QuantKind {
    fn from(q: CoreQuantKind) -> Self {
        match q {
            CoreQuantKind::Model => QuantKind::Model,
            CoreQuantKind::Mmproj => QuantKind::Mmproj,
        }
    }
}

/// Convert from mirror QuantKind to tama_core::config::QuantKind.
impl From<QuantKind> for CoreQuantKind {
    fn from(q: QuantKind) -> Self {
        match q {
            QuantKind::Model => CoreQuantKind::Model,
            QuantKind::Mmproj => CoreQuantKind::Mmproj,
        }
    }
}

/// Convert from tama_core::config::HealthCheck to mirror type.
impl From<tama_core::config::HealthCheck> for HealthCheck {
    fn from(h: tama_core::config::HealthCheck) -> Self {
        Self {
            url: h.url,
            interval_ms: h.interval_ms,
            timeout_ms: h.timeout_ms,
        }
    }
}

/// Convert from mirror HealthCheck to tama_core::config::HealthCheck.
impl From<HealthCheck> for CoreHealthCheck {
    fn from(h: HealthCheck) -> Self {
        Self {
            url: h.url,
            interval_ms: h.interval_ms,
            timeout_ms: h.timeout_ms,
        }
    }
}

/// Convert from tama_core::profiles::SamplingParams to mirror type.
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

/// Convert from mirror SamplingParams to tama_core::profiles::SamplingParams.
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
            hf_token: g.hf_token,
            update_check_interval: g.update_check_interval,
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
            hf_token: g.hf_token,
            update_check_interval: g.update_check_interval,
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
            num_parallel: m.num_parallel,
            profile: None, // Skip serializing - deprecated field
            api_name: m.api_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants.into_iter().map(|(k, v)| (k, v.into())).collect(),
            modalities: m.modalities.map(Into::into),
            display_name: m.display_name,
            kv_unified: m.kv_unified,
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
            num_parallel: m.num_parallel,
            profile: None, // Skip serializing - deprecated field
            api_name: m.api_name,
            gpu_layers: m.gpu_layers,
            quants: m.quants.into_iter().map(|(k, v)| (k, v.into())).collect(),
            modalities: m.modalities.map(Into::into),
            display_name: m.display_name,
            kv_unified: m.kv_unified,
            db_id: None, // not carried through mirror types
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
            host: p.host,
            port: p.port,
            auto_unload: p.auto_unload,
            idle_timeout_secs: p.idle_timeout_secs,
            startup_timeout_secs: p.startup_timeout_secs,
            circuit_breaker_threshold: p.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: p.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: p.metrics_retention_secs,
            download_queue_poll_interval_secs: p.download_queue_poll_interval_secs,
            max_loaded_models: p.max_loaded_models,
        }
    }
}

/// Convert from mirror ProxyConfig to CoreProxyConfig.
impl From<ProxyConfig> for CoreProxyConfig {
    fn from(p: ProxyConfig) -> Self {
        Self {
            host: p.host,
            port: p.port,
            auto_unload: p.auto_unload,
            idle_timeout_secs: p.idle_timeout_secs,
            startup_timeout_secs: p.startup_timeout_secs,
            circuit_breaker_threshold: p.circuit_breaker_threshold,
            circuit_breaker_cooldown_seconds: p.circuit_breaker_cooldown_seconds,
            metrics_retention_secs: p.metrics_retention_secs,
            download_queue_poll_interval_secs: p.download_queue_poll_interval_secs,
            max_loaded_models: p.max_loaded_models,
        }
    }
}

/// Convert from CoreConfig to mirror type.
impl From<CoreConfig> for Config {
    fn from(c: CoreConfig) -> Self {
        Self {
            general: c.general.into(),
            backends: c.backends.into_iter().map(|(k, v)| (k, v.into())).collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── QuantKind serialization tests ─────────────────────────────────────

    #[test]
    fn test_quant_kind_serialization() {
        let json_model = serde_json::to_string(&QuantKind::Model).unwrap();
        assert!(json_model.contains("model"));
        let deserialized: QuantKind = serde_json::from_str(&json_model).unwrap();
        assert_eq!(deserialized, QuantKind::Model);

        let json_mmproj = serde_json::to_string(&QuantKind::Mmproj).unwrap();
        assert!(json_mmproj.contains("mmproj"));
        let deserialized: QuantKind = serde_json::from_str(&json_mmproj).unwrap();
        assert_eq!(deserialized, QuantKind::Mmproj);
    }

    // ── QuantEntry serialization tests ────────────────────────────────────

    #[test]
    fn test_quant_entry_serialization() {
        let entry = QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: Some(5_000_000),
            context_length: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: QuantEntry = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.file, "model-Q4_K_M.gguf");
        assert_eq!(deserialized.kind, QuantKind::Model);
        assert_eq!(deserialized.size_bytes, Some(5_000_000));
    }

    #[test]
    fn test_quant_entry_no_size() {
        let entry = QuantEntry {
            file: "model.gguf".to_string(),
            kind: QuantKind::Model,
            size_bytes: None,
            context_length: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: QuantEntry = serde_json::from_str(&json).unwrap();

        assert!(deserialized.size_bytes.is_none());
    }

    // ── HealthCheck serialization tests ───────────────────────────────────

    #[test]
    fn test_health_check_serialization() {
        let health = HealthCheck {
            url: Some("http://localhost:8080/health".to_string()),
            interval_ms: Some(5000),
            timeout_ms: Some(3000),
        };

        let json = serde_json::to_string(&health).unwrap();
        let deserialized: HealthCheck = serde_json::from_str(&json).unwrap();

        assert_eq!(
            deserialized.url,
            Some("http://localhost:8080/health".to_string())
        );
        assert_eq!(deserialized.interval_ms, Some(5000));
        assert_eq!(deserialized.timeout_ms, Some(3000));
    }

    // ── SamplingParams serialization tests ────────────────────────────────

    #[test]
    fn test_sampling_params_serialization() {
        let params = SamplingParams {
            temperature: Some(0.7),
            top_k: Some(40),
            top_p: Some(0.95),
            min_p: Some(0.05),
            presence_penalty: Some(0.1),
            frequency_penalty: Some(0.2),
            repeat_penalty: Some(1.1),
        };

        let json = serde_json::to_string(&params).unwrap();
        let deserialized: SamplingParams = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.temperature, Some(0.7));
        assert_eq!(deserialized.top_k, Some(40));
        assert_eq!(deserialized.top_p, Some(0.95));
    }

    #[test]
    fn test_sampling_params_empty() {
        let params = SamplingParams::default();
        let json = serde_json::to_string(&params).unwrap();
        // Default should serialize to empty object or minimal JSON
        assert!(!json.is_empty());
    }

    // ── General config serialization tests ────────────────────────────────

    #[test]
    fn test_general_serialization() {
        let general = General {
            log_level: "info".to_string(),
            models_dir: None,
            logs_dir: None,
            hf_token: None,
            update_check_interval: 24,
        };

        let json = serde_json::to_string(&general).unwrap();
        let deserialized: General = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.update_check_interval, 24);
        assert_eq!(deserialized.log_level, "info");
    }

    // ── Supervisor config serialization tests ─────────────────────────────

    #[test]
    fn test_supervisor_serialization() {
        let supervisor = Supervisor {
            restart_policy: "always".to_string(),
            max_restarts: 3,
            restart_delay_ms: 5000,
            health_check_interval_ms: 10000,
            health_check_timeout_ms: 5000,
            health_check_retries: 2,
        };

        let json = serde_json::to_string(&supervisor).unwrap();
        let deserialized: Supervisor = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.restart_policy, "always");
        assert_eq!(deserialized.max_restarts, 3);
    }

    // ── ProxyConfig serialization tests ───────────────────────────────────

    #[test]
    fn test_proxy_config_serialization() {
        let proxy = ProxyConfig {
            host: "0.0.0.0".to_string(),
            port: 8080,
            auto_unload: false,
            idle_timeout_secs: 300,
            startup_timeout_secs: 60,
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown_seconds: 300,
            metrics_retention_secs: 86400,
            download_queue_poll_interval_secs: 2,
            max_loaded_models: 1,
        };

        let json = serde_json::to_string(&proxy).unwrap();
        let deserialized: ProxyConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.host, "0.0.0.0");
        assert_eq!(deserialized.port, 8080);
        assert!(!deserialized.auto_unload, "auto_unload should be false");
        assert_eq!(deserialized.idle_timeout_secs, 300);
        assert_eq!(deserialized.circuit_breaker_threshold, 5);
    }

    // ── ModelModalities serialization tests ───────────────────────────────

    #[test]
    fn test_model_modalities_serialization() {
        let modalities = ModelModalities {
            input: vec!["text".to_string()],
            output: vec!["text".to_string()],
        };

        let json = serde_json::to_string(&modalities).unwrap();
        let deserialized: ModelModalities = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.input, vec!["text".to_string()]);
        assert_eq!(deserialized.output, vec!["text".to_string()]);
    }

    #[test]
    fn test_model_modalities_empty() {
        let modalities = ModelModalities {
            input: vec![],
            output: vec![],
        };

        let json = serde_json::to_string(&modalities).unwrap();
        let deserialized: ModelModalities = serde_json::from_str(&json).unwrap();

        assert!(deserialized.input.is_empty());
        assert!(deserialized.output.is_empty());
    }

    // ── is_btreemap_empty tests ───────────────────────────────────────────

    #[test]
    fn test_is_btreemap_empty_true() {
        let map: BTreeMap<String, String> = BTreeMap::new();
        assert!(is_btreemap_empty(&map));
    }

    #[test]
    fn test_is_btreemap_empty_false() {
        let mut map = BTreeMap::new();
        map.insert("key".to_string(), "value".to_string());
        assert!(!is_btreemap_empty(&map));
    }
}
