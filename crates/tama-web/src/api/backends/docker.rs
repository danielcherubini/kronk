//! Docker backend API endpoints.

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::Event;
use axum::response::Sse;
use axum::Json;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::server::AppState;

// ── DTO types ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DockerInstallRequest {
    pub name: String,
    pub compose_yaml: String,
    pub dockerfile: Option<String>,
    pub target_port: Option<u16>,
    pub version: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DockerInstallResponse {
    pub job_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DockerStatusResponse {
    pub state: String,
    pub container_id: Option<String>,
    pub port: Option<u16>,
    pub health_url: String,
    pub uptime_seconds: Option<u64>,
    pub exit_code: Option<i32>,
}

// ── Handlers ───────────────────────────────────────────────────────────────

/// POST /tama/v1/backends/docker/install
pub async fn handle_docker_install(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DockerInstallRequest>,
) -> Result<Json<DockerInstallResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Validate YAML syntax first
    if let Err(_) = serde_yml::from_str::<serde_yml::Value>(&request.compose_yaml) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid compose YAML"})),
        ));
    }

    // Check for name collision
    let config_dir = tama_core::config::Config::base_dir().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("failed to get config dir: {}", e)})),
        )
    })?;
    let save_dir = config_dir.join("docker").join(&request.name);

    // If directory exists, treat as leftover from a failed install and clean it up.
    // (Docker backends are identified by name in the model_config table, not by this directory alone.)
    if save_dir.exists() {
        let _ = std::fs::remove_dir_all(&save_dir);
    }

    let job_manager = state.jobs.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "job manager not configured"})),
        )
    })?;

    // Create a job
    let job = match job_manager
        .submit(crate::jobs::JobKind::DockerInstall, None)
        .await
    {
        Ok(j) => j,
        Err(_) => {
            return Err((
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "another backend job is already running"})),
            ));
        }
    };

    let job_id = job.id.clone();

    // Spawn the install task
    let jm = job_manager.clone();
    let job_clone = job.clone();
    let name = request.name.clone();
    let compose_yaml = request.compose_yaml.clone();
    let dockerfile = request.dockerfile.clone();
    let target_port = request.target_port;
    let version = request.version.clone();

    tokio::spawn(async move {
        let result = async {
            // Get config dir
            let config_dir = tama_core::config::Config::base_dir()
                .map_err(|e| anyhow::anyhow!("Failed to get config dir: {}", e))?;

            // Create DockerBackend
            let docker_backend = tama_core::backends::docker::DockerBackend {
                name: name.clone(),
                compose_yaml: compose_yaml.clone(),
                dockerfile: dockerfile.clone(),
                target_port,
                config_dir: config_dir.clone(),
            };

            // Start container
            let _container_id =
                tama_core::backends::docker::install::start_container(&docker_backend)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to start container: {}", e))?;

            // Insert into backend_installations table
            let db_dir = config_dir.join("db");
            let conn = tama_core::db::open(&db_dir)?;
            let record = tama_core::db::queries::BackendInstallationRecord {
                id: 0,
                name: name.clone(),
                backend_type: "docker".to_string(),
                version: version.clone().unwrap_or("latest".to_string()),
                path: save_dir.to_string_lossy().to_string(),
                installed_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                gpu_type: None,
                source: None,
                is_active: true,
                compose_yaml: Some(compose_yaml.clone()),
                dockerfile: dockerfile.clone(),
                target_port: target_port.map(|p| p as i32),
            };
            tama_core::db::queries::insert_backend_installation(&conn.conn, &record)?;

            // Save compose YAML
            let save_dir = config_dir.join("docker").join(&name);
            std::fs::create_dir_all(&save_dir).ok();
            std::fs::write(save_dir.join("docker-compose.yaml"), &compose_yaml).ok();

            // Save Dockerfile if provided
            if let Some(ref df) = dockerfile {
                std::fs::write(save_dir.join("Dockerfile"), df).ok();
            }

            // Save backend metadata
            let metadata = serde_json::json!({
                "name": name,
                "compose_yaml": compose_yaml,
                "dockerfile": dockerfile,
                "target_port": target_port,
                "version": version,
            });
            std::fs::write(
                save_dir.join("metadata.json"),
                serde_json::to_string_pretty(&metadata).unwrap_or_default(),
            )
            .ok();

            Ok::<_, anyhow::Error>(())
        }
        .await;

        match result {
            Ok(()) => {
                let _ = jm
                    .finish(&job_clone, crate::jobs::JobStatus::Succeeded, None)
                    .await;
            }
            Err(e) => {
                let _ = jm
                    .finish(
                        &job_clone,
                        crate::jobs::JobStatus::Failed,
                        Some(e.to_string()),
                    )
                    .await;
            }
        }
    });

    Ok(Json(DockerInstallResponse { job_id }))
}

/// GET /tama/v1/backends/docker/install/:job_id/stream
pub async fn handle_docker_install_stream(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<
    Sse<impl Stream<Item = Result<Event, axum::Error>>>,
    (StatusCode, Json<serde_json::Value>),
> {
    let jobs = state.jobs.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "job manager not configured"})),
        )
    })?;

    let job = jobs.get(&job_id).await.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "job not found"})),
        )
    })?;

    let mut rx = job.log_tx.subscribe();

    // Snapshot + subscribe: take everything under overlapping locks to avoid races.
    let (head, tail, dropped, status, _finished_at, error) = {
        let (state, log_head, log_tail) =
            tokio::join!(job.state.read(), job.log_head.read(), job.log_tail.read(),);
        (
            log_head.iter().cloned().collect::<Vec<_>>(),
            log_tail.iter().cloned().collect::<Vec<_>>(),
            job.log_dropped.load(std::sync::atomic::Ordering::Relaxed),
            state.status,
            state.finished_at,
            state.error.clone(),
        )
    };

    let stream = stream! {
        // Replay head
        for line in head {
            yield Ok(Event::default().event("log").json_data(serde_json::json!({ "line": line}))?);
        }

        // Emit skipped marker if dropped > 0
        if dropped > 0 && !tail.is_empty() {
            yield Ok(Event::default().event("log")
                .json_data(serde_json::json!({ "line": format!("[... {} lines skipped ...]", dropped)}))?);
        }

        // Replay tail
        for line in tail {
            yield Ok(Event::default().event("log").json_data(serde_json::json!({ "line": line}))?);
        }

        // Emit final status if terminal
        if status != crate::jobs::JobStatus::Running {
            yield Ok(Event::default().event("status")
                .json_data(serde_json::json!({ "status": status}))?);
            if let Some(err) = error {
                yield Ok(Event::default().event("error")
                    .json_data(serde_json::json!({ "error": err}))?);
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
                                .json_data(serde_json::json!({ "line": line}))?);
                        }
                        Ok(crate::jobs::JobEvent::Status(s)) => {
                            yield Ok(Event::default().event("status")
                                .json_data(serde_json::json!({ "status": s}))?);
                            if s != crate::jobs::JobStatus::Running {
                                return; // Close on terminal status
                            }
                        }
                        Ok(crate::jobs::JobEvent::Result(results_json)) => {
                            yield Ok(Event::default().event("result")
                                .json_data(serde_json::json!({ "results": results_json}))?);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // Emit dropped marker
                            yield Ok(Event::default().event("log")
                                .json_data(serde_json::json!({ "line": format!("[{} lines dropped]", n)}))?);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            return;
                        }
                    }
                }
            }
        }
    };

    Ok(Sse::new(stream))
}

/// DELETE /tama/v1/backends/docker/:name
pub async fn handle_docker_uninstall(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let jobs = state.jobs.as_ref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "job manager not configured"})),
        )
    })?;

    let config_dir = tama_core::config::Config::base_dir().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "failed to get config directory"})),
        )
    })?;

    let docker_dir = config_dir.join("docker").join(&name);
    if !docker_dir.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "backend not found"})),
        ));
    }

    let container_name = format!("tama_{}", name);
    let jm = jobs.clone();
    let job_clone = jobs
        .submit(crate::jobs::JobKind::DockerUninstall, None)
        .await
        .ok();

    if let Some(job) = job_clone {
        let name_clone = name.clone();
        let container_name_clone = container_name.clone();
        let config_dir_clone = config_dir.clone();

        tokio::spawn(async move {
            let result = async {
                // Try graceful shutdown via compose
                let compose_path = config_dir_clone
                    .join("docker")
                    .join(&name_clone)
                    .join("docker-compose.yaml");
                if compose_path.exists() {
                    let output = tokio::process::Command::new("docker")
                        .args([
                            "compose",
                            "-f",
                            compose_path.to_string_lossy().as_ref(),
                            "down",
                            "-t",
                            "5",
                        ])
                        .output()
                        .await;

                    match output {
                        Ok(out) if !out.status.success() => {
                            // Try direct kill
                            let _ = tokio::process::Command::new("docker")
                                .args(["kill", &container_name_clone])
                                .output()
                                .await;
                        }
                        _ => {}
                    }
                }

                // Clean up directory
                std::fs::remove_dir_all(&docker_dir).ok();

                // Remove from DB
                let db_dir = config_dir_clone.join("db");
                if let Ok(conn) = tama_core::db::open(&db_dir) {
                    let _ = tama_core::db::queries::delete_backend_installation(
                        &conn.conn,
                        &name_clone,
                        "latest",
                    );
                }

                Ok::<_, anyhow::Error>(())
            }
            .await;

            match result {
                Ok(()) => {
                    let _ = jm
                        .finish(&job, crate::jobs::JobStatus::Succeeded, None)
                        .await;
                }
                Err(e) => {
                    let _ = jm
                        .finish(&job, crate::jobs::JobStatus::Failed, Some(e.to_string()))
                        .await;
                }
            }
        });
    }

    Ok(StatusCode::OK)
}

/// GET /tama/v1/backends/docker/:name/logs
pub async fn handle_docker_logs(
    Path(name): Path<String>,
) -> Result<
    Sse<impl Stream<Item = Result<Event, axum::Error>>>,
    (StatusCode, Json<serde_json::Value>),
> {
    let container_name = format!("tama_{}", name);

    let (mut rx, _handle) = tama_core::backends::docker::logs::stream_logs(&container_name)
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "failed to connect to container logs"})),
            )
        })?;

    let stream = stream! {
        while let Some(line) = rx.recv().await {
            yield Ok(Event::default()
                .event("log")
                .data(line));
        }
    };

    Ok(Sse::new(stream))
}

/// GET /tama/v1/backends/docker/:name/status
pub async fn handle_docker_status(
    Path(name): Path<String>,
) -> Result<Json<DockerStatusResponse>, (StatusCode, Json<serde_json::Value>)> {
    let container_name = format!("tama_{}", name);

    let status = tama_core::backends::docker::health::container_status(&container_name)
        .await
        .unwrap_or_else(|_| "not_found".to_string());

    let container_id = tama_core::backends::docker::health::container_id(&container_name)
        .await
        .ok()
        .flatten();

    // Look up port from metadata file
    let config_dir = tama_core::config::Config::base_dir().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "failed to get config"})),
        )
    })?;
    let metadata_path = config_dir.join("docker").join(&name).join("metadata.json");
    let port: Option<u16> = if metadata_path.exists() {
        let metadata_str = match std::fs::read_to_string(&metadata_path) {
            Ok(s) => s,
            Err(_) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "failed to read metadata"})),
                ));
            }
        };
        let metadata: serde_json::Value = match serde_json::from_str(&metadata_str) {
            Ok(v) => v,
            Err(_) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "failed to parse metadata"})),
                ));
            }
        };
        metadata
            .get("target_port")
            .and_then(|v| v.as_u64())
            .map(|p| p as u16)
    } else {
        None
    };

    let health_url = port
        .map(|p| format!("http://127.0.0.1:{}/health", p))
        .unwrap_or_else(|| "http://127.0.0.1:8000/health".to_string());

    // Get uptime and exit_code from docker inspect
    let (uptime_seconds, exit_code) = if status == "running" {
        match tokio::process::Command::new("docker")
            .args([
                "inspect",
                &container_name,
                "--format",
                "{{.State.StartedAt}}",
            ])
            .output()
            .await
        {
            Ok(inspect_output) if inspect_output.status.success() => {
                let started_at = String::from_utf8_lossy(&inspect_output.stdout)
                    .trim()
                    .to_string();
                match chrono::DateTime::parse_from_rfc3339(&started_at) {
                    Ok(parsed) => {
                        let uptime = chrono::Utc::now()
                            .signed_duration_since(parsed)
                            .num_seconds()
                            .max(0) as u64;
                        (Some(uptime), None)
                    }
                    Err(_) => (None, None),
                }
            }
            _ => (None, None),
        }
    } else if status == "exited" {
        match tokio::process::Command::new("docker")
            .args([
                "inspect",
                &container_name,
                "--format",
                "{{.State.ExitCode}}",
            ])
            .output()
            .await
        {
            Ok(inspect_output) if inspect_output.status.success() => {
                let exit_code = String::from_utf8_lossy(&inspect_output.stdout)
                    .trim()
                    .parse::<i32>()
                    .ok();
                (None, exit_code)
            }
            _ => (None, None),
        }
    } else {
        (None, None)
    };

    Ok(Json(DockerStatusResponse {
        state: status,
        container_id,
        port,
        health_url,
        uptime_seconds,
        exit_code,
    }))
}
