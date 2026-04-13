//! Backup and restore API endpoints.

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::services::ServeDir;
use uuid::Uuid;

use crate::server::AppState;

/// Request body for backup.
#[derive(Deserialize)]
pub struct BackupRequest {}

/// Response body for backup.
#[derive(Serialize)]
pub struct BackupResponse {
    pub path: String,
    pub size_bytes: u64,
    pub models: usize,
    pub backends: usize,
}

/// Request body for restore preview.
#[derive(Deserialize)]
pub struct RestorePreviewRequest {
    pub upload_id: String,
}

/// Response body for restore preview.
#[derive(Serialize)]
pub struct RestorePreviewResponse {
    pub upload_id: String,
    pub created_at: String,
    pub koji_version: String,
    pub models: Vec<BackupModelEntry>,
    pub backends: Vec<BackendEntry>,
}

/// Request body for restore.
#[derive(Deserialize)]
pub struct RestoreRequest {
    pub upload_id: String,
    #[serde(default)]
    pub selected_models: Option<Vec<String>>,
    #[serde(default)]
    pub skip_backends: bool,
    #[serde(default)]
    pub skip_models: bool,
}

/// Response body for restore.
#[derive(Serialize)]
pub struct RestoreResponse {
    pub job_id: String,
}

/// Model entry for backup manifest.
#[derive(Serialize, Clone)]
pub struct BackupModelEntry {
    pub repo_id: String,
    pub quants: Vec<String>,
    pub total_size_bytes: i64,
}

/// Backend entry for backup manifest.
#[derive(Serialize, Clone)]
pub struct BackendEntry {
    pub name: String,
    pub version: String,
    pub backend_type: String,
    pub source: String,
}

/// GET /api/backup - Create backup and return as file download
pub async fn create_backup(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config_dir = match &state.config_path {
        Some(p) => p.parent().unwrap_or(p.as_path()),
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "config_path not configured"})),
            )
                .into_response();
        }
    };

    let config_dir = config_dir.to_path_buf();

    // Spawn blocking task for backup
    let result = tokio::task::spawn_blocking(move || {
        let temp_dir = tempfile::tempdir().map_err(|e| anyhow::anyhow!(e))?;
        let output_path = temp_dir.path().join("backup.tar.gz");

        let manifest =
            koji_core::backup::create_backup(&config_dir, &output_path).map_err(|e| anyhow::anyhow!(e))?;

        let size = std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0);

        Ok::<_, anyhow::Error>((output_path, manifest, size))
    })
    .await;

    match result {
        Ok(Ok((output_path, manifest, size))) => {
            // Read the file and return as attachment
            let file_bytes = std::fs::read(&output_path)
                .map_err(|e| anyhow::anyhow!(e))?;

            (
                StatusCode::OK,
                [(
                    "Content-Type",
                    "application/gzip",
                )],
                [(
                    "Content-Disposition",
                    format!("attachment; filename=\"{}\"", output_path.file_name().unwrap_or_default().to_string_lossy()),
                )],
                file_bytes,
            )
                .into_response()
        }
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

/// POST /api/restore/preview - Upload archive and return manifest preview
pub async fn restore_preview(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    // Save upload to temp file
    let temp_dir = state.temp_uploads_dir();
    std::fs::create_dir_all(&temp_dir).ok();

    let upload_id = Uuid::new_v4().simple().to_string();
    let upload_path = temp_dir.join(format!("{}.tar.gz", upload_id));

    while let Some(field) = multipart.next_field().await.ok().flatten() {
        let bytes = field.bytes().await.unwrap_or_default();
        std::fs::write(&upload_path, &bytes).ok();
    }

    // Extract manifest
    let manifest_result = tokio::task::spawn_blocking(move || {
        koji_core::backup::extract_manifest(&upload_path)
    })
    .await;

    match manifest_result {
        Ok(Ok(manifest)) => {
            // Store upload reference
            let mut uploads = state.upload_lock.write().await;
            uploads.insert(
                upload_id.clone(),
                UploadEntry {
                    path: upload_path.clone(),
                    created_at: chrono::Utc::now(),
                },
            );

            Json(RestorePreviewResponse {
                upload_id,
                created_at: manifest.created_at,
                koji_version: manifest.koji_version,
                models: manifest
                    .models
                    .into_iter()
                    .map(|m| BackupModelEntry {
                        repo_id: m.repo_id,
                        quants: m.quants,
                        total_size_bytes: m.total_size_bytes,
                    })
                    .collect(),
                backends: manifest
                    .backends
                    .into_iter()
                    .map(|b| BackendEntry {
                        name: b.name,
                        version: b.version,
                        backend_type: b.backend_type,
                        source: b.source,
                    })
                    .collect(),
            })
                .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
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

/// POST /api/restore - Start restore job
pub async fn start_restore(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RestoreRequest>,
) -> impl IntoResponse {
    // Look up upload
    let uploads = state.upload_lock.read().await;
    let upload_path = match uploads.get(&body.upload_id) {
        Some(entry) => entry.path.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "Upload not found or expired"})),
            )
                .into_response();
        }
    };
    drop(uploads);

    // Create restore job
    let job = state
        .jobs
        .as_ref()
        .expect("Jobs not configured")
        .submit(
            crate::jobs::JobKind::Restore,
            None, // No backend type for restore
        )
        .await;

    match job {
        Ok(job) => {
            // Spawn background task for restore
            let config_dir = state.config_path.as_ref().unwrap().parent().unwrap().to_path_buf();
            let temp_dir = state.temp_uploads_dir();
            let job_id = job.id.clone();

            tokio::spawn(async move {
                let _ = tokio::task::spawn_blocking(move || {
                    // TODO: Implement actual restore logic
                    // This would call koji_core::backup functions
                    let _ = (config_dir, temp_dir, job);
                })
                .await;
            });

            Json(RestoreResponse { job_id }).into_response()
        }
        Err(e) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// Temporary upload entry.
#[derive(Clone)]
pub struct UploadEntry {
    pub path: std::path::PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
