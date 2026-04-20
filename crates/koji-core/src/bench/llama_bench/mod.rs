//! llama-bench integration for benchmarking GGUF files directly.
//!
//! Wraps the llama-bench binary from llama.cpp's tools/ directory. Runs raw
//! inference benchmarks without spawning a server.
//!
//! Split into:
//! - [`discovery`] — binary lookup and GPU-type inference (pure filesystem logic).
//! - [`args`] — CLI-argument construction from [`LlamaBenchConfig`].
//! - [`parse`] — JSON parsing of llama-bench's `-o json` output.
//!
//! This module's `mod.rs` keeps only the public surface ([`LlamaBenchConfig`],
//! [`find_llama_bench`], [`run_llama_bench`]) plus the async orchestrator that
//! ties the pieces together.

mod args;
mod discovery;
mod parse;

pub use discovery::find_llama_bench;

use crate::backends::ProgressSink;
use crate::bench::{BenchConfig, BenchReport, ModelInfo};
use crate::config::Config;
use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::process::Command;

/// Configuration for llama-bench specific parameters.
#[derive(Debug, Clone)]
pub struct LlamaBenchConfig {
    /// Prompt sizes to test (maps to -p)
    pub pp_sizes: Vec<u32>,
    /// Generation lengths to test (maps to -n)
    pub tg_sizes: Vec<u32>,
    /// Number of measurement runs (maps to -r)
    pub runs: u32,
    /// Warmup runs (handled by wrapper, not llama-bench itself)
    pub warmup: u32,
    /// Thread counts to test. None = auto-detect from system.
    pub threads: Option<Vec<u32>>,
    /// GPU layer range for sweet-spot sweep.
    /// Some("0-99+1") maps to --n-gpu-layers 0-99+1.
    /// None = use all layers (default).
    pub ngl_range: Option<String>,
    /// Optional context size override (maps to --fit-ctx)
    pub ctx_override: Option<u32>,
    /// Logical batch size (maps to -b). Sweep by comma-separating.
    pub batch_sizes: Vec<u32>,
    /// Physical micro-batch size (maps to -ub). Sweep by comma-separating.
    pub ubatch_sizes: Vec<u32>,
    /// KV cache type applied to BOTH -ctk and -ctv.
    /// Mismatched K/V quant falls back to CPU attention on most builds, so we
    /// only expose a single matched-pair value (e.g. "f16", "q8_0", "q4_0").
    pub kv_cache_type: Option<String>,
    /// Depth sweep (maps to -d). Tokens pre-filled into KV cache before timing.
    /// Critical for evaluating KV-cache quantisation at non-trivial context.
    pub depth: Vec<u32>,
    /// Flash attention toggle (maps to -fa 0|1). None = llama-bench default.
    pub flash_attn: Option<bool>,
}

/// Run a benchmark using llama-bench and return the report.
///
/// `backend_name` is an optional override — if provided, llama-bench is
/// resolved from that backend's installation path instead of the model's
/// configured backend.
///
/// This function is designed to be called from a background job — it streams
/// progress via the provided ProgressSink.
pub async fn run_llama_bench(
    config: &Config,
    model_id: &str,
    backend_name: Option<&str>,
    bench_config: &LlamaBenchConfig,
    progress: &dyn ProgressSink,
) -> Result<BenchReport> {
    use crate::db::OpenResult;

    let db_dir = Config::config_dir()?;
    let OpenResult { conn, .. } = crate::db::open(&db_dir)?;
    let model_configs = crate::db::load_model_configs(&conn)?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_id = if let Ok(db_id) = model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.as_str())
            .unwrap_or(model_id)
    } else {
        model_id
    };

    let (server_config, _backend_config) = config
        .resolve_server(&model_configs, resolved_id)
        .context("Failed to resolve server config for benchmark")?;

    let model_path = resolve_model_path(config, &db_dir, &conn, &model_configs, resolved_id)?;

    let target_backend = backend_name.unwrap_or(&server_config.backend);
    let backend_path = {
        let conn = Config::open_db();
        config.resolve_backend_path(target_backend, &conn)?
    };

    let bench_binary = discovery::find_llama_bench(&backend_path).context(format!(
        "llama-bench not found for backend '{}'. Install llama.cpp from source or set LLAMA_BENCH_PATH",
        target_backend
    ))?;

    // Get llama-bench version for reporting (best-effort).
    let _version_output = Command::new(&bench_binary)
        .arg("--version")
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    progress.log(&format!("Using llama-bench: {}", bench_binary.display()));
    progress.log(&format!("Model: {} ({})", model_id, model_path.display()));

    let args = args::build_args(&model_path, bench_config);

    progress.log(&format!(
        "Running: {} {}",
        bench_binary.display(),
        args.join(" ")
    ));

    let start_time = std::time::Instant::now();

    let output = Command::new(&bench_binary)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("Failed to execute llama-bench")?;

    let _duration = start_time.elapsed();

    if !output.stderr.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        for line in stderr.lines() {
            if !line.trim().is_empty() {
                progress.log(line);
            }
        }
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "llama-bench exited with error (code {}): {}",
            output.status,
            stderr
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summaries = parse::parse_bench_json(&stdout)?;

    // Prefer the human-friendly display name stored on the model config.
    // Fall back to the HF repo id, then the API name, then the raw model_id
    // (which is the db_id when called from the web UI — ugly but at least
    // identifies the row).
    let display_name = model_configs
        .get(resolved_id)
        .and_then(|mc| {
            mc.display_name
                .clone()
                .or_else(|| mc.api_name.clone())
                .or_else(|| mc.model.clone())
        })
        .unwrap_or_else(|| model_id.to_string());

    let model_info = ModelInfo {
        name: display_name,
        model_id: server_config.model.clone(),
        quant: server_config.quant.clone(),
        backend: server_config.backend.clone(),
        gpu_type: discovery::detect_gpu_type(&backend_path),
        context_length: bench_config.ctx_override.or(server_config.context_length),
        gpu_layers: None,
    };

    let report = BenchReport {
        model_info,
        config: BenchConfig {
            pp_sizes: bench_config.pp_sizes.clone(),
            tg_sizes: bench_config.tg_sizes.clone(),
            runs: bench_config.runs,
            warmup: bench_config.warmup,
            ctx_override: bench_config.ctx_override,
            batch_sizes: bench_config.batch_sizes.clone(),
            ubatch_sizes: bench_config.ubatch_sizes.clone(),
            kv_cache_type: bench_config.kv_cache_type.clone(),
            depth: bench_config.depth.clone(),
            flash_attn: bench_config.flash_attn,
        },
        summaries,
        load_time_ms: 0.0,
        vram: crate::gpu::query_vram(),
    };

    // Stream the full report to the client via the progress sink. The frontend
    // uses this to render the header card (model / backend / GPU / VRAM) plus
    // the per-test results table — so we serialize the whole report, not just
    // `summaries`.
    if let Ok(report_json) = serde_json::to_string(&report) {
        progress.result(&report_json);
    }

    Ok(report)
}

/// Resolve the on-disk GGUF path for a model config.
///
/// Falls back to the legacy `<db_dir>/models/` location if the configured
/// `models_dir` doesn't hold the file.
fn resolve_model_path(
    config: &Config,
    db_dir: &std::path::Path,
    conn: &rusqlite::Connection,
    model_configs: &std::collections::HashMap<String, crate::config::ModelConfig>,
    resolved_id: &str,
) -> Result<std::path::PathBuf> {
    let mc = model_configs
        .get(resolved_id)
        .with_context(|| format!("Model config '{}' not found", resolved_id))?;
    let rec_id = mc.db_id.context("Model config has no db_id")?;
    let record = crate::db::queries::get_model_config(conn, rec_id)?
        .with_context(|| format!("Model config record (id={}) not found in database", rec_id))?;
    let files = crate::db::queries::get_model_files(conn, record.id)?;
    let model_file = files
        .into_iter()
        .find(|f| f.filename.ends_with(".gguf"))
        .context("No .gguf model file found for this config")?;

    let model_data_dir = config.models_dir()?;
    let candidate = model_data_dir
        .join(&record.repo_id)
        .join(&model_file.filename);
    if candidate.exists() {
        return Ok(candidate);
    }

    let legacy = db_dir.join("models");
    let legacy_candidate = legacy.join(&record.repo_id).join(&model_file.filename);
    if legacy_candidate.exists() {
        return Ok(legacy_candidate);
    }

    bail!(
        "Model file not found: {} (searched {:?} and {:?})",
        model_file.filename,
        candidate,
        legacy_candidate
    )
}
