//! Benchmark API endpoints.
//!
//! Provides REST endpoints for triggering llama-bench benchmarks,
//! streaming progress via SSE, and managing benchmark history.

mod history;
mod mtp;
mod run;
mod spec;

// ── Shared imports (re-exported for sub-modules) ─────────────────────

use anyhow::{Context, Result};
use axum::response::sse::Event;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
    Json,
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::gpu::query_vram;
use crate::jobs::{JobEvent, JobKind, JobManager, JobStatus};
use crate::server::AppState;
use tama_core::bench::llama_cli_spec::{SpecBenchConfig, SpecType};

// ── Request/Response DTOs ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkRunRequest {
    pub model_id: String,
    /// Optional quant label (e.g. "Q6_K"). When provided, the benchmark uses
    /// the GGUF file for this specific quant instead of the default.
    #[serde(default)]
    pub quant: Option<String>,
    /// Optional backend name to use for llama-bench. If not provided, the
    /// backend is resolved from the model config.
    #[serde(default)]
    pub backend_name: Option<String>,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub warmup: u32,
    #[serde(default)]
    pub threads: Option<Vec<u32>>,
    #[serde(default)]
    pub ngl_range: Option<String>,
    #[serde(default)]
    pub ctx_override: Option<u32>,
    #[serde(default)]
    pub batch_sizes: Vec<u32>,
    #[serde(default)]
    pub ubatch_sizes: Vec<u32>,
    #[serde(default)]
    pub kv_cache_type: Option<String>,
    #[serde(default)]
    pub depth: Vec<u32>,
    #[serde(default)]
    pub flash_attn: Option<bool>,
    /// Identifies what kind of benchmark was run (e.g., "baseline", "pp_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkRunResponse {
    pub job_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SpecBenchmarkRunRequest {
    pub model_id: String,
    /// Optional quant label (e.g. "Q6_K"). When provided, the benchmark uses
    /// the GGUF file for this specific quant instead of the default.
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub backend_name: Option<String>,
    /// Optional GPU variant to use for the backend (e.g. "cpu", "cuda", "rocm", "vulkan").
    /// When provided, overrides config/DB resolution for the backend path.
    #[serde(default)]
    pub gpu_variant: Option<String>,
    pub spec_types: Vec<SpecType>,
    #[serde(default)]
    pub draft_max_values: Vec<u32>,
    #[serde(default)]
    pub ngram_n_values: Vec<u32>,
    #[serde(default)]
    pub ngram_m_values: Vec<u32>,
    /// N-gram minimum match values for n-gram-mod.
    #[serde(default)]
    pub ngram_min_values: Vec<u32>,
    /// N-gram maximum match values for n-gram-mod.
    #[serde(default)]
    pub ngram_max_values: Vec<u32>,
    #[serde(default = "default_min_hits")]
    pub ngram_min_hits: u32,
    #[serde(default = "default_gen_tokens")]
    pub gen_tokens: u32,
    #[serde(default = "default_runs")]
    pub runs: u32,
    #[serde(default)]
    pub ngl: Option<u32>,
    #[serde(default = "default_flash_attn")]
    pub flash_attn: bool,
    /// Identifies what kind of benchmark was run (e.g., "spec_scan", "spec_sweep").
    #[serde(default)]
    pub benchmark_type: Option<String>,
}

fn default_min_hits() -> u32 {
    1
}
fn default_gen_tokens() -> u32 {
    256
}
fn default_runs() -> u32 {
    3
}
fn default_flash_attn() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct BenchmarkHistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
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

// ── Re-exports from sub-modules ───────────────────────────────────────

pub use history::{
    benchmark_events, delete_benchmark, get_benchmark_result, list_benchmark_history,
};
pub use mtp::run_mtp_benchmark;
pub use run::{run_benchmark, run_benchmark_inner};
pub use spec::{run_spec_benchmark, run_spec_benchmark_inner, validate_spec_sweep};
