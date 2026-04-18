use async_stream::stream;
use axum::response::sse::{Event, KeepAlive};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Sse,
    Json,
};
use futures_util::Stream;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::broadcast;

use super::types::*;
use crate::server::AppState;

/// GET /api/backends/jobs/:id
#[allow(dead_code)]
pub async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<JobSnapshotDto>, StatusCode> {
    let jobs = state
        .jobs
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let job = jobs.get(&job_id).await.ok_or(StatusCode::NOT_FOUND)?;

    let (state, log_head, log_tail, dropped) = tokio::join!(
        job.state.read(),
        job.log_head.read(),
        job.log_tail.read(),
        async { job.log_dropped.load(Ordering::Relaxed) }
    );

    let mut log: Vec<String> = log_head.iter().cloned().collect();
    if dropped > 0 && !log_tail.is_empty() {
        log.push(format!("[... {} lines skipped ...]", dropped));
    }
    log.extend(log_tail.iter().cloned());

    Ok(Json(JobSnapshotDto {
        id: job.id.clone(),
        kind: match job.kind {
            crate::jobs::JobKind::Install => "install".to_string(),
            crate::jobs::JobKind::Update => "update".to_string(),
            crate::jobs::JobKind::Restore => "restore".to_string(),
        },
        status: state.status,
        backend_type: job
            .backend_type
            .as_ref()
            .map(|b| b.to_string())
            .unwrap_or_default(),
        started_at: state.started_at,
        finished_at: state.finished_at,
        error: state.error.clone(),
        log,
    }))
}

/// GET /api/backends/jobs/:id/events
#[allow(dead_code)]
pub async fn job_events_sse(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, axum::Error>>>, StatusCode> {
    let jobs = state
        .jobs
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let job = jobs.get(&job_id).await.ok_or(StatusCode::NOT_FOUND)?;

    let mut rx = job.log_tx.subscribe();

    // Snapshot + subscribe: take both under the same lock to avoid race
    let (head, tail, dropped, status, _finished_at, error) = {
        let (state, log_head, log_tail) =
            tokio::join!(job.state.read(), job.log_head.read(), job.log_tail.read());
        (
            log_head.iter().cloned().collect::<Vec<_>>(),
            log_tail.iter().cloned().collect::<Vec<_>>(),
            job.log_dropped.load(Ordering::Relaxed),
            state.status,
            state.finished_at,
            state.error.clone(),
        )
    };

    let stream = stream! {
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
        if status != crate::jobs::JobStatus::Running {
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
                        Ok(crate::jobs::JobEvent::Log(line)) => {
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": line}))?);
                        }
                        Ok(crate::jobs::JobEvent::Status(s)) => {
                            yield Ok(Event::default().event("status")
                                .json_data(json!({ "status": s}))?);
                            if s != crate::jobs::JobStatus::Running {
                                return; // Close on terminal status
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            // Emit dropped marker
                            yield Ok(Event::default().event("log")
                                .json_data(json!({ "line": format!("[{} lines dropped]", n)}))?);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            return;
                        }
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
