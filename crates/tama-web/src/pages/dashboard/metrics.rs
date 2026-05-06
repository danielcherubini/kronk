use js_sys::Date;
use leptos::prelude::{Get, RwSignal, Set, Update};
use log::warn;
use serde::{Deserialize, Serialize};

use crate::utils::extract_and_store_csrf_token;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VramInfo {
    pub used_mib: u64,
    pub total_mib: u64,
}

/// Frontend mirror of the backend `MetricsHistoryEntry` response type.
///
/// Uses `i64` for memory and GPU fields to match the JSON wire format
/// (SQLite stores integers as i64). Converted to `MetricSample` on ingestion.
#[derive(Debug, Clone, Deserialize)]
pub struct MetricsHistoryEntry {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: i64,
    pub ram_total_mib: i64,
    pub gpu_utilization_pct: Option<i64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
}

impl From<MetricsHistoryEntry> for MetricSample {
    fn from(entry: MetricsHistoryEntry) -> Self {
        MetricSample {
            ts_unix_ms: entry.ts_unix_ms,
            cpu_usage_pct: entry.cpu_usage_pct,
            ram_used_mib: entry.ram_used_mib as u64,
            ram_total_mib: entry.ram_total_mib as u64,
            gpu_utilization_pct: entry.gpu_utilization_pct.map(|v| v as u8),
            vram: entry.vram_used_mib.and_then(|used| {
                entry.vram_total_mib.map(|total| VramInfo {
                    used_mib: used as u64,
                    total_mib: total as u64,
                })
            }),
            models_loaded: 0,
            models: vec![],
        }
    }
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

/// Merge new metric samples into the buffer.
/// Combines, sorts by timestamp, deduplicates (keeping the FIRST entry for each timestamp),
/// and trims to the last `max_len` samples.
///
/// Keeping the first entry is intentional: SSE entries (which include `models` data)
/// are already in the buffer, and backfill entries (which have `models: vec![]`)
/// are extended after. Keeping the first preserves the richer SSE entry.
pub fn merge_samples(buf: &mut Vec<MetricSample>, new: Vec<MetricSample>, max_len: usize) {
    buf.extend(new);
    buf.sort_by_key(|s| s.ts_unix_ms);
    buf.dedup_by(|a, b| a.ts_unix_ms == b.ts_unix_ms); // keeps a (first), removes b (subsequent)
    if buf.len() > max_len {
        buf.drain(..buf.len() - max_len);
    }
}

/// Fetch metric history from the backend and merge into the history signal.
///
/// Applies a 5-second cooldown (tracked by `last_backfill`) to avoid
/// redundant requests. Used by both the SSE `lagged` handler and the
/// `visibilitychange` handler so both paths behave identically.
pub async fn backfill_metrics(history: RwSignal<Vec<MetricSample>>, last_backfill: RwSignal<u64>) {
    // Cooldown: skip if backfilled in the last 5 seconds
    let now = Date::now() as u64;
    if (now - last_backfill.get()) < 5000 {
        return;
    }
    last_backfill.set(now);

    let url = "/tama/v1/system/metrics/history?limit=450";
    match gloo_net::http::Request::get(url).send().await {
        Ok(resp) => {
            extract_and_store_csrf_token(&resp);
            match resp.json::<Vec<MetricsHistoryEntry>>().await {
                Ok(entries) => {
                    let new: Vec<MetricSample> = entries.into_iter().map(Into::into).collect();
                    if !new.is_empty() {
                        history.update(|buf| {
                            merge_samples(buf, new, 450);
                        });
                    }
                }
                Err(e) => warn!("backfill: failed to parse history JSON: {}", e),
            }
        }
        Err(e) => warn!("backfill: failed to fetch /metrics/history: {}", e),
    }
}
