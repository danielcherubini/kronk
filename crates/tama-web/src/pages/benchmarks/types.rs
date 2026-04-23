//! Types for the benchmarks page.

use serde::{Deserialize, Serialize};

/// Valid benchmark type identifiers and their display labels.
pub const BENCHMARK_TYPES: &[(&str, &str)] = &[
    ("baseline", "Baseline"),
    ("pp_sweep", "PP Sweep"),
    ("kv_quant_q8", "KV Quant (q8_0)"),
    ("kv_quant_q4", "KV Quant (q4_0)"),
    ("context_test", "Context Test"),
    ("spec_scan", "Spec Scan"),
    ("spec_sweep", "Spec Sweep"),
];

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
    /// Engine used for this benchmark: "llama_bench" or "llama_cli_spec".
    #[serde(default)]
    pub engine: Option<String>,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
    pub results: serde_json::Value,
}

/// Preset configurations — each one maps to a phase in the LLM inference
/// tuning methodology (see `llm-inference-tuning-methodology.md`). The
/// presets are ordered so running them top-to-bottom yields the
/// "measure-one-variable-at-a-time" workflow the methodology advocates:
#[derive(Debug, Clone)]
pub struct BenchmarkPresetSpec {
    pub pp_sizes: &'static str,
    pub tg_sizes: &'static str,
    pub batch_sizes: &'static str,
    pub ubatch_sizes: &'static str,
    pub kv_cache_type: &'static str,
    pub depth: &'static str,
}

/// Auto-fill presets for the LLaMA-Bench Test Type dropdown.
pub const LLAMA_BENCH_PRESETS: &[(&str, BenchmarkPresetSpec)] = &[
    (
        "baseline",
        BenchmarkPresetSpec {
            pp_sizes: "2048",
            tg_sizes: "128",
            batch_sizes: "",
            ubatch_sizes: "",
            kv_cache_type: "default",
            depth: "",
        },
    ),
    (
        "pp_sweep",
        BenchmarkPresetSpec {
            pp_sizes: "2048",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "512,1024,2048,4096",
            kv_cache_type: "default",
            depth: "",
        },
    ),
    (
        "kv_quant_q8",
        BenchmarkPresetSpec {
            pp_sizes: "0",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "2048",
            kv_cache_type: "q8_0",
            depth: "0,65536,131072",
        },
    ),
    (
        "kv_quant_q4",
        BenchmarkPresetSpec {
            pp_sizes: "0",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "2048",
            kv_cache_type: "q4_0",
            depth: "0,65536,131072",
        },
    ),
    (
        "context_test",
        BenchmarkPresetSpec {
            pp_sizes: "0",
            tg_sizes: "128",
            batch_sizes: "4096",
            ubatch_sizes: "2048",
            kv_cache_type: "q8_0",
            depth: "131072",
        },
    ),
];

/// Auto-fill presets for the Spec Decoding Test Type dropdown.
#[allow(clippy::type_complexity)]
pub const SPEC_BENCH_PRESETS: &[(&str, (&[u32], &str, &str, &str))] = &[
    ("spec_scan", (&[256], "16", "12", "48")),
    (
        "spec_sweep",
        (&[8, 16, 32, 48, 64], "8,16,32,48,64", "12,16,24", "32,48"),
    ),
];
///   1. Baseline — know your starting point.
///   2. Batch sweep — find the PP `-ub` knee (often the biggest single win).
///   3. KV quant @ depth — two presets (q8_0 / q4_0) so the user re-runs
///      once with each. Matched pair only; mismatched K/V falls back to CPU
///      attention and kills perf.
///   4. Depth validation — lock in the winner, run at real target context.
#[derive(Debug, Clone)]
pub struct BenchmarkPreset {
    pub label: &'static str,
    pub description: &'static str,
    pub pp_sizes: &'static [u32],
    pub tg_sizes: &'static [u32],
    pub runs: u32,
    pub threads: Option<Vec<u32>>,
    pub ngl_range: Option<&'static str>,
    #[allow(dead_code)]
    pub ctx_override: Option<u32>,
    pub batch_sizes: &'static [u32],
    pub ubatch_sizes: &'static [u32],
    pub kv_cache_type: Option<&'static str>,
    pub depth: &'static [u32],
    pub flash_attn: Option<bool>,
}

impl BenchmarkPreset {
    pub fn all() -> Vec<Self> {
        vec![
            Self {
                label: "1. Baseline",
                description: "Known-good flags. Record PP and TG as the reference point.",
                pp_sizes: &[2048],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("99"),
                ctx_override: None,
                batch_sizes: &[],
                ubatch_sizes: &[],
                kv_cache_type: None,
                depth: &[],
                flash_attn: Some(true),
            },
            Self {
                label: "2. Batch sweep",
                description: "Sweep -ub to find the PP knee. Pick the smallest -ub at the plateau.",
                pp_sizes: &[2048],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("99"),
                ctx_override: None,
                batch_sizes: &[4096],
                ubatch_sizes: &[512, 1024, 2048, 4096],
                kv_cache_type: None,
                depth: &[],
                flash_attn: Some(true),
            },
            Self {
                label: "3a. KV quant (q8_0)",
                description: "KV quant baseline at depth. Rerun with q4_0 next to compare.",
                pp_sizes: &[0],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("99"),
                ctx_override: None,
                batch_sizes: &[4096],
                ubatch_sizes: &[2048],
                kv_cache_type: Some("q8_0"),
                depth: &[0, 65536, 131072],
                flash_attn: Some(true),
            },
            Self {
                label: "3b. KV quant (q4_0)",
                description: "Half-size KV cache. Usually ties q8_0 at d=0; pulls ahead at 128k+.",
                pp_sizes: &[0],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("99"),
                ctx_override: None,
                batch_sizes: &[4096],
                ubatch_sizes: &[2048],
                kv_cache_type: Some("q4_0"),
                depth: &[0, 65536, 131072],
                flash_attn: Some(true),
            },
            Self {
                label: "4. Depth validation",
                description: "Lock winning KV config; run at your real target depth. Edit -d.",
                pp_sizes: &[0],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("99"),
                ctx_override: None,
                batch_sizes: &[4096],
                ubatch_sizes: &[2048],
                kv_cache_type: Some("q8_0"),
                depth: &[131072],
                flash_attn: Some(true),
            },
        ]
    }
}
