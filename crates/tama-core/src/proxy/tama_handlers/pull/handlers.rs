use axum::{
    extract::{Path, State},
    response::{sse::Event, sse::KeepAlive, IntoResponse, Response, Sse},
    Json,
};
use futures_util::stream;
use reqwest::StatusCode;
use std::sync::Arc;

use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::tama_handlers::types::{max_concurrent_pulls, PullRequest};
use crate::proxy::ProxyState;

use super::enqueue_download;

/// Handle starting a pull job (Tama management API).
pub async fn handle_tama_pull_model(
    state: State<Arc<ProxyState>>,
    Json(request): Json<PullRequest>,
) -> Response {
    let repo_id = request.repo_id.clone();

    // Multi-quant path: when `quants` is non-empty, spawn one job per entry.
    if !request.quants.is_empty() {
        let max_pulls = max_concurrent_pulls();
        if request.quants.len() > max_pulls {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("Too many quants requested. Maximum is {}.", max_pulls)
                })),
            )
                .into_response();
        }

        // Fetch the HF listing once and validate every requested filename against it.
        let listing = match crate::models::pull::list_gguf_files(&repo_id).await {
            Ok(l) => l,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to fetch file list from HuggingFace: {}", e),
                            "type": "UpstreamError"
                        }
                    })),
                )
                    .into_response();
            }
        };
        let allowed_filenames: std::collections::HashSet<&str> =
            listing.files.iter().map(|f| f.filename.as_str()).collect();

        for spec in &request.quants {
            if !allowed_filenames.contains(spec.filename.as_str()) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!(
                                "Filename '{}' is not a valid GGUF file for repo '{}'",
                                spec.filename, repo_id
                            ),
                            "type": "ValidationError"
                        }
                    })),
                )
                    .into_response();
            }
        }

        // Reject if the request contains duplicate filenames — concurrent downloads
        // to the same dest path would corrupt the shared temp part files.
        {
            let mut seen = std::collections::HashSet::new();
            for spec in &request.quants {
                if !seen.insert(&spec.filename) {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!(
                                    "Duplicate filename '{}' in request",
                                    spec.filename
                                ),
                                "type": "ValidationError"
                            }
                        })),
                    )
                        .into_response();
                }
            }
        }

        let mut job_entries = Vec::with_capacity(request.quants.len());

        for spec in &request.quants {
            let job_id = format!("pull-{}", uuid::Uuid::new_v4().hyphenated());
            let pull_job = PullJob {
                job_id: job_id.clone(),
                repo_id: repo_id.clone(),
                filename: spec.filename.clone(),
                ..Default::default()
            };

            {
                let mut jobs = state.pull_jobs.write().await;
                jobs.insert(job_id.clone(), pull_job);
            }

            // Enqueue in the DB queue (best-effort — don't fail the pull if enqueue fails)
            let display_name = state
                .model_configs
                .read()
                .await
                .get(&format!(
                    "{}--{}",
                    repo_id.replace('/', "--"),
                    spec.quant.as_deref().unwrap_or("unknown")
                ))
                .and_then(|mc| mc.display_name.clone());
            let _ = enqueue_download(
                &state,
                job_id.clone(),
                repo_id.clone(),
                &spec.filename,
                display_name.as_deref(),
                spec.quant.as_deref(),
                spec.context_length,
            );

            job_entries.push(serde_json::json!({
                "job_id": job_id,
                "filename": spec.filename,
                "status": "pending"
            }));
        }

        return Json(serde_json::Value::Array(job_entries)).into_response();
    }

    // Legacy single-quant path.

    // Quant is required — if missing, fetch the available quants from HF and return them.
    let quant = match request.quant {
        Some(q) => q,
        None => {
            let available = match crate::models::pull::list_gguf_files(&repo_id).await {
                Ok(listing) => listing
                    .files
                    .into_iter()
                    .map(|f| {
                        serde_json::json!({
                            "filename": f.filename,
                            "quant": f.quant
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(e) => {
                    tracing::warn!(repo_id = %repo_id, "Failed to fetch quant list: {}", e);
                    vec![]
                }
            };

            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": {
                        "message": "quant is required",
                        "type": "ValidationError",
                        "available_quants": available
                    }
                })),
            )
                .into_response();
        }
    };

    // Resolve the quant to a concrete filename from the HF listing.
    let listing = match crate::models::pull::list_gguf_files(&repo_id).await {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to fetch file list from HuggingFace: {}", e),
                        "type": "UpstreamError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Find a file matching the requested quant (case-insensitive).
    let matched_file = listing
        .files
        .iter()
        .find(|f| f.quant.as_deref().map(|q| q.eq_ignore_ascii_case(&quant)) == Some(true));

    let filename = match matched_file {
        Some(f) => f.filename.clone(),
        None => {
            let available: Vec<serde_json::Value> = listing
                .files
                .into_iter()
                .map(|f| serde_json::json!({ "filename": f.filename, "quant": f.quant }))
                .collect();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Quant '{}' not found in repo '{}'", quant, repo_id),
                        "type": "ValidationError",
                        "available_quants": available
                    }
                })),
            )
                .into_response();
        }
    };

    let job_id = format!("pull-{}", uuid::Uuid::new_v4().hyphenated());

    // Create pull job
    let pull_job = PullJob {
        job_id: job_id.clone(),
        repo_id: repo_id.clone(),
        filename: filename.clone(),
        ..Default::default()
    };

    // Store the job
    {
        let mut jobs = state.pull_jobs.write().await;
        jobs.insert(job_id.clone(), pull_job);
    }

    // Enqueue in the DB queue (best-effort — don't fail the pull if enqueue fails)
    let display_name = state
        .model_configs
        .read()
        .await
        .get(&format!(
            "{}--{}",
            repo_id.replace('/', "--"),
            quant.clone()
        ))
        .and_then(|mc| mc.display_name.clone());
    let _ = enqueue_download(
        &state,
        job_id.clone(),
        repo_id.clone(),
        &filename,
        display_name.as_deref(),
        Some(&quant),
        request.context_length,
    );

    Json(serde_json::json!({
        "job_id": job_id,
        "status": "pending",
        "repo_id": repo_id,
        "filename": filename,
        "bytes_downloaded": 0,
        "total_bytes": null,
        "error": null
    }))
    .into_response()
}

/// Handle getting pull job status (Tama management API).
pub async fn handle_tama_get_pull_job(
    state: State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Response {
    let jobs = state.pull_jobs.read().await;
    let job = jobs.get(&job_id).cloned();

    match job {
        Some(j) => {
            let status_str = match j.status {
                crate::proxy::pull_jobs::PullJobStatus::Pending => "pending",
                crate::proxy::pull_jobs::PullJobStatus::Running => "running",
                crate::proxy::pull_jobs::PullJobStatus::Verifying => "verifying",
                crate::proxy::pull_jobs::PullJobStatus::Completed => "completed",
                crate::proxy::pull_jobs::PullJobStatus::Failed => "failed",
            };

            Json(serde_json::json!({
                "job_id": j.job_id,
                "status": status_str,
                "repo_id": j.repo_id,
                "filename": j.filename,
                "bytes_downloaded": j.bytes_downloaded,
                "total_bytes": j.total_bytes,
                "error": j.error
            }))
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Pull job not found",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}

/// Stream `PullJob` snapshots as SSE events every 500 ms until the job reaches a terminal state.
///
/// Events:
/// - `progress`: emitted while the job is pending or running
/// - `done`: emitted once when the job completes or fails, then the stream closes
///
/// Registered as `GET /tama/v1/pulls/:job_id/stream`.
pub async fn handle_pull_job_stream(
    state: State<Arc<ProxyState>>,
    Path(job_id): Path<String>,
) -> Sse<impl futures_util::stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    // State tuple: (proxy_state, job_id, just_emitted_done)
    let stream = stream::unfold(
        (state.0, job_id, false),
        |(state, job_id, just_done)| async move {
            // Previous iteration already emitted the done event.
            // Sleep briefly so the runtime can flush the done event's write buffer
            // before we close the stream — without this the final chunk may not be
            // sent before the connection drops.
            if just_done {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                return None;
            }

            // Poll every 500 ms.
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            let jobs = state.pull_jobs.read().await;
            let Some(job) = jobs.get(&job_id).cloned() else {
                // Job not found — close the stream.
                return None;
            };
            drop(jobs);

            let is_terminal =
                matches!(job.status, PullJobStatus::Completed | PullJobStatus::Failed);
            let event_name = if is_terminal { "done" } else { "progress" };
            let data = serde_json::to_string(&job).unwrap_or_default();
            let event = Event::default().event(event_name).data(data);

            // If terminal, set just_done=true so the next iteration closes the stream.
            Some((Ok(event), (state, job_id, is_terminal)))
        },
    );

    Sse::new(stream).keep_alive(KeepAlive::default())
}
