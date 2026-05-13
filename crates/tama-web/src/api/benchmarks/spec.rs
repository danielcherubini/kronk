use super::*;
use crate::api::benchmarks::run::{resolve_model_path, unload_model_before_benchmark};

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
        ngram_min_values: req.ngram_min_values.clone(),
        ngram_max_values: req.ngram_max_values.clone(),
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
    let proxy_base_url = state.proxy_base_url.clone();
    let client = state.client.clone();

    // Spawn the benchmark in the background
    tokio::spawn(async move {
        if let Err(e) = run_spec_benchmark_inner(
            jobs.clone(),
            &job,
            &req_clone,
            config_path,
            proxy_base_url,
            client,
        )
        .await
        {
            tracing::error!(job_id = %job.id, error = %e, "Spec benchmark failed");
            jobs.finish(&job, JobStatus::Failed, Some(e.to_string()))
                .await;
        } else {
            jobs.finish(&job, JobStatus::Succeeded, None).await;
        }
    });

    (StatusCode::ACCEPTED, Json(BenchmarkRunResponse { job_id })).into_response()
}

/// Validate that the spec sweep configuration would produce at least one entry.
pub fn validate_spec_sweep(config: &SpecBenchConfig) -> Result<()> {
    tama_core::bench::llama_cli_spec::validate_sweep_config(config)
}

pub async fn run_spec_benchmark_inner(
    jobs: Arc<JobManager>,
    job: &Arc<crate::jobs::Job>,
    req: &SpecBenchmarkRunRequest,
    config_path: Option<std::path::PathBuf>,
    proxy_base_url: String,
    client: reqwest::Client,
) -> Result<()> {
    use tama_core::bench::llama_cli_spec;

    // Unload any active server for this model before running the benchmark.
    unload_model_before_benchmark(&client, &proxy_base_url, &req.model_id, &job.id).await;

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

    let model_path = resolve_model_path(
        &config,
        &db_dir,
        &conn,
        &model_configs,
        resolved_id,
        req.quant.as_deref(),
    )?;

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
        ngram_min_values: req.ngram_min_values.clone(),
        ngram_max_values: req.ngram_max_values.clone(),
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

    let sink = Arc::new(SpecBenchProgressSink {
        job: job_clone.clone(),
        jobs: jobs_clone.clone(),
    });

    // Resolve backend path for llama-server discovery
    let target_backend = req
        .backend_name
        .as_deref()
        .unwrap_or(&server_config.backend);
    let manager = tama_core::backends::BackendManager::open(&db_dir)?;
    let backend_path =
        config.resolve_backend_path(target_backend, req.gpu_variant.as_deref(), &manager)?;

    // Discover llama-server binary
    // The resolved path may be a file (llama-server) rather than the backend directory.
    // Use its parent as the search base for llama-server.
    let backend_dir = backend_path.parent().unwrap_or(&backend_path);
    tracing::info!(job_id = %job.id, backend_dir = %backend_dir.display(), "Resolving llama-server for benchmark");
    let server_binary = llama_cli_spec::find_llama_server(backend_dir).context(format!(
        "llama-server not found for backend '{}'. Install llama.cpp from source or set LLAMA_SERVER_PATH",
        target_backend
    ))?;
    tracing::info!(
        job_id = %job.id,
        model = %resolved_id,
        backend = %target_backend,
        spec_types = ?req.spec_types,
        draft_max = ?req.draft_max_values,
        ngram_n = ?req.ngram_n_values,
        ngram_m = ?req.ngram_m_values,
        gen_tokens = gen_tokens,
        runs = runs,
        "Starting speculative decoding benchmark",
    );
    tracing::info!(job_id = %job.id, llama_server = %server_binary.display(), "Using llama-server binary");

    // Run spec benchmark
    let result =
        llama_cli_spec::run_spec_bench(&spec_config, Some(server_binary), sink.clone()).await?;

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
            quant: req.quant.as_deref(),
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

    tracing::info!(
        job_id = %job.id,
        entries = result.entries.len(),
        baseline_tg_ts = result.baseline_tg_ts,
        "Speculative decoding benchmark completed",
    );

    Ok(())
}
