//! Backend log endpoints: file-based reading and grouped listing.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Json, Sse};
use futures_util::stream::{BoxStream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::logging;
use super::super::ProxyState;

/// Response from GET /tama/v1/logs — grouped by source.
#[derive(Debug, Clone, Serialize)]
pub struct AllLogsResponse {
    pub sources: Vec<SourceLogs>,
}

/// Logs for a single source (e.g. "tama", "llama_cpp_1").
#[derive(Debug, Clone, Serialize)]
pub struct SourceLogs {
    pub name: String,
    pub lines: Vec<String>,
}

/// Query params for GET /tama/v1/logs.
#[derive(Deserialize)]
pub struct AllLogsQuery {
    /// Number of lines per source (default: 200).
    #[serde(default = "default_lines")]
    pub lines: usize,
}

fn default_lines() -> usize {
    200
}

/// GET /tama/v1/logs — return grouped logs from all configured sources.
pub async fn handle_all_logs(
    State(state): State<Arc<ProxyState>>,
    Query(query): Query<AllLogsQuery>,
) -> impl IntoResponse {
    let n = query.lines.min(10_000);

    // Get logs_dir from config
    let logs_dir = match state.config.read().await.logs_dir() {
        Ok(d) => d,
        Err(_) => {
            return Json(serde_json::json!({ "sources": Vec::<SourceLogs>::new() }));
        }
    };

    // Collect tama.log
    let mut sources = Vec::new();
    {
        let tama_path = logs_dir.join("tama.log");
        if tama_path.exists() {
            let lines = match tokio::task::spawn_blocking(move || {
                logging::tail_lines(&tama_path, n)
            })
            .await
            {
                Ok(Ok(l)) => l,
                _ => Vec::new(),
            };
            if !lines.is_empty() {
                sources.push(SourceLogs {
                    name: "tama".to_string(),
                    lines,
                });
            }
        }

        // Collect backend logs (named {backend}_{server_name}.log)
        let mut entries = match std::fs::read_dir(logs_dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        // Sort by modification time (newest first — unstable sort is fine for file list)
        entries.sort_by(|a, b| {
            let a_mod = a.metadata().ok().and_then(|m| m.modified().ok());
            let b_mod = b.metadata().ok().and_then(|m| m.modified().ok());
            b_mod.cmp(&a_mod) // newest first
        });

        for entry in entries {
            let fname = entry.file_name();
            let fname_str = fname.to_string_lossy();
            if fname_str.ends_with(".log") && fname_str != "tama.log" {
                let source_name = fname_str[..fname_str.len() - 4].to_string();
                let path = entry.path();
                let lines = match tokio::task::spawn_blocking(move || {
                    logging::tail_lines(&path, n)
                })
                .await
                {
                    Ok(Ok(l)) => l,
                    _ => Vec::new(),
                };
                if !lines.is_empty() {
                    sources.push(SourceLogs { name: source_name, lines });
                }
            }
        }
    }

    // Sort sources: tama first, then alphabetical
    sources.sort_by(|a, b| {
        if a.name == "tama" {
            std::cmp::Ordering::Less
        } else if b.name == "tama" {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    Json(serde_json::json!({ "sources": sources }))
}

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
