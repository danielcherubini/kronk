use super::*;

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
    let proxy_base_url = state.proxy_base_url.clone();
    let client = state.client.clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_benchmark_inner(
            jobs.clone(),
            &job,
            &req_clone,
            config_path,
            proxy_base_url,
            client,
        )
        .await
        {
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string()))
                .await;
        } else {
            jobs.finish(&job, JobStatus::Succeeded, None).await;
        }
    });

    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

pub async fn run_benchmark_inner(
    jobs: Arc<JobManager>,
    job: &Arc<crate::jobs::Job>,
    req: &BenchmarkRunRequest,
    config_path: Option<std::path::PathBuf>,
    proxy_base_url: String,
    client: reqwest::Client,
) -> Result<()> {
    use tama_core::bench::llama_bench::{self, LlamaBenchConfig};

    // Unload any active server for this model before running the benchmark.
    // This prevents GPU memory conflicts when the model is already loaded.
    unload_model_before_benchmark(&client, &proxy_base_url, &req.model_id, &job.id).await;

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

    tracing::info!(
        job_id = %job.id,
        model_id = %req.model_id,
        backend = ?req.backend_name,
        pp_sizes = ?req.pp_sizes,
        tg_sizes = ?req.tg_sizes,
        runs = req.runs,
        "Starting llama-bench benchmark",
    );

    // Run benchmark
    let report = llama_bench::run_llama_bench(
        &config,
        &req.model_id,
        req.quant.as_deref(),
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

    tracing::info!(
        job_id = %job.id,
        display_name = ?display_name,
        backend = %report.model_info.backend,
        entries = report.summaries.len(),
        "llama-bench benchmark completed",
    );

    Ok(())
}

// ── Shared helpers ────────────────────────────────────────────────────

/// Best-effort unload of any active proxy server for the given model.
/// Used before benchmarks to prevent GPU memory conflicts. Errors are logged
/// at debug level and never block the benchmark — the model may not be loaded,
/// or the proxy may be unreachable.
pub(super) async fn unload_model_before_benchmark(
    client: &reqwest::Client,
    proxy_base_url: &str,
    model_id: &str,
    job_id: &str,
) {
    let unload_url = format!("{}/tama/v1/models/{}/unload", proxy_base_url, model_id);
    match client.post(&unload_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(job_id = %job_id, "Unloaded active model before benchmark");
        }
        Ok(resp) => {
            tracing::debug!(
                job_id = %job_id,
                status = %resp.status(),
                "Model unload returned non-success (model may not be loaded)"
            );
        }
        Err(e) => {
            tracing::debug!(
                job_id = %job_id,
                error = %e,
                "Failed to call model unload (may not be reachable)"
            );
        }
    }
}

/// Resolve a model's file path from config and database.
/// `quant_override` takes priority over `mc.quant` when resolving the target file.
pub(super) fn resolve_model_path(
    config: &tama_core::config::Config,
    db_dir: &std::path::Path,
    conn: &rusqlite::Connection,
    model_configs: &std::collections::HashMap<String, tama_core::config::ModelConfig>,
    resolved_id: &str,
    quant_override: Option<&str>,
) -> Result<std::path::PathBuf> {
    let mc = model_configs
        .get(resolved_id)
        .with_context(|| format!("Model config '{}' not found", resolved_id))?;
    let rec_id = mc.db_id.context("Model config has no db_id")?;
    let record = tama_core::db::queries::get_model_config(conn, rec_id)?
        .with_context(|| format!("Model config record (id={}) not found in database", rec_id))?;
    let files = tama_core::db::queries::get_model_files(conn, record.id)?;

    // Resolve the target filename: prefer quant_override, then mc.quant from config,
    // falling back to the first .gguf if quants map is empty (legacy configs).
    let first_gguf = files
        .iter()
        .find(|f| f.filename.ends_with(".gguf"))
        .map(|f| f.filename.clone());

    let target_filename = quant_override
        .or(mc.quant.as_deref())
        .and_then(|quant_label| mc.quants.get(quant_label).map(|qe| qe.file.clone()))
        .or(first_gguf)
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
