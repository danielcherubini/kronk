use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use super::resolve_model_id;
use crate::api::load_config_from_state;
use crate::server::AppState;

/// Serialize a `ModelFileRecord` into the same shape used by the enriched
/// quants response so refresh/verify callers get data identical to a GET.
fn file_record_json(rec: &koji_core::db::queries::ModelFileRecord) -> serde_json::Value {
    serde_json::json!({
        "filename": rec.filename,
        "quant": rec.quant,
        "lfs_oid": rec.lfs_oid,
        "size_bytes": rec.size_bytes,
        "downloaded_at": rec.downloaded_at,
        "last_verified_at": rec.last_verified_at,
        "verified_ok": rec.verified_ok,
        "verify_error": rec.verify_error,
    })
}

// ── Refresh / Verify ──────────────────────────────────────────────────────────

/// POST /api/models/:id/refresh — re-query HuggingFace for the current commit
/// SHA and per-file LFS hashes / sizes, and write them into the local DB.
///
/// Structured to keep `rusqlite::Connection` off `.await` points:
///   1. `spawn_blocking` — resolve repo_id from config
///   2. `.await` — fetch from HF
///   3. `spawn_blocking` — open DB, upsert pull + files, read back
pub async fn refresh_model_metadata(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    // Step 1: resolve model_id (from id_str) and repo_id (config load on blocking pool).
    let state1 = state.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let (cfg, config_dir) = load_config_from_state(&state1)?;
        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let models_dir = cfg.models_dir().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok::<_, (StatusCode, serde_json::Value)>((model_id, record.repo_id, config_dir, models_dir))
    })
    .await;
    let (model_id, repo_id, config_dir, _models_dir) = match resolved {
        Ok(Ok(x)) => x,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Step 2: async HF fetches (no DB handle held).
    let listing = match koji_core::models::pull::list_gguf_files(&repo_id).await {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("HuggingFace listing failed: {}", e)
                })),
            )
                .into_response();
        }
    };
    let blobs = match koji_core::models::pull::fetch_blob_metadata(&listing.repo_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("fetch_blob_metadata failed for {}: {}", listing.repo_id, e);
            std::collections::HashMap::new()
        }
    };

    // Step 3: DB writes (blocking pool, fresh connection).
    let repo_id_for_db = repo_id.clone();
    let config_dir_for_db = config_dir.clone();
    let commit_sha = listing.commit_sha.clone();
    let files = listing.files.clone();
    let write = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let open = koji_core::db::open(&config_dir_for_db)?;
        let conn = &open.conn;
        koji_core::db::queries::upsert_model_pull(conn, model_id, &repo_id_for_db, &commit_sha)?;
        for file in &files {
            let blob = blobs.get(&file.filename);
            koji_core::db::queries::upsert_model_file(
                conn,
                model_id,
                &repo_id_for_db,
                &file.filename,
                file.quant.as_deref(),
                blob.and_then(|b| b.lfs_sha256.as_deref()),
                blob.and_then(|b| b.size),
            )?;
        }
        let files_out = koji_core::db::queries::get_model_files(conn, model_id)?;
        let pull_out = koji_core::db::queries::get_model_pull(conn, model_id)?;
        Ok((pull_out, files_out))
    })
    .await;

    match write {
        Ok(Ok((pull, files))) => {
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": true,
                "id": model_id,
                "repo_id": repo_id,
                "repo_commit_sha": pull.as_ref().map(|p| p.commit_sha.clone()),
                "repo_pulled_at": pull.as_ref().map(|p| p.pulled_at.clone()),
                "files": files_json,
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("DB write failed: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/models/:id/verify — recompute SHA-256 for every tracked file of
/// this model and compare against the stored LFS hash, persisting the result.
///
/// Sequential, CPU-bound, potentially multi-minute for large GGUFs. Runs on
/// the blocking threadpool. Per-file progress events are NOT streamed here;
/// the wizard already streams them during pulls.
pub async fn verify_model_files(
    State(state): State<Arc<AppState>>,
    Path(id_str): Path<String>,
) -> impl IntoResponse {
    let state1 = state.clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let (_cfg, config_dir) = load_config_from_state(&state1)?;
        let open = koji_core::db::open(&config_dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        let model_id = resolve_model_id(&id_str, &open.conn)
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let record = koji_core::db::queries::get_model_config(&open.conn, model_id)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": e.to_string()}),
                )
            })?
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    serde_json::json!({"error": "Model not found"}),
                )
            })?;
        let models_dir = _cfg.models_dir().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}),
            )
        })?;
        Ok::<_, (StatusCode, serde_json::Value)>((model_id, record.repo_id, config_dir, models_dir))
    })
    .await;
    let (model_id, repo_id, config_dir, models_dir) = match resolved {
        Ok(Ok(x)) => x,
        Ok(Err((s, b))) => return (s, Json(b)).into_response(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    // Model files live at <models_dir>/<repo_id>/<filename>.gguf
    let model_dir = koji_core::models::repo_path(&models_dir, &repo_id);
    let repo_id_clone = repo_id.clone();

    let task = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let open = koji_core::db::open(&config_dir)?;
        let results = koji_core::models::verify::verify_model(
            &open.conn,
            model_id,
            &repo_id_clone,
            &model_dir,
        )?;
        let files = koji_core::db::queries::get_model_files(&open.conn, model_id)?;
        Ok((results, files))
    })
    .await;

    match task {
        Ok(Ok((results, files))) => {
            let all_ok = results.iter().all(|r| r.ok != Some(false));
            let any_unknown = results.iter().any(|r| r.ok.is_none());
            let summary: Vec<_> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "filename": r.filename,
                        "ok": r.ok,
                        "error": r.error,
                    })
                })
                .collect();
            let files_json: Vec<_> = files.iter().map(file_record_json).collect();
            Json(serde_json::json!({
                "ok": all_ok,
                "any_unknown": any_unknown,
                "id": model_id,
                "repo_id": repo_id,
                "results": summary,
                "files": files_json,
            }))
            .into_response()
        }
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("verify failed: {}", e)})),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}
