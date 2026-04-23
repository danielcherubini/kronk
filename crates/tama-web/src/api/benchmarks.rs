//! Benchmark API endpoints.
//!
//! Provides REST endpoints for triggering llama-bench benchmarks,
//! streaming progress via SSE, and managing benchmark history.

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
    #[serde(default)]
    pub backend_name: Option<String>,
    pub spec_types: Vec<SpecType>,
    #[serde(default)]
    pub draft_max_values: Vec<u32>,
    #[serde(default)]
    pub ngram_n_values: Vec<u32>,
    #[serde(default)]
    pub ngram_m_values: Vec<u32>,
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

// ── Handler: Submit benchmark job ─────────────────────────────────────

pub async fn run_benchmark(
    State(state): State<Arc<AppState>>,
    Json(req): Json<BenchmarkRunRequest>,
) -> impl IntoResponse {
    let jobs = match &state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Job manager not available"})),
            )
                .into_response();
        }
    };

    // Submit a benchmark job
    let job = match jobs.submit(JobKind::Benchmark, None).await {
        Ok(j) => j,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "Another job is already running"})),
            )
                .into_response();
        }
    };

    let job_id = job.id.clone();
    let req_clone = req.clone();
    let config_path = state.config_path.clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_benchmark_inner(jobs.clone(), &job, &req_clone, config_path).await {
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string()))
                .await;
        } else {
            jobs.finish(&job, JobStatus::Succeeded, None).await;
        }
    });

    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

async fn run_benchmark_inner(
    jobs: Arc<JobManager>,
    job: &Arc<crate::jobs::Job>,
    req: &BenchmarkRunRequest,
    config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    use tama_core::bench::llama_bench::{self, LlamaBenchConfig};

    // Load config - clone config_dir for the blocking task
    let config_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .context("Cannot determine config directory")?
        .to_path_buf();

    let config =
        tokio::task::spawn_blocking(move || tama_core::config::Config::load_from(&config_dir))
            .await??;

    // Create progress sink adapter (same pattern as backend install)
    let job_clone = job.clone();
    let jobs_clone = jobs.clone();
    struct BenchProgressSink {
        job: Arc<crate::jobs::Job>,
        jobs: Arc<JobManager>,
    }
    impl tama_core::backends::ProgressSink for BenchProgressSink {
        fn log(&self, line: &str) {
            let job = self.job.clone();
            let jobs = self.jobs.clone();
            let line = line.to_string();
            tokio::spawn(async move {
                jobs.append_log(&job, line).await;
            });
        }

        fn result(&self, json: &str) {
            let job = self.job.clone();
            let data = json.to_string();
            tracing::info!("BenchmarkProgressSink::result called, job_id={}", job.id);

            // Broadcast over the shared job event channel so live SSE
            // subscribers get the result immediately. Send synchronously —
            // `broadcast::Sender::send` is non-blocking.
            if let Err(e) = job.log_tx.send(JobEvent::Result(data.clone())) {
                tracing::warn!("Failed to broadcast result for job {}: {}", job.id, e);
            }

            tokio::spawn(async move {
                // Also store in job state so late subscribers can pick it
                // up on replay and the REST endpoint can return it.
                let mut results = job.benchmark_results.write().await;
                *results = Some(data);
                tracing::info!("Stored benchmark results in job state");
            });
        }
    }

    let sink = BenchProgressSink {
        job: job_clone.clone(),
        jobs: jobs_clone.clone(),
    };

    // Build llama-bench config
    let bench_config = LlamaBenchConfig {
        pp_sizes: req.pp_sizes.clone(),
        tg_sizes: req.tg_sizes.clone(),
        runs: req.runs,
        warmup: req.warmup,
        threads: req.threads.clone(),
        ngl_range: req.ngl_range.clone(),
        ctx_override: req.ctx_override,
        batch_sizes: req.batch_sizes.clone(),
        ubatch_sizes: req.ubatch_sizes.clone(),
        kv_cache_type: req.kv_cache_type.clone(),
        depth: req.depth.clone(),
        flash_attn: req.flash_attn,
    };

    // Run benchmark
    let report = llama_bench::run_llama_bench(
        &config,
        &req.model_id,
        req.backend_name.as_deref(),
        &bench_config,
        &sink,
    )
    .await?;

    // Store results in database
    let db_dir = tama_core::config::Config::config_dir()?;
    let tama_core::db::OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;

    // Get model display name from config. The request carries the db_id as a
    // string (e.g. "4") because that's what the model dropdown submits, so we
    // resolve it to the config key first — otherwise `.get("4")` never hits.
    let model_configs = tama_core::db::load_model_configs(&conn)?;
    let resolved_key = if let Ok(db_id) = req.model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| req.model_id.clone())
    } else {
        req.model_id.clone()
    };
    let display_name = model_configs.get(&resolved_key).and_then(|mc| {
        mc.display_name
            .clone()
            .or_else(|| mc.api_name.clone())
            .or_else(|| mc.model.clone())
    });

    // Serialize the full report for storage so history can reconstruct model
    // metadata (backend, GPU, VRAM, load time, batch/ubatch/KV cache choices),
    // not just the per-test summary rows.
    let results_json =
        serde_json::to_string(&report).context("Failed to serialize benchmark report")?;
    let pp_sizes_json =
        serde_json::to_string(&req.pp_sizes).context("Failed to serialize pp_sizes")?;
    let tg_sizes_json =
        serde_json::to_string(&req.tg_sizes).context("Failed to serialize tg_sizes")?;
    let threads_json = req
        .threads
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("Failed to serialize threads")?;

    // Get VRAM info
    let vram = query_vram();

    // Insert into database
    let _id = tama_core::db::queries::insert_benchmark(
        &conn,
        &tama_core::db::queries::BenchmarkInsertParams {
            model_id: &req.model_id,
            display_name: display_name.as_deref(),
            quant: report.model_info.quant.as_deref(),
            backend: &report.model_info.backend,
            engine: "llama_bench",
            pp_sizes_json: &pp_sizes_json,
            tg_sizes_json: &tg_sizes_json,
            threads_json: threads_json.as_deref(),
            ngl_range: req.ngl_range.as_deref(),
            runs: req.runs,
            warmup: req.warmup,
            results_json: &results_json,
            load_time_ms: Some(report.load_time_ms),
            vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
            vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
            duration_seconds: 0.0, // duration tracked by job system
            status: "success",
            benchmark_type: req.benchmark_type.as_deref(),
        },
    )?;

    Ok(())
}

// ── Handler: Submit spec benchmark job ────────────────────────────────

pub async fn run_spec_benchmark(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SpecBenchmarkRunRequest>,
) -> impl IntoResponse {
    let jobs = match &state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Job manager not available"})),
            )
                .into_response();
        }
    };

    // Validate spec_types is not empty
    if req.spec_types.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "spec_types must not be empty"})),
        )
            .into_response();
    }

    // Apply minimum guards
    let runs = req.runs.max(1);
    let gen_tokens = req.gen_tokens.max(1);

    // Build config to validate sweep dimensions
    let model_path = std::path::PathBuf::from("/tmp/validation_model.gguf");
    let validation_config = SpecBenchConfig {
        model_path,
        spec_types: req.spec_types.clone(),
        draft_max_values: req.draft_max_values.clone(),
        ngram_n_values: req.ngram_n_values.clone(),
        ngram_m_values: req.ngram_m_values.clone(),
        ngram_min_hits: req.ngram_min_hits,
        gen_tokens,
        runs,
        ngl: req.ngl,
        flash_attn: req.flash_attn,
    };

    // Validate sweep matrix would produce entries
    if let Err(e) = validate_spec_sweep(&validation_config) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response();
    }

    // Submit a benchmark job
    let job = match jobs.submit(JobKind::Benchmark, None).await {
        Ok(j) => j,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "Another job is already running"})),
            )
                .into_response();
        }
    };

    let job_id = job.id.clone();
    let req_clone = req.clone();
    let config_path = state.config_path.clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_spec_benchmark_inner(jobs.clone(), &job, &req_clone, config_path).await
        {
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string()))
                .await;
        } else {
            jobs.finish(&job, JobStatus::Succeeded, None).await;
        }
    });

    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

/// Validate that the spec sweep configuration would produce at least one entry.
fn validate_spec_sweep(config: &SpecBenchConfig) -> Result<()> {
    tama_core::bench::llama_cli_spec::validate_sweep_config(config)
}

async fn run_spec_benchmark_inner(
    jobs: Arc<JobManager>,
    job: &Arc<crate::jobs::Job>,
    req: &SpecBenchmarkRunRequest,
    config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    use tama_core::bench::llama_cli_spec;

    // Load config
    let config_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .context("Cannot determine config directory")?
        .to_path_buf();

    let config =
        tokio::task::spawn_blocking(move || tama_core::config::Config::load_from(&config_dir))
            .await??;

    // Resolve model path (same pattern as llama_bench)
    let db_dir = tama_core::config::Config::config_dir()?;
    let tama_core::db::OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;
    let model_configs = tama_core::db::load_model_configs(&conn)?;

    // If model_id is an integer db_id, resolve it to the config key first.
    let resolved_id = if let Ok(db_id) = req.model_id.parse::<i64>() {
        model_configs
            .iter()
            .find(|(_, mc)| mc.db_id == Some(db_id))
            .map(|(key, _)| key.as_str())
            .unwrap_or(&req.model_id)
    } else {
        &req.model_id
    };

    let (server_config, _) = config
        .resolve_server(&model_configs, resolved_id)
        .context("Failed to resolve server config for benchmark")?;

    let model_path = resolve_model_path(&config, &db_dir, &conn, &model_configs, resolved_id)?;

    // Get model display name from config
    let display_name = model_configs.get(resolved_id).and_then(|mc| {
        mc.display_name
            .clone()
            .or_else(|| mc.api_name.clone())
            .or_else(|| mc.model.clone())
    });

    // Apply minimum guards
    let runs = req.runs.max(1);
    let gen_tokens = req.gen_tokens.max(1);

    // Build SpecBenchConfig
    let spec_config = SpecBenchConfig {
        model_path: model_path.clone(),
        spec_types: req.spec_types.clone(),
        draft_max_values: req.draft_max_values.clone(),
        ngram_n_values: req.ngram_n_values.clone(),
        ngram_m_values: req.ngram_m_values.clone(),
        ngram_min_hits: req.ngram_min_hits,
        gen_tokens,
        runs,
        ngl: req.ngl,
        flash_attn: req.flash_attn,
    };

    // Create progress sink adapter (same pattern as existing benchmark handler)
    let job_clone = job.clone();
    let jobs_clone = jobs.clone();
    struct SpecBenchProgressSink {
        job: Arc<crate::jobs::Job>,
        jobs: Arc<JobManager>,
    }
    impl tama_core::backends::ProgressSink for SpecBenchProgressSink {
        fn log(&self, line: &str) {
            let job = self.job.clone();
            let jobs = self.jobs.clone();
            let line = line.to_string();
            tokio::spawn(async move {
                jobs.append_log(&job, line).await;
            });
        }

        fn result(&self, json: &str) {
            let job = self.job.clone();
            let data = json.to_string();
            tracing::info!("SpecBenchProgressSink::result called, job_id={}", job.id);

            // Broadcast over the shared job event channel so live SSE
            // subscribers get the result immediately.
            if let Err(e) = job.log_tx.send(JobEvent::Result(data.clone())) {
                tracing::warn!("Failed to broadcast result for job {}: {}", job.id, e);
            }

            tokio::spawn(async move {
                let mut results = job.benchmark_results.write().await;
                *results = Some(data);
                tracing::info!("Stored spec benchmark results in job state");
            });
        }
    }

    let sink = SpecBenchProgressSink {
        job: job_clone.clone(),
        jobs: jobs_clone.clone(),
    };

    // Resolve backend path for llama-cli discovery
    let target_backend = req
        .backend_name
        .as_deref()
        .unwrap_or(&server_config.backend);
    let backend_path = {
        let conn = tama_core::config::Config::open_db();
        config.resolve_backend_path(target_backend, &conn)?
    };

    // Discover llama-cli binary
    let cli_binary = llama_cli_spec::find_llama_cli(&backend_path).context(format!(
        "llama-cli not found for backend '{}'. Install llama.cpp from source or set LLAMA_CLI_PATH",
        target_backend
    ))?;

    // Run spec benchmark
    let result = llama_cli_spec::run_spec_bench(&spec_config, Some(cli_binary), &sink).await?;

    // Store results in database
    let db_dir = tama_core::config::Config::config_dir()?;
    let tama_core::db::OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;

    // Serialize the full result for storage
    let results_json =
        serde_json::to_string(&result).context("Failed to serialize spec benchmark result")?;
    let pp_sizes_json = "[]";
    let tg_sizes_json =
        serde_json::to_string(&[gen_tokens]).context("Failed to serialize gen_tokens")?;

    // Get VRAM info
    let vram = query_vram();

    // Insert into database
    let _id = tama_core::db::queries::insert_benchmark(
        &conn,
        &tama_core::db::queries::BenchmarkInsertParams {
            model_id: &req.model_id,
            display_name: display_name.as_deref(),
            quant: None,
            backend: target_backend.to_string().as_str(),
            engine: "llama_cli_spec",
            pp_sizes_json,
            tg_sizes_json: &tg_sizes_json,
            threads_json: None,
            ngl_range: None,
            runs,
            warmup: 0,
            results_json: &results_json,
            load_time_ms: None,
            vram_used_mib: vram.as_ref().map(|v| v.used_mib as i64),
            vram_total_mib: vram.as_ref().map(|v| v.total_mib as i64),
            duration_seconds: 0.0,
            status: "success",
            benchmark_type: req.benchmark_type.as_deref(),
        },
    )?;

    Ok(())
}

/// Resolve a model's file path from config and database.
fn resolve_model_path(
    config: &tama_core::config::Config,
    db_dir: &std::path::Path,
    conn: &rusqlite::Connection,
    model_configs: &std::collections::HashMap<String, tama_core::config::ModelConfig>,
    resolved_id: &str,
) -> Result<std::path::PathBuf> {
    let mc = model_configs
        .get(resolved_id)
        .with_context(|| format!("Model config '{}' not found", resolved_id))?;
    let rec_id = mc.db_id.context("Model config has no db_id")?;
    let record = tama_core::db::queries::get_model_config(conn, rec_id)?
        .with_context(|| format!("Model config record (id={}) not found in database", rec_id))?;
    let files = tama_core::db::queries::get_model_files(conn, record.id)?;

    // Resolve the target filename: use the selected quant's file from mc.quants,
    // falling back to the first .gguf if quants map is empty (legacy configs).
    let first_gguf = files
        .iter()
        .find(|f| f.filename.ends_with(".gguf"))
        .map(|f| f.filename.clone());

    let target_filename = if let Some(ref quant_label) = mc.quant {
        mc.quants
            .get(quant_label)
            .map(|qe| qe.file.clone())
            .or(first_gguf.clone())
    } else {
        first_gguf
    }
    .context("No model file found for this config")?;

    let model_file = files
        .into_iter()
        .find(|f| f.filename == target_filename)
        .context("Resolved model file not found in database")?;

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

    anyhow::bail!(
        "Model file not found: {} (searched {:?} and {:?})",
        model_file.filename,
        candidate,
        legacy_candidate
    )
}

// ── Handler: Get benchmark result ─────────────────────────────────────

pub async fn get_benchmark_result(
    State(_state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let jobs = match &_state.jobs {
        Some(j) => j.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "Job manager not available"})),
            )
                .into_response();
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Job not found"})),
            )
                .into_response();
        }
    };

    let state = job.state.read().await;
    let error = state.error.clone();
    let status = format!("{:?}", state.status);
    drop(state);

    // Read log lines for context
    let log_lines: Vec<String> = {
        let head = job.log_head.read().await;
        let tail = job.log_tail.read().await;
        let mut lines: Vec<String> = head.iter().cloned().collect();
        lines.extend(tail.iter().cloned());
        lines
    };

    // Get benchmark results if available
    let benchmark_results = {
        let results = job.benchmark_results.read().await;
        let cloned = results.clone();
        tracing::info!(
            "get_benchmark_result: benchmark_results={:?}",
            cloned.is_some()
        );
        cloned
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "job_id": job_id,
            "status": status,
            "error": error,
            "log_lines": log_lines,
            "benchmark_results": benchmark_results,
        })),
    )
        .into_response()
}

// ── Handler: SSE events for benchmark progress ────────────────────────

pub async fn benchmark_events(
    State(_state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let jobs = match &_state.jobs {
        Some(j) => j.clone(),
        None => {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j,
        None => {
            return Err(StatusCode::NOT_FOUND);
        }
    };

    let mut rx = job.log_tx.subscribe();

    // Snapshot + subscribe: take everything under overlapping locks to avoid races.
    let (head, tail, dropped, status, _finished_at, error, stored_result) = {
        let (state, log_head, log_tail, bench_results) = tokio::join!(
            job.state.read(),
            job.log_head.read(),
            job.log_tail.read(),
            job.benchmark_results.read()
        );
        (
            log_head.iter().cloned().collect::<Vec<_>>(),
            log_tail.iter().cloned().collect::<Vec<_>>(),
            job.log_dropped.load(std::sync::atomic::Ordering::Relaxed),
            state.status,
            state.finished_at,
            state.error.clone(),
            bench_results.clone(),
        )
    };

    let stream = async_stream::stream! {
        // Replay head
        for line in head {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line}))?);
        }

        // Emit skipped marker if dropped > 0
        if dropped > 0 && !tail.is_empty() {
            yield Ok(Event::default().event("log")
                .json_data(json!({ "line": format!("[... {} lines skipped ...]", dropped)}))?);
        }

        // Replay tail
        for line in tail {
            yield Ok(Event::default().event("log").json_data(json!({ "line": line}))?);
        }

        // Replay stored benchmark results (for late subscribers)
        if let Some(ref results_json) = stored_result {
            yield Ok(Event::default().event("result")
                .json_data(json!({ "results": results_json}))?);
        }

        // Emit final status if terminal
        if status != JobStatus::Running {
            yield Ok(Event::default().event("status")
                .json_data(json!({ "status": status}))?);
            if let Some(err) = error {
                yield Ok(Event::default().event("error")
                    .json_data(json!({ "error": err}))?);
            }
            return; // Close after terminal job
        }

        // Live stream
        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(JobEvent::Log(line)) => {
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": line}))?);
                        }
                        Ok(JobEvent::Status(s)) => {
                            yield Ok(Event::default().event("status")
                                .json_data(json!({ "status": s}))?);
                            if s != JobStatus::Running {
                                return; // Close on terminal status
                            }
                        }
                        Ok(JobEvent::Result(results_json)) => {
                            yield Ok(Event::default().event("result")
                                .json_data(json!({ "results": results_json}))?);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": format!("[{} lines dropped]", n)}))?);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return;
                        }
                    }
                }
            }
        }
    };

    // No keep-alive: the stream ends naturally when the job completes,
    // and we close the EventSource on the client side to prevent reconnection loops.
    Ok(Sse::new(stream))
}

// ── Handler: List benchmark history ───────────────────────────────────

pub async fn list_benchmark_history(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let db_dir = match tama_core::config::Config::config_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let entries = match tokio::task::spawn_blocking(move || {
        let tama_core::db::OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;
        tama_core::db::queries::list_benchmarks(&conn)
    })
    .await
    {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let history: Vec<BenchmarkHistoryEntry> = entries
        .into_iter()
        .map(|e| {
            let pp_sizes: Vec<u32> = serde_json::from_str(&e.pp_sizes).unwrap_or_default();
            let tg_sizes: Vec<u32> = serde_json::from_str(&e.tg_sizes).unwrap_or_default();
            // `results_json` may be either the full BenchReport (new rows) or a
            // plain summaries array (legacy rows). Extract the summaries array
            // so the frontend only has one shape to deal with.
            let raw: serde_json::Value = serde_json::from_str(&e.results).unwrap_or_else(|err| {
                tracing::warn!("Failed to parse results for benchmark id={}: {}", e.id, err);
                serde_json::Value::Null
            });
            let summaries = match raw.get("summaries") {
                Some(v) if v.is_array() => v.clone(),
                _ if raw.is_array() => raw,
                _ => serde_json::Value::Array(vec![]),
            };
            let results_count = summaries.as_array().map(|a| a.len()).unwrap_or(0);
            BenchmarkHistoryEntry {
                id: e.id,
                created_at: e.created_at,
                model_id: e.model_id,
                display_name: e.display_name,
                quant: e.quant,
                backend: e.backend,
                engine: Some(e.engine),
                benchmark_type: e.benchmark_type,
                pp_sizes,
                tg_sizes,
                runs: e.runs,
                results_count,
                status: e.status,
                results: summaries,
            }
        })
        .collect();

    Json(history).into_response()
}

// ── Handler: Delete benchmark history entry ───────────────────────────

pub async fn delete_benchmark(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let db_dir = match tama_core::config::Config::config_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    match tokio::task::spawn_blocking(move || {
        let tama_core::db::OpenResult { conn, .. } = tama_core::db::open(&db_dir)?;
        tama_core::db::queries::delete_benchmark(&conn, id)
    })
    .await
    {
        Ok(Ok(())) => Json(serde_json::json!({"ok": true})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
