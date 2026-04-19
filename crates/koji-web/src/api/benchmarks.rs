//! Benchmark API endpoints.
//!
//! Provides REST endpoints for triggering llama-bench benchmarks,
//! streaming progress via SSE, and managing benchmark history.

use axum::response::sse::{Event, KeepAlive};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Sse},
    Json,
};
use futures_util::Stream;
use serde_json::json;
use std::sync::Arc;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::gpu::query_vram;
use crate::jobs::{JobEvent, JobManager, JobKind, JobStatus};
use crate::server::AppState;

// ── Request/Response DTOs ─────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct BenchmarkRunRequest {
    pub model_id: String,
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
}

#[derive(Debug, Serialize)]
pub struct BenchmarkRunResponse {
    pub job_id: String,
}

#[derive(Debug, Serialize)]
pub struct BenchmarkHistoryEntry {
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
            ).into_response();
        }
    };

    // Submit a benchmark job
    let job = match jobs.submit(JobKind::Benchmark, None).await {
        Ok(j) => j,
        Err(_) => {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "Another job is already running"})),
            ).into_response();
        }
    };

    let job_id = job.id.clone();
    let req_clone = req.clone();
    let config_path = state.config_path.clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_benchmark_inner(jobs.clone(), &job, &req_clone, config_path).await {
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string())).await;
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
    use koji_core::bench::llama_bench::{self, LlamaBenchConfig};

    // Load config - clone config_dir for the blocking task
    let config_dir = config_path
        .as_ref()
        .and_then(|p| p.parent())
        .context("Cannot determine config directory")?
        .to_path_buf();

    let config = tokio::task::spawn_blocking(move || {
        koji_core::config::Config::load_from(&config_dir)
    })
    .await??;

    // Create progress sink adapter (same pattern as backend install)
    let job_clone = job.clone();
    let jobs_clone = jobs.clone();
    struct BenchProgressSink {
        job: Arc<crate::jobs::Job>,
        jobs: Arc<JobManager>,
    }
    impl koji_core::backends::ProgressSink for BenchProgressSink {
        fn log(&self, line: &str) {
            let job = self.job.clone();
            let jobs = self.jobs.clone();
            let line = line.to_string();
            tokio::spawn(async move {
                jobs.append_log(&job, line).await;
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
    };

    // Run benchmark
    let report = llama_bench::run_llama_bench(&config, &req.model_id, &bench_config, &sink).await?;

    // Store results in database
    let db_dir = koji_core::config::Config::config_dir()?;
    let koji_core::db::OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;

    // Get model display name from config
    let model_configs = koji_core::db::load_model_configs(&conn)?;
    let display_name = model_configs.get(&req.model_id)
        .and_then(|mc| mc.display_name.clone());

    // Serialize results to JSON string for storage
    let results_json = serde_json::to_string(&report.summaries)
        .context("Failed to serialize benchmark results")?;
    let pp_sizes_json = serde_json::to_string(&req.pp_sizes)
        .context("Failed to serialize pp_sizes")?;
    let tg_sizes_json = serde_json::to_string(&req.tg_sizes)
        .context("Failed to serialize tg_sizes")?;
    let threads_json = req.threads.as_ref()
        .map(|t| serde_json::to_string(t))
        .transpose()
        .context("Failed to serialize threads")?;

    // Get VRAM info
    let vram = query_vram();

    // Insert into database
    let _id = koji_core::db::queries::insert_benchmark(
        &conn,
        &req.model_id,
        display_name.as_deref(),
        report.model_info.quant.as_deref(),
        &report.model_info.backend,
        "llama_bench",
        &pp_sizes_json,
        &tg_sizes_json,
        threads_json.as_deref(),
        req.ngl_range.as_deref(),
        req.runs,
        req.warmup,
        &results_json,
        Some(report.load_time_ms),
        vram.as_ref().map(|v| v.used_mib as i64),
        vram.as_ref().map(|v| v.total_mib as i64),
        0.0, // duration tracked by job system
        "success",
    )?;

    Ok(())
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
            ).into_response();
        }
    };

    let job = match jobs.get(&job_id).await {
        Some(j) => j,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Job not found"})),
            ).into_response();
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

    (StatusCode::OK, Json(serde_json::json!({
        "job_id": job_id,
        "status": status,
        "error": error,
        "log_lines": log_lines,
    }))).into_response()
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

    // Snapshot + subscribe: take both under the same lock to avoid race
    let (head, tail, dropped, status, _finished_at, error) = {
        let (state, log_head, log_tail) =
            tokio::join!(job.state.read(), job.log_head.read(), job.log_tail.read());
        (
            log_head.iter().cloned().collect::<Vec<_>>(),
            log_tail.iter().cloned().collect::<Vec<_>>(),
            job.log_dropped.load(std::sync::atomic::Ordering::Relaxed),
            state.status,
            state.finished_at,
            state.error.clone(),
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
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // Emit dropped marker
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

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ── Handler: List benchmark history ───────────────────────────────────

pub async fn list_benchmark_history(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let db_dir = match koji_core::config::Config::config_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
    };

    let entries = match tokio::task::spawn_blocking(move || {
        let koji_core::db::OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
        koji_core::db::queries::list_benchmarks(&conn)
    }).await {
        Ok(Ok(entries)) => entries,
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
    };

    let history: Vec<BenchmarkHistoryEntry> = entries.into_iter().map(|e| {
        let pp_sizes: Vec<u32> = serde_json::from_str(&e.pp_sizes).unwrap_or_default();
        let tg_sizes: Vec<u32> = serde_json::from_str(&e.tg_sizes).unwrap_or_default();
        BenchmarkHistoryEntry {
            id: e.id,
            created_at: e.created_at,
            model_id: e.model_id,
            display_name: e.display_name,
            quant: e.quant,
            backend: e.backend,
            pp_sizes,
            tg_sizes,
            runs: e.runs,
            results_count: serde_json::from_str::<Vec<serde_json::Value>>(&e.results).map(|v| v.len()).unwrap_or(0),
            status: e.status,
        }
    }).collect();

    Json(history).into_response()
}

// ── Handler: Delete benchmark history entry ───────────────────────────

pub async fn delete_benchmark(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    let db_dir = match koji_core::config::Config::config_dir() {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ).into_response();
        }
    };

    match tokio::task::spawn_blocking(move || {
        let koji_core::db::OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
        koji_core::db::queries::delete_benchmark(&conn, id)
    }).await {
        Ok(Ok(())) => Json(serde_json::json!({"ok": true})).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ).into_response(),
    }
}
