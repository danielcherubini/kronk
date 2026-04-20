//! Types for the benchmarks page.

use serde::{Deserialize, Serialize};

/// Parse a model JSON value into (id, display_name, quant).
/// The API returns `id` as an integer (db_id), not a string.
pub fn parse_model(m: &serde_json::Value) -> Option<(String, String, String)> {
    let id = m.get("id").map(|v| v.to_string()).unwrap_or_default();
    let name = m
        .get("display_name")
        .or_else(|| m.get("api_name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| id.clone());
    let quant = m
        .get("quant")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((id, name, quant))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
    pub results: serde_json::Value,
}

/// Preset configurations for quick benchmark setup.
#[derive(Debug, Clone)]
pub struct BenchmarkPreset {
    pub label: &'static str,
    pub pp_sizes: &'static [u32],
    pub tg_sizes: &'static [u32],
    pub runs: u32,
    pub threads: Option<Vec<u32>>,
    pub ngl_range: Option<&'static str>,
    #[allow(dead_code)]
    pub ctx_override: Option<u32>,
}

impl BenchmarkPreset {
    pub fn all() -> Vec<Self> {
        vec![
            Self {
                label: "Quick",
                pp_sizes: &[512],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: None,
                ctx_override: None,
            },
            Self {
                label: "VRAM Sweet Spot",
                pp_sizes: &[512],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("0-99+1"),
                ctx_override: Some(4096),
            },
            Self {
                label: "Thread Scaling",
                pp_sizes: &[64],
                tg_sizes: &[16],
                runs: 3,
                threads: Some(vec![1, 2, 4, 8, 16, 32]),
                ngl_range: None,
                ctx_override: None,
            },
        ]
    }
}
