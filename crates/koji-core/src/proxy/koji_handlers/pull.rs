use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::{Path, State},
    response::{sse::Event, sse::KeepAlive, IntoResponse, Response, Sse},
    Json,
};
use futures_util::stream;
use reqwest::StatusCode;

use crate::models::repo_path;

use super::types::{
    is_safe_path_component, max_concurrent_pulls, PullRequest, QuantDownloadSpec, CONFIG_WRITE_LOCK,
};
use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::ProxyState;

/// Spawn a real download task for a single file and return the created `PullJob`.
///
/// The job is inserted into `pull_jobs` before this function returns.
fn spawn_download_job(
    state: Arc<ProxyState>,
    job_id: String,
    repo_id: String,
    filename: String,
    spec: QuantDownloadSpec,
) {
    let pull_jobs_arc = Arc::clone(&state.pull_jobs);
    let in_flight_clone = Arc::clone(&state.in_flight_downloads);
    let state_clone = Arc::clone(&state);
    let job_id_clone = job_id.clone();
    let repo_id_clone = repo_id.clone();
    let filename_clone = filename.clone();
    let spec_clone = spec.clone();

    tokio::spawn(async move {
        tracing::info!(
            job_id = %job_id_clone,
            repo = %repo_id_clone,
            file = %filename_clone,
            "Starting download job"
        );

        // Validate filename and repo_id to prevent path traversal.
        if !is_safe_path_component(&filename_clone) {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some("Invalid filename".to_string());
            }
            return;
        }
        if !repo_id_clone.split('/').all(is_safe_path_component) {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some("Invalid repo_id".to_string());
            }
            return;
        }

        // Update status to Running
        {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Running;
                tracing::info!(job_id = %job_id_clone, "Job transitioned to Running");
            } else {
                tracing::warn!(job_id = %job_id_clone, "Job not found when setting Running");
                return;
            }
        }

        let models_dir = match state_clone.config.read().await.models_dir() {
            Ok(d) => d,
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!("Failed to get models dir: {}", e));
                }
                return;
            }
        };
        // Use the two-level org/repo directory structure (e.g. "unsloth/Qwen3.5-35B-A3B-GGUF")
        // to match the convention expected by ModelRegistry (models_dir/org/repo).
        let dest_dir = repo_path(&models_dir, &repo_id_clone);
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                job.error = Some(format!("Failed to create dest dir: {}", e));
            }
            return;
        }

        let dest_path = dest_dir.join(&filename_clone);

        // In-flight dedup guard: reject if another task is already downloading this path.
        // This prevents two concurrent tasks from writing to the same temp part files,
        // which would silently corrupt the assembled output.
        {
            let mut inflight = in_flight_clone.lock().await;
            if !inflight.insert(dest_path.clone()) {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!(
                        "Another download of '{}' is already in progress",
                        filename_clone
                    ));
                }
                return;
            }
        }

        let url = format!(
            "https://huggingface.co/{}/resolve/main/{}",
            repo_id_clone, filename_clone
        );

        // HEAD request to get total_bytes upfront
        let client = reqwest::Client::new();
        if let Ok(resp) = client.head(&url).send().await {
            let total = crate::models::download::parse_content_length(resp.headers());
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.total_bytes = total;
            }
        }

        // Spawn a task that polls file size every 500ms to update bytes_downloaded
        let poll_jobs = Arc::clone(&pull_jobs_arc);
        let poll_job_id = job_id_clone.clone();
        let poll_dest = dest_path.clone();
        let poll_handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                // If the job is no longer running, stop polling
                {
                    let jobs = poll_jobs.read().await;
                    if let Some(job) = jobs.get(&poll_job_id) {
                        if !matches!(job.status, PullJobStatus::Running) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                // Read file size from disk
                if let Ok(meta) = tokio::fs::metadata(&poll_dest).await {
                    let mut jobs = poll_jobs.write().await;
                    if let Some(job) = jobs.get_mut(&poll_job_id) {
                        job.bytes_downloaded = meta.len();
                    }
                }
            }
        });

        // Get hf-hub API (configured with max_files=8 for parallel downloads)
        let api = match crate::models::pull::hf_api().await {
            Ok(api) => api,
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!("Failed to get hf-hub API client: {}", e));
                }
                return;
            }
        };

        // Create progress callback that updates job status directly
        let progress_jobs = Arc::clone(&pull_jobs_arc);
        let progress_job_id = job_id_clone.clone();
        let progress_callback: crate::models::download::ProgressCallback =
            Arc::new(move |downloaded: u64, total: u64| {
                let job_id = progress_job_id.clone();
                // Use try_write to avoid blocking the download task
                if let Ok(mut jobs) = progress_jobs.try_write() {
                    if let Some(job) = jobs.get_mut(&job_id) {
                        job.bytes_downloaded = downloaded;
                        if total > 0 && job.total_bytes.is_none() {
                            job.total_bytes = Some(total);
                        }
                    }
                }
            });

        tracing::info!(
            job_id = %job_id_clone,
            repo = %repo_id_clone,
            file = %filename_clone,
            "Beginning file download via hf-hub"
        );

        // Use hf-hub's downloader with progress adapter
        let download_start = std::time::Instant::now();
        let repo = api.model(repo_id_clone.clone());
        let progress_adapter = crate::models::pull::ProgressAdapter::new(Some(progress_callback));

        let cached_path = match repo
            .download_with_progress(&filename_clone, progress_adapter)
            .await
        {
            Ok(path) => path,
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!("Download failed: {}", e));
                }
                poll_handle.abort();
                return;
            }
        };

        // Get file size from cached file
        let bytes = match tokio::fs::metadata(&cached_path).await {
            Ok(meta) => meta.len(),
            Err(e) => {
                let mut jobs = pull_jobs_arc.write().await;
                if let Some(job) = jobs.get_mut(&job_id_clone) {
                    job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                    job.error = Some(format!("Failed to get file size: {}", e));
                }
                poll_handle.abort();
                return;
            }
        };

        let download_duration = download_start.elapsed();
        tracing::info!(
            job_id = %job_id_clone,
            bytes = bytes,
            duration = ?download_duration,
            "Download phase complete, entering verify phase"
        );

        // Stop the file size polling task.
        poll_handle.abort();

        // Record final downloaded byte count.
        {
            let mut jobs = pull_jobs_arc.write().await;
            if let Some(job) = jobs.get_mut(&job_id_clone) {
                job.bytes_downloaded = bytes;
                job.total_bytes = Some(bytes);
            }
        }

        // Verify the file while it is still in the HF cache, then move/copy it
        // to the destination only if verification passes. On failure the cache
        // file is deleted so no corrupt data lingers.
        let verified = run_verification(
            Arc::clone(&pull_jobs_arc),
            state_clone.db_dir.clone(),
            job_id_clone.clone(),
            repo_id_clone.clone(),
            filename_clone.clone(),
            spec_clone.quant.clone(),
            cached_path.clone(),
            dest_path.clone(),
            bytes,
        )
        .await;

        // Only register the model in config/card once the file is at its
        // destination and known-good.
        if verified {
            setup_model_after_pull(
                Arc::clone(&state_clone),
                &repo_id_clone,
                &spec_clone,
                &dest_dir,
            )
            .await;
        }

        // Release the in-flight lock after setup and verification complete
        // to prevent concurrent retries from starting mid-processing.
        in_flight_clone.lock().await.remove(&dest_path);
    });
}

/// Run the post-download verification phase for a pull job.
///
/// Hashes the file **in the HF cache** (before it is moved), then:
/// - Pass: canonicalise the cache path to the real blob, rename/copy it to
///   `dest_path`, and delete the cache copy.  Returns `true`.
/// - Fail / hash error: delete the cache copy so no corrupt data lingers.
///   Returns `false`.
///
/// `None` upstream hash is treated as a pass (HF had no LFS SHA to compare).
#[allow(clippy::too_many_arguments)]
async fn run_verification(
    pull_jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    db_dir: Option<std::path::PathBuf>,
    job_id: String,
    repo_id: String,
    filename: String,
    quant_hint: Option<String>,
    cached_path: std::path::PathBuf,
    dest_path: std::path::PathBuf,
    bytes: u64,
) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};

    // Step 1: fetch upstream LFS hash (best-effort).
    let expected_sha: Option<String> =
        match crate::models::pull::fetch_blob_metadata(&repo_id).await {
            Ok(blobs) => blobs.get(&filename).and_then(|b| b.lfs_sha256.clone()),
            Err(e) => {
                tracing::warn!(job_id = %job_id, repo = %repo_id, error = %e,
                "Failed to fetch HF blob metadata for verification");
                None
            }
        };

    // Step 2: transition to Verifying.
    {
        let mut jobs = pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Verifying;
            job.verify_bytes_hashed = 0;
            job.verify_total_bytes = Some(bytes);
            tracing::info!(job_id = %job_id, "Job transitioned to Verifying");
        }
    }

    // Step 3: hash the cached file in a blocking thread.
    // cached_path is an hf-hub snapshot symlink → blob; the OS follows it
    // automatically so we hash the real blob content without resolving manually.
    let progress = Arc::new(AtomicU64::new(0));
    let poll_progress = Arc::clone(&progress);
    let poll_jobs = Arc::clone(&pull_jobs);
    let poll_job_id = job_id.clone();
    let poll_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let hashed = poll_progress.load(Ordering::Relaxed);
            let mut jobs = poll_jobs.write().await;
            let Some(job) = jobs.get_mut(&poll_job_id) else {
                break;
            };
            if !matches!(job.status, PullJobStatus::Verifying) {
                break;
            }
            job.verify_bytes_hashed = hashed;
        }
    });

    let hash_progress = Arc::clone(&progress);
    let hash_src = cached_path.clone(); // hash the cache file, not dest
    let hash_expected = expected_sha.clone();
    let hash_repo = repo_id.clone();
    let hash_filename = filename.clone();
    let hash_quant = quant_hint.clone();
    let hash_db_dir = db_dir.clone();

    let blocking_result = tokio::task::spawn_blocking(move || -> (Option<bool>, Option<String>) {
        let actual = match crate::models::verify::sha256_file(&hash_src, |n| {
            hash_progress.store(n, Ordering::Relaxed);
        }) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(error = %e, path = %hash_src.display(), "Hashing failed");
                None
            }
        };

        let (ok, err): (Option<bool>, Option<String>) =
            match (hash_expected.as_deref(), actual.as_deref()) {
                (None, _) => (None, None),
                (Some(_), None) => (
                    Some(false),
                    Some("hash error: failed to read file".to_string()),
                ),
                (Some(exp), Some(act)) if act.eq_ignore_ascii_case(exp) => (Some(true), None),
                (Some(exp), Some(act)) => (
                    Some(false),
                    Some(format!(
                        "hash mismatch: expected {} got {}",
                        &exp.chars().take(10).collect::<String>(),
                        &act.chars().take(10).collect::<String>()
                    )),
                ),
            };

        if let Some(dir) = hash_db_dir.as_ref() {
            if let Ok(open_res) = crate::db::open(dir) {
                let conn = open_res.conn;
                let _ = crate::db::queries::upsert_model_file(
                    &conn,
                    &hash_repo,
                    &hash_filename,
                    hash_quant.as_deref(),
                    hash_expected.as_deref(),
                    Some(bytes as i64),
                );
                let _ = crate::db::queries::update_verification(
                    &conn,
                    &hash_repo,
                    &hash_filename,
                    ok,
                    err.as_deref(),
                );
            }
        }

        (ok, err)
    })
    .await;

    poll_handle.abort();

    let (ok, err) = blocking_result.unwrap_or_else(|e| {
        tracing::error!(error = %e, "Verification blocking task panicked");
        (
            Some(false),
            Some(format!("verification task panicked: {}", e)),
        )
    });

    let passed = ok != Some(false);

    if passed {
        // Verification passed — move the blob to its final destination.
        // Canonicalise to resolve hf-hub's internal snapshot→blob symlink so
        // we rename/copy the real file, not the symlink entry.
        let blob = tokio::fs::canonicalize(&cached_path)
            .await
            .unwrap_or_else(|_| cached_path.clone());

        if blob != dest_path {
            if dest_path.exists() {
                tokio::fs::remove_file(&dest_path).await.ok();
            }
            if let Err(e) = tokio::fs::rename(&blob, &dest_path).await {
                tracing::debug!(job_id=%job_id, "rename failed ({}), falling back to copy", e);
                match tokio::fs::copy(&blob, &dest_path).await {
                    Ok(_) => {
                        tokio::fs::remove_file(&blob).await.ok();
                    }
                    Err(e2) => {
                        tracing::error!(job_id=%job_id, "copy to dest failed: {}", e2);
                        // Treat as failure — clean up cache and bail.
                        tokio::fs::remove_file(&blob).await.ok();
                        tokio::fs::remove_file(&cached_path).await.ok();
                        let mut jobs = pull_jobs.write().await;
                        if let Some(job) = jobs.get_mut(&job_id) {
                            job.verify_bytes_hashed = bytes;
                            job.verified_ok = Some(false);
                            job.verify_error =
                                Some(format!("failed to move file to destination: {}", e2));
                            job.error = job.verify_error.clone();
                            job.completed_at = Some(Instant::now());
                            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
                        }
                        return false;
                    }
                }
            }
            // Remove the snapshot symlink if it still exists (now dead after rename).
            if cached_path != blob {
                tokio::fs::remove_file(&cached_path).await.ok();
            }
        }

        let mut jobs = pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.verify_bytes_hashed = bytes;
            job.verified_ok = ok;
            job.verify_error = None;
            job.completed_at = Some(Instant::now());
            job.status = crate::proxy::pull_jobs::PullJobStatus::Completed;
            tracing::info!(job_id = %job_id, verified_ok = ?ok, "Job completed");
        }
        true
    } else {
        // Verification failed — delete the corrupt/mismatched cache file so it
        // cannot be mistaken for a good download on the next attempt.
        let blob = tokio::fs::canonicalize(&cached_path)
            .await
            .unwrap_or_else(|_| cached_path.clone());
        tokio::fs::remove_file(&blob).await.ok();
        if cached_path != blob {
            tokio::fs::remove_file(&cached_path).await.ok();
        }
        tracing::error!(job_id = %job_id, error = ?err, "Verification failed — cache deleted");

        let mut jobs = pull_jobs.write().await;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.verify_bytes_hashed = bytes;
            job.verified_ok = ok;
            job.verify_error = err.clone();
            job.error = err;
            job.completed_at = Some(Instant::now());
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            tracing::error!(job_id = %job_id, "Job failed after verification");
        }
        false
    }
}

/// Inner implementation of post-download setup, accepting an explicit config.
/// Separated for testability — `setup_model_after_pull` delegates to this.
pub(crate) async fn _setup_model_after_pull_with_config(
    config: &mut crate::config::Config,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
) {
    let configs_dir = match config.configs_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let repo_slug = repo_id.replace('/', "--");
    let card_path = configs_dir.join(format!("{}.toml", repo_slug));

    // Load existing or build a new card
    let mut card = crate::models::card::ModelCard::load(&card_path).unwrap_or_else(|_| {
        crate::models::card::ModelCard {
            model: crate::models::card::ModelMeta {
                name: repo_id.to_string(),
                source: repo_id.to_string(),
                default_context_length: None,
                default_gpu_layers: None,
            },
            sampling: std::collections::HashMap::new(),
            quants: std::collections::HashMap::new(),
        }
    });

    // Try community card for sampling presets and context defaults (best-effort, no network in tests).
    // We intentionally do NOT overwrite card.model.name from the community card — community cards
    // often have the GGUF suffix stripped (e.g. "OmniCoder-9B" instead of "OmniCoder-9B-GGUF"),
    // which loses information. The name is derived from the repo_id above and kept as-is.
    if let Some(community) = crate::models::pull::fetch_community_card(repo_id).await {
        for (k, v) in community.sampling {
            card.sampling.entry(k).or_insert(v);
        }
        if card.model.default_context_length.is_none() {
            card.model.default_context_length = community.model.default_context_length;
        }
        if card.model.default_gpu_layers.is_none() {
            card.model.default_gpu_layers = community.model.default_gpu_layers;
        }
    }

    // Determine the quant key
    let quant_key = spec.quant.clone().unwrap_or_else(|| {
        crate::models::pull::infer_quant_from_filename(&spec.filename).unwrap_or_else(|| {
            // Fallback: use last component after splitting by `-` or `_`
            spec.filename
                .trim_end_matches(".gguf")
                .split(|c| ['-', '_'].contains(&c))
                .next_back()
                .unwrap_or("unknown")
                .to_string()
        })
    });

    // Get actual file size from disk
    let size_bytes = std::fs::metadata(dest_dir.join(&spec.filename))
        .ok()
        .map(|m| m.len());

    // Insert/update quant entry in card. Detect mmproj files by filename so
    // they get tagged with `kind = Mmproj` and tracked separately from real
    // model quants.
    card.quants.insert(
        quant_key.clone(),
        crate::models::card::QuantInfo {
            file: spec.filename.clone(),
            kind: crate::config::QuantKind::from_filename(&spec.filename),
            size_bytes,
            context_length: spec.context_length,
        },
    );

    // Find an existing model entry for this repo (if any), regardless of
    // its key format. This prevents creating duplicate model entries when
    // pulling additional quants for a model that's already in the config.
    // Matching is by the `model` field rather than the key, so user-renamed
    // entries are preserved.
    let existing_key: Option<String> = config
        .models
        .iter()
        .find(|(_, m)| m.model.as_deref() == Some(repo_id))
        .map(|(k, _)| k.clone());

    // For mmproj files: don't create or modify a model entry. The mmproj is
    // a sibling file that gets attached to an existing model only when the
    // user explicitly enables it via the editor's vision toggle.
    let is_mmproj = matches!(
        crate::config::QuantKind::from_filename(&spec.filename),
        crate::config::QuantKind::Mmproj
    );
    if !is_mmproj {
        // Fetch pipeline_tag from HF to infer modalities (best-effort).
        let modalities = match crate::models::pull::fetch_model_pipeline_tag(repo_id).await {
            Ok(pipeline_tag) => {
                crate::models::pull::infer_modalities_from_pipeline(pipeline_tag.as_deref())
            }
            Err(e) => {
                tracing::debug!(repo = %repo_id, error = %e, "Failed to fetch pipeline_tag for modalities inference");
                None
            }
        };

        // Generate display name from HF repo name (e.g., "Unsloth: Qwen3.5 35B A3B").
        let display_name = crate::proxy::koji_handlers::generate_display_name(repo_id);

        // Reuse the existing model key if we found one, otherwise create a
        // new entry keyed by the bare repo slug (no per-quant suffix).
        let model_key = existing_key.unwrap_or_else(|| repo_slug.to_lowercase());
        config
            .models
            .entry(model_key)
            .or_insert_with(|| crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                model: Some(repo_id.to_string()),
                quant: Some(quant_key),
                mmproj: None,
                context_length: spec.context_length,
                enabled: true,
                args: vec![],
                sampling: None,
                port: None,
                health_check: None,
                profile: None,
                api_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
                modalities,
                display_name: Some(display_name),
            });
    }

    // Save card and config (best-effort — download is already marked Completed)
    let _ = std::fs::create_dir_all(&configs_dir);
    let _ = card.save(&card_path);
    let _ = config.save();
}

/// Post-download: auto-create model card and config entries.
///
/// Called after a quant download completes. Updates the model card, saves config to
/// disk, and — critically — also inserts the new model entry into the live
/// `ProxyState.config` so it appears immediately in the models list without a restart.
pub(crate) async fn setup_model_after_pull(
    state: Arc<ProxyState>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
) {
    let _guard = CONFIG_WRITE_LOCK.lock().await;
    let mut config = state.config.write().await;
    _setup_model_after_pull_with_config(&mut config, repo_id, spec, dest_dir).await;
    // _guard dropped here, releasing the lock
    // config write guard also dropped here, making the new model entry visible immediately
}

/// Handle starting a pull job (Koji management API).
pub async fn handle_koji_pull_model(
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

            spawn_download_job(
                Arc::clone(&state),
                job_id.clone(),
                repo_id.clone(),
                spec.filename.clone(),
                spec.clone(),
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

    // Spawn real download task
    let legacy_spec = QuantDownloadSpec {
        filename: filename.clone(),
        quant: Some(quant.clone()),
        context_length: request.context_length,
    };
    spawn_download_job(
        Arc::clone(&state),
        job_id.clone(),
        repo_id.clone(),
        filename.clone(),
        legacy_spec,
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

/// Handle getting pull job status (Koji management API).
pub async fn handle_koji_get_pull_job(
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
/// Registered as `GET /koji/v1/pulls/:job_id/stream`.
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
