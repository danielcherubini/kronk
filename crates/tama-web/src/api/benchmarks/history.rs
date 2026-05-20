use super::*;

// ── Handler: Get benchmark result ─────────────────────────────────────

pub async fn get_benchmark_result(
    State(_state): State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let jobs = match &_state.web_jobs {
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
    State(_state): State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let jobs = match &_state.web_jobs {
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

pub async fn list_benchmark_history(State(_state): State<Arc<ProxyState>>) -> impl IntoResponse {
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

            // `results_json` may be:
            // - full BenchReport with "summaries" key (llama-bench)
            // - SpecBenchResult with "entries" key (spec decode)
            // - plain summaries array (legacy rows)
            let raw: serde_json::Value = serde_json::from_str(&e.results).unwrap_or_else(|err| {
                tracing::warn!("Failed to parse results for benchmark id={}: {}", e.id, err);
                serde_json::Value::Null
            });
            let summaries = match raw.get("summaries") {
                Some(v) if v.is_array() => v.clone(),
                // SpecBenchResult: convert entries to llama-bench summary format
                // so the frontend can render them. Maps:
                //   tg_ts_mean → tg_mean, tg_ts_stddev → tg_stddev,
                //   spec_type + draft_max → extra fields for display
                Some(entries) if entries.is_array() && raw.get("baseline_tg_ts").is_some() => {
                    let _baseline = raw["baseline_tg_ts"].as_f64().unwrap_or(0.0);
                    let mut summaries = serde_json::Value::Array(vec![]);
                    for entry in entries.as_array().unwrap() {
                        let tg_mean = entry["tg_ts_mean"].as_f64().unwrap_or(0.0);
                        let stddev = entry["tg_ts_stddev"].as_f64().unwrap_or(0.0);
                        let status = entry["status"].as_str().unwrap_or("failed");
                        let delta_pct = entry["delta_pct"].as_f64().unwrap_or(0.0);
                        let spec_type = entry["spec_type"].as_str().unwrap_or("");
                        let draft_max = entry["draft_max"].as_u64().unwrap_or(0);
                        let ngram_n = entry["ngram_n"].as_u64();
                        let ngram_m = entry["ngram_m"].as_u64();

                        let mut summary = serde_json::Map::new();
                        // Frontend expects these fields for rendering.
                        summary.insert("prompt_tokens".to_string(), serde_json::json!(0u64));
                        summary.insert(
                            "gen_tokens".to_string(),
                            serde_json::json!(tg_sizes.first().copied().unwrap_or(0)),
                        );
                        summary.insert("tg_mean".to_string(), serde_json::json!(tg_mean));
                        summary.insert("tg_stddev".to_string(), serde_json::json!(stddev));
                        // Keep spec-specific fields for display.
                        summary.insert("spec_type".to_string(), serde_json::json!(spec_type));
                        summary.insert("draft_max".to_string(), serde_json::json!(draft_max));
                        if let Some(n) = ngram_n {
                            summary.insert("ngram_n".to_string(), serde_json::json!(n));
                        }
                        if let Some(m) = ngram_m {
                            summary.insert("ngram_m".to_string(), serde_json::json!(m));
                        }
                        if delta_pct != 0.0 {
                            summary.insert("delta_pct".to_string(), serde_json::json!(delta_pct));
                            summary.insert(
                                "delta_pct_display".to_string(),
                                serde_json::json!(format!("{:+.1}%", delta_pct)),
                            );
                        }
                        summary.insert("status".to_string(), serde_json::json!(status));
                        summaries
                            .as_array_mut()
                            .unwrap()
                            .push(serde_json::Value::Object(summary));
                    }
                    summaries
                }
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
    State(_state): State<Arc<ProxyState>>,
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
