//! Backend log SSE endpoint: GET /tama/v1/logs/:backend/events

use axum::extract::{Path, State};
use axum::response::Sse;
use futures_util::stream::{BoxStream, StreamExt};
use serde_json::json;
use std::sync::Arc;

use super::super::ProxyState;

/// Type-erased SSE stream for backend logs.
type LogStream = BoxStream<'static, Result<axum::response::sse::Event, axum::Error>>;

/// GET /tama/v1/logs/:backend/events — SSE stream of backend log lines.
pub async fn handle_backend_log_sse(
    State(state): State<Arc<ProxyState>>,
    Path(backend): Path<String>,
) -> Sse<LogStream> {
    let backend_logs = &state.backend_logs;

    let stream: LogStream = match backend_logs.get(&backend).await {
        Some(log_stream) => {
            let rx = log_stream.subscribe();
            let head = log_stream.snapshot().await;
            futures_util::stream::iter(head.into_iter().map(|line| {
                Ok(axum::response::sse::Event::default()
                    .event("log")
                    .json_data(json!({ "line": line }))
                    .unwrap())
            }))
            .chain(futures_util::stream::unfold(rx, move |mut rx| async move {
                loop {
                    match rx.recv().await {
                        Ok(line) => {
                            return Some((
                                Ok(axum::response::sse::Event::default()
                                    .event("log")
                                    .json_data(json!({ "line": line }))
                                    .unwrap()),
                                rx,
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::debug!("Backend log subscriber lagged by {} lines", n);
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return None;
                        }
                    }
                }
            }))
            .boxed()
        }
        None => {
            // Return an empty stream that stays open but sends no data.
            // This keeps the SSE connection alive without spamming requests
            // when there's no active backend to stream from.
            futures_util::stream::empty().boxed()
        }
    };

    Sse::new(stream)
}
