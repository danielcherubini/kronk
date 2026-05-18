use std::sync::Arc;
use std::time::Instant;

use crate::config::default_num_parallel;
use crate::models::repo_path;
use crate::proxy::download_queue::DownloadQueueService;
use crate::proxy::pull_jobs::{PullJob, PullJobStatus};
use crate::proxy::tama_handlers::types::{is_safe_path_component, QuantDownloadSpec};
use crate::proxy::ProxyState;

/// Start a download from the queue.
///
/// This is the ONLY code path that starts a download from the queue processor.
/// Takes a `job_id`, `state`, and `QuantDownloadSpec`, performs the actual download,
/// and updates both the DB queue item and in-memory PullJob on completion/failure.
pub async fn start_download_from_queue(
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

    // Record start time for duration calculation
    let download_start = std::time::Instant::now();

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
        drop(jobs);
        if let Some(ref svc) = state_clone.download_queue {
            let _ = svc.update_status(
                &job_id_clone,
                "failed",
                0,
                None,
                Some("Invalid filename"),
                None,
            );
        }
        return;
    }
    if !repo_id_clone.split('/').all(is_safe_path_component) {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            job.error = Some("Invalid repo_id".to_string());
        }
        drop(jobs);
        if let Some(ref svc) = state_clone.download_queue {
            let _ = svc.update_status(
                &job_id_clone,
                "failed",
                0,
                None,
                Some("Invalid repo_id"),
                None,
            );
        }
        return;
    }

    // Update status to Running
    {
        let mut jobs = pull_jobs_arc.write().await;
        let map_ptr = &*jobs as *const _;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.status = crate::proxy::pull_jobs::PullJobStatus::Running;
            tracing::info!(
                job_id = %job_id_clone,
                map_addr = ?map_ptr,
                "Job transitioned to Running"
            );
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
            drop(jobs);
            if let Some(ref svc) = state_clone.download_queue {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to get models dir: {}", e)),
                    None,
                );
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
        drop(jobs);
        if let Some(ref svc) = state_clone.download_queue {
            let _ = svc.update_status(
                &job_id_clone,
                "failed",
                0,
                None,
                Some(&format!("Failed to create dest dir: {}", e)),
                None,
            );
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
            drop(jobs);
            if let Some(ref svc) = state_clone.download_queue {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!(
                        "Another download of '{}' is already in progress",
                        filename_clone
                    )),
                    None,
                );
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
        drop(jobs);
    }

    // Spawn a task that polls file size every 500ms to update bytes_downloaded
    // and pushes progress updates to the DB queue for SSE streaming.
    let poll_jobs = Arc::clone(&pull_jobs_arc);
    let poll_job_id = job_id_clone.clone();
    let poll_dest = dest_path.clone();
    let poll_download_queue = state_clone.download_queue.clone();
    let poll_handle = tokio::spawn(async move {
        let mut last_progress_pct: u32 = 0;
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
                let bytes_downloaded = meta.len();
                let mut jobs = poll_jobs.write().await;
                if let Some(job) = jobs.get_mut(&poll_job_id) {
                    job.bytes_downloaded = bytes_downloaded;
                    // Push progress to DB queue for SSE streaming (throttled to 1% steps)
                    if let Some(total) = job.total_bytes {
                        if total > 0 {
                            let pct = (bytes_downloaded as f64 / total as f64 * 100.0) as u32;
                            if pct > last_progress_pct {
                                last_progress_pct = pct;
                                drop(jobs);
                                if let Some(ref svc) = poll_download_queue {
                                    let _ = svc.update_progress(
                                        &poll_job_id,
                                        bytes_downloaded as i64,
                                        Some(total as i64),
                                    );
                                }
                            }
                        }
                    }
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
            drop(jobs);
            poll_handle.abort();
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.download_queue {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to get hf-hub API client: {}", e)),
                    None,
                );
            }
            return;
        }
    };

    // Create progress callback that updates job status and emits SSE events
    let progress_jobs = Arc::clone(&pull_jobs_arc);
    let progress_job_id = job_id_clone.clone();
    let progress_queue = state_clone.download_queue.clone();
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
            // Emit SSE progress event directly (throttled by hf-hub's callback frequency)
            if let Some(ref svc) = progress_queue {
                let _ = svc.update_progress(&job_id, downloaded as i64, Some(total as i64));
            }
        });

    tracing::info!(
        job_id = %job_id_clone,
        repo = %repo_id_clone,
        file = %filename_clone,
        "Beginning file download via hf-hub"
    );

    // Use hf-hub's downloader with progress adapter
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
            drop(jobs);
            poll_handle.abort();
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.download_queue {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Download failed: {}", e)),
                    None,
                );
            }
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
            drop(jobs);
            poll_handle.abort();
            in_flight_clone.lock().await.remove(&dest_path);
            if let Some(ref svc) = state_clone.download_queue {
                let _ = svc.update_status(
                    &job_id_clone,
                    "failed",
                    0,
                    None,
                    Some(&format!("Failed to get file size: {}", e)),
                    None,
                );
            }
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
    let outcome = run_verification(
        Arc::clone(&pull_jobs_arc),
        state_clone.db_dir.clone(),
        state_clone.download_queue.clone(),
        job_id_clone.clone(),
        repo_id_clone.clone(),
        filename_clone.clone(),
        spec_clone.quant.clone(),
        cached_path.clone(),
        dest_path.clone(),
        bytes,
    )
    .await;

    // Calculate duration for DB event
    let duration_ms = Some(download_start.elapsed().as_millis() as u64);

    // Parse GGUF metadata (soft failure — don't fail the download)
    // Skip mmproj files — they're vision projectors, not LLM models.
    let is_mmproj = matches!(
        crate::config::QuantKind::from_filename(&filename_clone),
        crate::config::QuantKind::Mmproj
    );
    let gguf_metadata = if outcome.passed && !is_mmproj {
        match crate::models::gguf::parse_gguf_metadata(&dest_path) {
            Ok(meta) => {
                tracing::info!(
                    job_id = %job_id_clone,
                    architecture = ?meta.architecture,
                    context_length = ?meta.context_length,
                    "GGUF metadata parsed"
                );
                Some(meta)
            }
            Err(e) => {
                tracing::warn!(
                    job_id = %job_id_clone,
                    error = %e,
                    "GGUF metadata parsing failed — using defaults"
                );
                None
            }
        }
    } else {
        None
    };

    // Store GGUF metadata in PullJob for SSE streaming
    {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.gguf_metadata = gguf_metadata.clone();
            // Also set the serialized field for SSE events (frontend reads this)
            job.gguf_context_length = gguf_metadata.as_ref().and_then(|m| m.context_length);
        }
    }

    // Only register the model in config/card once the file is at its
    // destination and known-good. setup_model_after_pull creates the
    // matching model_configs row, which the model_files row below FKs to.
    if outcome.passed {
        let model_id = setup_model_after_pull(
            Arc::clone(&state_clone),
            &repo_id_clone,
            &spec_clone,
            &dest_dir,
            gguf_metadata.clone(),
        )
        .await;

        // Persist the hash + verification state to model_files now that
        // the parent model_configs row exists. Use the id returned by
        // setup_model_after_pull so there's no case-sensitive lookup in
        // between that could miss.
        match (state_clone.model_mgr(), model_id) {
            (Some(mgr), Some(mid)) => {
                if let Err(e) = mgr.upsert_file(
                    mid,
                    &repo_id_clone,
                    &filename_clone,
                    spec_clone.quant.as_deref(),
                    outcome.expected_sha.as_deref(),
                    Some(bytes as i64),
                ) {
                    tracing::error!(
                        job_id = %job_id_clone,
                        model_id = mid,
                        file = %filename_clone,
                        error = %e,
                        "upsert_model_file failed"
                    );
                } else {
                    tracing::info!(
                        job_id = %job_id_clone,
                        model_id = mid,
                        file = %filename_clone,
                        "model_files row written"
                    );
                }
                if let Err(e) = mgr.update_verification(
                    mid,
                    &filename_clone,
                    outcome.ok,
                    outcome.err.as_deref(),
                ) {
                    tracing::warn!(
                        job_id = %job_id_clone,
                        model_id = mid,
                        file = %filename_clone,
                        error = %e,
                        "update_verification failed"
                    );
                }
            }
            (None, _) => {
                tracing::warn!(
                    job_id = %job_id_clone,
                    "db_dir not configured — model_files row skipped"
                );
            }
            (Some(_), None) => {
                tracing::error!(
                    job_id = %job_id_clone,
                    repo = %repo_id_clone,
                    "setup_model_after_pull returned no model_id — model_files skipped"
                );
            }
        }
    }

    // Release the in-flight lock after setup and verification complete
    // to prevent concurrent retries from starting mid-processing.
    in_flight_clone.lock().await.remove(&dest_path);

    // Update DB queue item with final status
    let final_status = if outcome.passed {
        "completed"
    } else {
        "failed"
    };
    let error_msg = if !outcome.passed {
        outcome.err.as_deref()
    } else {
        None
    };

    if let Some(ref svc) = state_clone.download_queue {
        let _ = svc.update_status(
            &job_id_clone,
            final_status,
            bytes as i64,
            Some(bytes as i64),
            error_msg,
            duration_ms,
        );
    }

    // Update in-memory PullJob with duration
    {
        let mut jobs = pull_jobs_arc.write().await;
        if let Some(job) = jobs.get_mut(&job_id_clone) {
            job.duration_ms = duration_ms;
        }
    }
}

/// Outcome of a verification pass. Carries the hash info so the caller can
/// persist it to `model_files` *after* `setup_model_after_pull` has created
/// the matching `model_configs` row (the DB FK requires it to exist).
pub(crate) struct VerificationOutcome {
    pub passed: bool,
    pub expected_sha: Option<String>,
    pub ok: Option<bool>,
    pub err: Option<String>,
}

/// Run the post-download verification phase for a pull job.
///
/// Hashes the file **in the HF cache** (before it is moved), then:
/// - Pass: canonicalise the cache path to the real blob, rename/copy it to
///   `dest_path`, and delete the cache copy. Returns `passed = true`.
/// - Fail / hash error: delete the cache copy so no corrupt data lingers.
///   Returns `passed = false`.
///
/// `None` upstream hash is treated as a pass (HF had no LFS SHA to compare).
#[allow(clippy::too_many_arguments)]
async fn run_verification(
    pull_jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    _db_dir: Option<std::path::PathBuf>,
    download_queue: Option<Arc<DownloadQueueService>>,
    job_id: String,
    repo_id: String,
    filename: String,
    _quant_hint: Option<String>,
    cached_path: std::path::PathBuf,
    dest_path: std::path::PathBuf,
    bytes: u64,
) -> VerificationOutcome {
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

    // Update DB queue item to "verifying" so Downloads Center shows progress.
    if let Some(ref svc) = download_queue {
        let _ = svc.update_status(
            &job_id,
            "verifying",
            bytes as i64,
            Some(bytes as i64),
            None,
            None,
        );
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
        }
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
                        return VerificationOutcome {
                            passed: false,
                            expected_sha: expected_sha.clone(),
                            ok,
                            err,
                        };
                    }
                }
            }
            // Remove the snapshot symlink if it still exists (now dead after rename).
            if cached_path != blob {
                tokio::fs::remove_file(&cached_path).await.ok();
            }
        }

        let mut jobs = pull_jobs.write().await;
        let map_ptr = &*jobs as *const _;
        if let Some(job) = jobs.get_mut(&job_id) {
            job.verify_bytes_hashed = bytes;
            job.verified_ok = ok;
            job.verify_error = None;
            job.completed_at = Some(Instant::now());
            job.status = crate::proxy::pull_jobs::PullJobStatus::Completed;
            tracing::info!(
                job_id = %job_id,
                verified_ok = ?ok,
                bytes_downloaded = job.bytes_downloaded,
                map_addr = ?map_ptr,
                "Job completed"
            );
        }
        VerificationOutcome {
            passed: true,
            expected_sha,
            ok,
            err,
        }
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
            job.error = err.clone();
            job.completed_at = Some(Instant::now());
            job.status = crate::proxy::pull_jobs::PullJobStatus::Failed;
            tracing::error!(job_id = %job_id, "Job failed after verification");
        }
        VerificationOutcome {
            passed: false,
            expected_sha,
            ok,
            err,
        }
    }
}

/// Inner implementation of post-download setup, accepting an explicit config.
/// Separated for testability — `setup_model_after_pull` delegates to this.
pub(crate) async fn _setup_model_after_pull_with_config(
    configs_dir: &std::path::Path,
    model_configs: &mut std::collections::HashMap<String, crate::config::ModelConfig>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
    gguf_metadata: Option<&crate::models::pull::GgufMetadata>,
) -> Option<String> {
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

    // Determine context_length: GGUF parsed value > spec value > None
    let context_length = gguf_metadata
        .and_then(|m| m.context_length.map(|v| v as u32))
        .or(spec.context_length);

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
            context_length,
        },
    );

    // Find an existing model entry for this repo (if any), regardless of
    // its key format. This prevents creating duplicate model entries when
    // pulling additional quants for a model that's already in the config.
    // Matching is by the `model` field rather than the key, so user-renamed
    // entries are preserved.
    let existing_key: Option<String> = model_configs
        .iter()
        .find(|(_, m)| m.model.as_deref() == Some(repo_id))
        .map(|(k, _)| k.clone());

    // For mmproj files: if the parent model config already exists, turn on
    // vision by wiring the mmproj filename and adding "image" to the input
    // modalities. Downloading an mmproj is an explicit user choice, so
    // enabling vision automatically saves an extra click in the editor.
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
        let display_name = crate::proxy::tama_handlers::generate_display_name(repo_id);

        // Reuse the existing model key if we found one, otherwise create a
        // new entry keyed by the bare repo slug (no per-quant suffix).
        let model_key = existing_key.unwrap_or_else(|| repo_slug.to_lowercase());
        let entry =
            model_configs
                .entry(model_key.clone())
                .or_insert_with(|| crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: None,
                    model: Some(repo_id.to_string()),
                    quant: Some(quant_key.clone()),
                    mmproj: None,
                    context_length,
                    num_parallel: default_num_parallel(),
                    kv_unified: true,
                    enabled: true,
                    args: vec![],
                    sampling: None,
                    port: None,
                    health_check: None,
                    profile: None,
                    api_name: Some(repo_id.to_string()),
                    gpu_layers: None,
                    cache_type_k: None,
                    cache_type_v: None,
                    hf_format: None,
                    hf_base_model: None,
                    hf_pipeline_tag: None,
                    hf_total_params: None,
                    hf_active_params: None,
                    hf_architecture_type: None,
                    hf_context_length: None,
                    hf_num_layers: None,
                    hf_last_modified: None,
                    quants: std::collections::BTreeMap::new(),
                    modalities: modalities.clone(),
                    display_name: Some(display_name.clone()),
                    db_id: None, // will be set after reload_model_configs()
                    spec_decoding: Default::default(),
                });

        // Promote a stub entry (created by a prior mmproj-first pull) into a
        // real, enabled model once the main quant arrives. Without this, the
        // stub's `quant=None, enabled=false` would persist even though the
        // model file is now on disk.
        if entry.quant.is_none() {
            entry.quant = Some(quant_key);
            entry.enabled = true;
        }
        if entry.context_length.is_none() {
            entry.context_length = context_length;
        }
        if entry.modalities.is_none() {
            entry.modalities = modalities;
        }
        if entry.display_name.is_none() {
            entry.display_name = Some(display_name);
        }

        // Populate hf_* informational fields from GGUF metadata
        if let Some(meta) = gguf_metadata {
            entry.hf_architecture_type = meta.architecture.clone();
            entry.hf_context_length = meta.context_length.map(|v| v as u32);
            entry.hf_num_layers = meta.block_count.map(|v| v as u32);
        }

        // Save card (best-effort — download is already marked Completed)
        let _ = std::fs::create_dir_all(configs_dir);
        let _ = card.save(&card_path);

        return Some(model_key);
    }

    // For mmproj, still save the card.
    let _ = std::fs::create_dir_all(configs_dir);
    let _ = card.save(&card_path);

    let key = match existing_key {
        Some(k) => k,
        None => {
            // mmproj pulled before any main quant. Create a stub entry with
            // enabled=false so the file is tracked; the next main-quant pull
            // will find this entry by `model == repo_id` and flip enabled to
            // true. Without the stub, the mmproj file sits on disk invisible
            // to the editor.
            let display_name = crate::proxy::tama_handlers::generate_display_name(repo_id);
            let stub_key = repo_slug.to_lowercase();
            model_configs
                .entry(stub_key.clone())
                .or_insert_with(|| crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    gpu_variant: None,
                    model: Some(repo_id.to_string()),
                    quant: None,
                    mmproj: None,
                    context_length: None,
                    num_parallel: default_num_parallel(),
                    kv_unified: true,
                    enabled: false,
                    args: vec![],
                    sampling: None,
                    port: None,
                    health_check: None,
                    profile: None,
                    api_name: Some(repo_id.to_string()),
                    gpu_layers: None,
                    cache_type_k: None,
                    cache_type_v: None,
                    hf_format: None,
                    hf_base_model: None,
                    hf_pipeline_tag: None,
                    hf_total_params: None,
                    hf_active_params: None,
                    hf_architecture_type: None,
                    hf_context_length: None,
                    hf_num_layers: None,
                    hf_last_modified: None,
                    quants: std::collections::BTreeMap::new(),
                    modalities: None,
                    display_name: Some(display_name),
                    db_id: None,
                    spec_decoding: Default::default(),
                });
            stub_key
        }
    };

    // Wire mmproj + image modality onto the entry (stub or existing parent).
    if let Some(mc) = model_configs.get_mut(&key) {
        mc.mmproj = Some(spec.filename.clone());
        let modalities = mc.modalities.get_or_insert_with(Default::default);
        if !modalities
            .input
            .iter()
            .any(|m| m.eq_ignore_ascii_case("text"))
        {
            modalities.input.push("text".to_string());
        }
        if !modalities
            .input
            .iter()
            .any(|m| m.eq_ignore_ascii_case("image"))
        {
            modalities.input.push("image".to_string());
        }
        if modalities.output.is_empty() {
            modalities.output.push("text".to_string());
        }
    }
    Some(key)
}

/// Post-download: auto-create model card and config entries.
///
/// Called after a quant download completes. Updates the model card, saves config to
/// disk, and — critically — also inserts the new model entry into the live
/// `ProxyState.config` so it appears immediately in the models list without a restart.
///
/// Returns the integer model_configs id when the row was (re)saved, so the
/// caller can persist related rows (model_files) against it without a second
/// lookup that could miss due to case or key drift.
pub(crate) async fn setup_model_after_pull(
    state: Arc<ProxyState>,
    repo_id: &str,
    spec: &QuantDownloadSpec,
    dest_dir: &std::path::Path,
    gguf_metadata: Option<crate::models::pull::GgufMetadata>,
) -> Option<i64> {
    let _permit = state.config_write_semaphore.acquire().await.ok()?;
    // Clone needed data from config before awaiting — don't hold the read guard
    // across an awaited call to avoid blocking other writers/readers unnecessarily.
    let configs_dir = match state.config.read().await.configs_dir() {
        Ok(d) => d,
        Err(_) => return None,
    };
    // Config read guard is dropped here automatically when it goes out of scope.

    let mut model_configs = state.model_configs.write().await;
    let model_key = _setup_model_after_pull_with_config(
        &configs_dir,
        &mut model_configs,
        repo_id,
        spec,
        dest_dir,
        gguf_metadata.as_ref(),
    )
    .await;

    let mut saved_id: Option<i64> = None;
    if let Some(key) = model_key {
        if let Some(mgr) = state.model_mgr() {
            let save_result = model_configs
                .get(&key)
                .map(|mc| mgr.save_model_config(&key, mc));
            match save_result {
                Some(Ok(id)) => {
                    saved_id = Some(id);
                    if let Some(mc_mut) = model_configs.get_mut(&key) {
                        mc_mut.db_id = Some(id);
                    }
                }
                Some(Err(e)) => {
                    tracing::error!(key = %key, error = %e, "Failed to save model config to DB after pull");
                }
                None => {}
            }
        }
    }
    saved_id
    // _guard dropped here, releasing the lock
    // config write guard also dropped here, making the new model entry visible immediately
}
