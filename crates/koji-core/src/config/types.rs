use crate::profiles::SamplingParams;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// What kind of file a quant entry represents.
///
/// Used to distinguish regular GGUF model quants from auxiliary files like
/// vision projectors (mmproj). Drives both UI grouping and how the file is
/// passed on the server command line (`-m` vs `--mmproj`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum QuantKind {
    /// A regular GGUF model quantization (Q4_K_M, Q8_0, F16, etc.).
    #[default]
    Model,
    /// A vision projector (mmproj-*.gguf). Passed via `--mmproj` to llama.cpp.
    Mmproj,
}

impl QuantKind {
    /// Infer the kind from a filename. Defaults to `Model` for anything that
    /// doesn't match a known auxiliary-file pattern.
    pub fn from_filename(filename: &str) -> Self {
        let lower = filename.to_lowercase();
        if lower.starts_with("mmproj") && lower.ends_with(".gguf") {
            QuantKind::Mmproj
        } else {
            QuantKind::Model
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantEntry {
    pub file: String,
    /// What kind of file this is. Defaults to `Model` for backward compat
    /// with config files written before this field existed.
    #[serde(default)]
    pub kind: QuantKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

fn is_btreemap_empty<K, V>(map: &BTreeMap<K, V>) -> bool {
    map.is_empty()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: General,
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,
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
    /// How often the download queue processor checks for new items (in seconds).
    /// Default is 2, minimum is 1.
    #[serde(default = "default_download_queue_poll_interval")]
    pub download_queue_poll_interval_secs: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            host: default_proxy_host(),
            port: default_proxy_port(),
            idle_timeout_secs: default_proxy_timeout(),
            startup_timeout_secs: default_startup_timeout(),
            circuit_breaker_threshold: default_circuit_breaker_threshold(),
            circuit_breaker_cooldown_seconds: default_circuit_breaker_cooldown(),
            metrics_retention_secs: default_metrics_retention(),
            download_queue_poll_interval_secs: default_download_queue_poll_interval(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
    /// HuggingFace API token for downloading gated models.
    /// When set, this is exported as HF_TOKEN environment variable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hf_token: Option<String>,
    /// How often to check for updates (in hours). Default 12.
    #[serde(default = "crate::config::defaults::default_update_check_interval")]
    pub update_check_interval: u32,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
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
    /// Which mmproj (vision projector) to use, if any. References a key in
    /// `quants` whose entry has `kind = Mmproj`. When set, the launch command
    /// gets `--mmproj <path>` injected automatically.
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
    /// When present in an old config.toml, the migration reads this, resolves it to
    /// concrete SamplingParams, writes those into `sampling`, and clears this field.
    /// Must NOT be serialized back (skip_serializing).
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
    /// Modalities supported by this model (e.g. ["text", "image"] for input, ["text"] for output)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modalities: Option<ModelModalities>,
    /// Pretty display name for UI (e.g., "Unsloth: Gemma 4 26B A4B").
    /// Derived from HF repo name when pulling, but can be overridden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Integer database id — set at runtime when loading from DB, never
    /// persisted via serde (TOML or JSON). Used by the status endpoint to
    /// expose the canonical integer id for API consumers.
    #[serde(default, skip)]
    pub db_id: Option<i64>,
}

impl ModelConfig {
    /// Serialise to a ModelConfigRecord for DB storage.
    /// `repo_id` is the HF repo id (e.g. "unsloth/gemma-4-31B-it-GGUF").
    pub fn to_db_record(&self, repo_id: &str) -> crate::db::queries::ModelConfigRecord {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        crate::db::queries::ModelConfigRecord {
            id: 0, // auto-generated on insert
            repo_id: repo_id.to_string(),
            display_name: self.display_name.clone(),
            backend: self.backend.clone(),
            enabled: self.enabled,
            selected_quant: self.quant.clone(),
            selected_mmproj: self.mmproj.clone(),
            context_length: self.context_length,
            gpu_layers: self.gpu_layers,
            port: self.port,
            args: serde_json::to_string(&self.args).ok(),
            sampling: self
                .sampling
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok()),
            modalities: self
                .modalities
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok()),
            profile: self.profile.clone(),
            api_name: self.api_name.clone(),
            health_check: self
                .health_check
                .as_ref()
                .and_then(|s| serde_json::to_string(s).ok()),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Deserialise from a DB record. JSON fields are parsed; parse errors
    /// fall back to None / default so a bad JSON column never hard-fails.
    pub fn from_db_record(record: &crate::db::queries::ModelConfigRecord) -> Self {
        Self {
            backend: record.backend.clone(),
            enabled: record.enabled,
            display_name: record.display_name.clone(),
            api_name: record
                .api_name
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| Some(record.repo_id.clone())),
            port: record.port,
            context_length: record.context_length,
            gpu_layers: record.gpu_layers,
            model: Some(record.repo_id.clone()),
            quant: record.selected_quant.clone(),
            mmproj: record.selected_mmproj.clone(),
            args: record
                .args
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default(),
            sampling: record
                .sampling
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            modalities: record
                .modalities
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            health_check: record
                .health_check
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            profile: record.profile.clone(),
            quants: BTreeMap::new(), // Not stored in DB record
            db_id: Some(record.id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
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
    #[serde(default = "default_health_check_timeout_ms")]
    pub health_check_timeout_ms: u64,
    #[serde(default = "default_health_check_retries")]
    pub health_check_retries: u32,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            restart_policy: default_restart_policy(),
            max_restarts: default_max_restarts(),
            restart_delay_ms: default_restart_delay_ms(),
            health_check_interval_ms: default_health_check_interval_ms(),
            health_check_timeout_ms: default_health_check_timeout_ms(),
            health_check_retries: default_health_check_retries(),
        }
    }
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

fn default_metrics_retention() -> u64 {
    86_400
}

fn default_download_queue_poll_interval() -> u64 {
    2
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

    /// Test that the default `metrics_retention_secs` equals 86_400 (24 hours).
    #[test]
    fn test_proxy_config_default_metrics_retention() {
        let config = ProxyConfig::default();
        assert_eq!(config.metrics_retention_secs, 86_400);
    }

    /// Test that deserializing `metrics_retention_secs = 3600` sets the field correctly.
    /// Test that the default update check interval is applied when missing from config.
    #[test]
    fn test_general_config_update_check_interval_default() {
        let config: Config = toml::from_str(
            r#"
[general]
log_level = "info"
"#,
        )
        .unwrap();
        assert_eq!(config.general.update_check_interval, 12);
    }

    /// Test that a ModelConfig survives a round-trip through the DB record.
    #[test]
    fn test_model_config_round_trip() {
        let mc = ModelConfig {
            backend: "llama.cpp".to_string(),
            args: vec!["--n-gpu-layers".to_string(), "32".to_string()],
            sampling: Some(SamplingParams {
                temperature: Some(0.7),
                top_p: Some(0.9),
                ..Default::default()
            }),
            model: Some("owner/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: Some("mmproj-model.gguf".to_string()),
            port: Some(8080),
            health_check: Some(HealthCheck {
                url: Some("/health".to_string()),
                interval_ms: Some(1000),
                timeout_ms: Some(500),
            }),
            enabled: true,
            context_length: Some(4096),
            api_name: Some("my-model".to_string()),
            gpu_layers: Some(32),
            modalities: Some(ModelModalities {
                input: vec!["text".to_string(), "image".to_string()],
                output: vec!["text".to_string()],
            }),
            display_name: Some("My Custom Model".to_string()),
            ..Default::default()
        };

        let record = mc.to_db_record("owner/repo");
        let round_trip = ModelConfig::from_db_record(&record);

        assert_eq!(round_trip.backend, mc.backend);
        assert_eq!(round_trip.args, mc.args);
        assert_eq!(round_trip.sampling, mc.sampling);
        assert_eq!(round_trip.model, Some("owner/repo".to_string()));
        assert_eq!(round_trip.quant, mc.quant);
        assert_eq!(round_trip.mmproj, mc.mmproj);
        assert_eq!(round_trip.port, mc.port);
        assert_eq!(round_trip.health_check, mc.health_check);
        assert_eq!(round_trip.enabled, mc.enabled);
        assert_eq!(round_trip.context_length, mc.context_length);
        assert_eq!(round_trip.api_name, mc.api_name);
        assert_eq!(round_trip.gpu_layers, mc.gpu_layers);
        assert_eq!(round_trip.modalities, mc.modalities);
        assert_eq!(round_trip.display_name, mc.display_name);

        // quants should be empty as it's not persisted
        assert!(round_trip.quants.is_empty());
    }
}
