use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSample {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    pub gpu_utilization_pct: Option<u8>,
    pub vram: Option<VramInfo>,
    pub models_loaded: u64,
    /// Per-model loaded/idle status mirrored from `tama_core::gpu::MetricSample.models`.
    ///
    /// `#[serde(default)]` keeps the dashboard resilient if the backend is
    /// slightly out of sync (e.g. during a partial rollout) or if older cached
    /// payloads without this field are encountered — missing arrays decode as
    /// an empty `Vec` rather than failing the whole sample.
    #[serde(default)]
    pub models: Vec<ModelStatus>,
    pub tps: Option<f32>,
    pub prompt_tps: Option<f32>,
    pub cache_hit_pct: Option<f32>,
    pub spec_accept_pct: Option<f32>,
    #[serde(default)]
    pub spec_decoding_active: bool,
    pub inference_last_updated_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VramInfo {
    pub used_mib: u64,
    pub total_mib: u64,
}

/// Frontend mirror of `tama_core::gpu::ModelStatus`.
///
/// Kept private to this module so the dashboard owns its wire shape; the only
/// contract with the backend is the JSON field names, which must match the
/// server-side struct exactly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(deprecated)]
pub struct ModelStatus {
    pub id: String,
    #[serde(default)]
    pub db_id: Option<i64>,
    #[serde(default)]
    pub api_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    pub backend: String,
    #[deprecated(since = "1.45.0", note = "use state field instead")]
    #[serde(default)]
    pub loaded: bool,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub hf_architecture_type: Option<String>,
    #[serde(default)]
    pub hf_base_model: Option<String>,
}

/// Format a number with comma separators (e.g. `8460` → `"8,460"`).
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

/// Filter models to only those that are currently active (ready, loading, or unloading).
///
/// Used by the dashboard to render the Active Models list and by the
/// "X loaded" summary heading. Extracted as a free function so it can
/// be unit-tested independently of the Leptos reactive view.
pub fn active_models(models: &[ModelStatus]) -> Vec<ModelStatus> {
    models
        .iter()
        .filter(|m| matches!(m.state.as_str(), "ready" | "loading" | "unloading"))
        .cloned()
        .collect()
}

/// Returns models whose state is NOT one of the "active" states.
/// These are models that are idle, failed, or otherwise not running.
/// Note: Models with an empty state string are treated as inactive.
/// This matches the behavior of `active_models()` which only considers
/// "ready", "loading", and "unloading" as active states.
pub fn inactive_models(models: &[ModelStatus]) -> Vec<ModelStatus> {
    models
        .iter()
        .filter(|m| !matches!(m.state.as_str(), "ready" | "loading" | "unloading"))
        .cloned()
        .collect()
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
pub fn model_display_name(m: &ModelStatus) -> String {
    m.display_name
        .as_deref()
        .or(m.api_name.as_deref())
        .unwrap_or(m.id.as_str())
        .to_string()
}

/// Sort models by base model, then by display name as a tiebreaker.
pub fn model_sort_key(m: &ModelStatus) -> (String, String) {
    let primary = m
        .hf_base_model
        .clone()
        .unwrap_or_else(|| model_display_name(m));
    let secondary = model_display_name(m);
    (primary, secondary)
}
