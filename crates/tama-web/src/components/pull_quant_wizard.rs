use leptos::prelude::*;
use std::collections::HashSet;

use crate::utils::{post_request, put_request};

use crate::components::pull_wizard::*;

#[cfg(not(feature = "ssr"))]
use futures_util::StreamExt;

#[cfg(not(feature = "ssr"))]
use crate::utils::sse_stream;

// Re-export CompletedQuant for use in pages
use crate::components::pull_wizard::components::{
    context_step::ContextStep, done_step::DoneStep, download_step::DownloadStep,
    repo_input::RepoInput, selection_step::SelectionStep,
};
pub use crate::components::pull_wizard::CompletedQuant;

#[component]
pub fn PullQuantWizard(
    /// Pre-set HF repo ID. If non-empty AND `is_open` transitions to true,
    /// the wizard skips step 1 and immediately fetches quants. If empty,
    /// the wizard starts at the repo-input step.
    #[prop(into)]
    initial_repo: Signal<String>,

    /// Whether the wizard is currently visible. Convention: `None` means
    /// "hosted directly on a page, always visible, never auto-reset" — the
    /// reset Effect is not registered. `Some(signal)` enables the modal
    /// lifecycle where (closed → open) transitions drive reset/refetch.
    #[prop(optional)]
    is_open: Option<Signal<bool>>,

    /// Called once after all downloads in the current session reach a terminal
    /// state. Receives the list of quants that completed successfully (failed
    /// jobs are filtered out). Fires exactly once per session, guarded by
    /// `did_complete`.
    #[prop(optional)]
    on_complete: Option<Callback<Vec<CompletedQuant>>>,

    /// Called when the user dismisses via in-step Cancel/Hide/Close button.
    /// Wizard never hides itself — host decides what happens.
    #[prop(optional)]
    on_close: Option<Callback<()>>,
) -> impl IntoView {
    // ── Signals ──────────────────────────────────────────────────────────────
    let wizard_step = RwSignal::new(WizardStep::RepoInput);
    let repo_id = RwSignal::new(String::new());
    let available_quants = RwSignal::new(Vec::<QuantEntry>::new());
    let available_mmprojs = RwSignal::new(Vec::<QuantEntry>::new());
    let selected_filenames = RwSignal::new(HashSet::<String>::new());
    let selected_mmproj_filenames = RwSignal::new(HashSet::<String>::new());
    let gguf_context_length = RwSignal::new(None::<u64>);
    let context_settings = RwSignal::new(ContextSettings::default());
    let model_id = RwSignal::new(None::<u32>);
    let hf_metadata = RwSignal::new(HfModelMetadata::default());
    let download_jobs = RwSignal::new(Vec::<JobProgress>::new());
    let error_msg = RwSignal::new(Option::<String>::None);
    let did_complete = RwSignal::new(false);

    // ── Cancel flag: flipped on component unmount ───────────────────────────
    let cancelled = RwSignal::new(false);
    on_cleanup(move || {
        cancelled.set(true);
    });

    // ── on_complete Effect (only if on_complete is Some) ─────────────────────
    // Watches download_jobs signal for terminal state transitions.
    // Moved out of the view closure to avoid calling during render.
    if let Some(cb) = on_complete {
        Effect::new(move |_| {
            let step = wizard_step.get();
            if step != WizardStep::Done {
                return;
            }
            if did_complete.get_untracked() {
                return;
            }
            did_complete.set(true);

            let jobs = download_jobs.get_untracked();
            let quants_listing = available_quants.get_untracked();
            let repo = repo_id.get_untracked();

            let completed: Vec<CompletedQuant> = jobs
                .into_iter()
                .filter(|j| j.status == "completed")
                .map(|j| {
                    let entry = quants_listing.iter().find(|q| q.filename == j.filename);
                    let quant = entry
                        .and_then(|e| e.quant.clone())
                        .or_else(|| infer_quant_from_filename(&j.filename));
                    CompletedQuant {
                        repo_id: repo.clone(),
                        filename: j.filename.clone(),
                        quant,
                        size_bytes: Some(j.bytes_downloaded),
                    }
                })
                .collect();

            cb.run(completed);
        });
    }

    // ── Downloading → SetContext transition Effect ──────────────────────────
    // Watches download_jobs for terminal-state transitions and advances to
    // WizardStep::SetContext so the user can configure model settings.
    Effect::new(move |_| {
        let jobs = download_jobs.get();
        if jobs.is_empty() {
            return;
        }
        let all_terminal = jobs
            .iter()
            .all(|j| j.status == "completed" || j.status == "failed");
        if !all_terminal {
            return;
        }
        // Only transition if we're currently on the Downloading step.
        let current_step = wizard_step.get();
        if current_step == WizardStep::Downloading {
            wizard_step.set(WizardStep::SetContext);
        }
    });

    // ── Reset Effect (only if is_open is Some) ──────────────────────────────
    if let Some(is_open_sig) = is_open {
        Effect::new(move |_| {
            let open = is_open_sig.get();
            if !open {
                return;
            }
            let step = wizard_step.get_untracked();
            if !matches!(step, WizardStep::RepoInput | WizardStep::Done) {
                return;
            }
            selected_filenames.set(std::collections::HashSet::new());
            selected_mmproj_filenames.set(std::collections::HashSet::new());
            gguf_context_length.set(None);
            model_id.set(None);
            hf_metadata.set(HfModelMetadata::default());
            context_settings.set(ContextSettings::default());
            download_jobs.set(Vec::new());
            error_msg.set(None);
            did_complete.set(false);
            wizard_step.set(WizardStep::RepoInput);

            let repo = initial_repo.get_untracked();
            if repo.trim().is_empty() {
                return;
            }
            repo_id.set(repo.clone());
            wizard_step.set(WizardStep::LoadingQuants);

            wasm_bindgen_futures::spawn_local(async move {
                let url = format!("/tama/v1/hf/{}", repo);
                match gloo_net::http::Request::get(&url).send().await {
                    Ok(resp) => match resp.json::<Vec<QuantEntry>>().await {
                        Ok(quants) => {
                            if quants.is_empty() {
                                error_msg.set(Some(
                                    "No GGUF files found for this repo. Check the repo ID and try again.".to_string(),
                                ));
                                wizard_step.set(WizardStep::RepoInput);
                            } else {
                                let mut model_quants: Vec<QuantEntry> = Vec::new();
                                let mut mmprojs: Vec<QuantEntry> = Vec::new();
                                for q in quants {
                                    if q.kind == QuantKind::Mmproj {
                                        mmprojs.push(q);
                                    } else {
                                        model_quants.push(q);
                                    }
                                }
                                available_quants.set(model_quants);
                                available_mmprojs.set(mmprojs);
                                wizard_step.set(WizardStep::SelectQuants);
                            }
                        }
                        Err(e) => {
                            error_msg.set(Some(format!("Failed to parse response: {e}")));
                            wizard_step.set(WizardStep::RepoInput);
                        }
                    },
                    Err(e) => {
                        error_msg.set(Some(format!("Request failed: {e}")));
                        wizard_step.set(WizardStep::RepoInput);
                    }
                }
            });
        });
    }

    // ── Step dispatch ───────────────────────────────────────────────────────
    view! {
        <div class="wizard-steps mb-3">
            {move || {
                let step = wizard_step.get();
                let show_repo_step = initial_repo.get().trim().is_empty();
                view! {
                    {show_repo_step.then(|| view! {
                        <div class=step_class(&step, &WizardStep::RepoInput, 0)>
                            "1. Repo"
                        </div>
                    })}
                    <div class=step_class(&step, &WizardStep::SelectQuants, 1)>
                        "2. Select"
                    </div>
                    <div class=step_class(&step, &WizardStep::Downloading, 2)>
                        "3. Download"
                    </div>
                    <div class=step_class(&step, &WizardStep::SetContext, 3)>
                        "4. Configure"
                    </div>
                    <div class=step_class(&step, &WizardStep::Done, 4)>
                        "5. Done"
                    </div>
                }
            }}
        </div>

        <div class="card">
            {move || match wizard_step.get() {
                WizardStep::RepoInput => view! {
                    <RepoInput
                        repo_id=repo_id
                        error_msg=error_msg
                        on_close=on_close
                        on_search=Callback::new(move |rid| {
                            error_msg.set(None);
                            selected_filenames.set(std::collections::HashSet::new());
                            gguf_context_length.set(None);
                            model_id.set(None);
                            context_settings.set(ContextSettings::default());
                            hf_metadata.set(HfModelMetadata::default());
                            available_quants.set(Vec::new());
                            // Fetch quants + metadata in parallel, then create stub with metadata
                            wasm_bindgen_futures::spawn_local(async move {
                                let quants_url = format!("/tama/v1/hf/{}", rid);
                                let metadata_url = format!("/tama/v1/hf/{}/metadata", rid);
                                let quants_future = gloo_net::http::Request::get(&quants_url).send();
                                let metadata_future =
                                    gloo_net::http::Request::get(&metadata_url).send();

                                let (quants_resp, metadata_resp) =
                                    futures_util::join!(quants_future, metadata_future);

                                // Parse metadata (soft failure — stub still created without it)
                                let metadata = match metadata_resp {
                                    Ok(r) if (200..300).contains(&r.status()) => {
                                        match r.json::<HfModelMetadata>().await {
                                            Ok(m) => Some(m),
                                            Err(e) => {
                                                log::warn!("Failed to parse metadata: {}", e);
                                                None
                                            }
                                        }
                                    }
                                    _ => {
                                        log::warn!("Failed to fetch metadata for '{}'", rid);
                                        None
                                    }
                                };

                                // Create stub model with metadata
                                let stub_body = serde_json::json!({
                                    "repo_id": &rid,
                                    "backend": "llama_cpp",
                                    "metadata": metadata,
                                });
                                let stub_resp = post_request("/tama/v1/models")
                                    .json(&stub_body)
                                    .unwrap()
                                    .send()
                                    .await;

                                // Handle stub creation response
                                match stub_resp {
                                    Ok(r) if (200..300).contains(&r.status()) => {
                                        if let Ok(json) = r.json::<serde_json::Value>().await {
                                            if let Some(id) = json.get("id").and_then(|v| v.as_u64()) {
                                                model_id.set(Some(id as u32));
                                            }
                                        }
                                    }
                                    _ => {
                                        log::warn!("Failed to create stub model for '{}'", rid);
                                    }
                                }

                                // Store metadata for later use
                                if let Some(m) = metadata {
                                    hf_metadata.set(m);
                                }

                                // Handle quant list response
                                match quants_resp {
                                    Ok(resp) => match resp.json::<Vec<QuantEntry>>().await {
                                        Ok(quants) => {
                                            if quants.is_empty() {
                                                error_msg.set(Some(
                                                    "No GGUF files found for this repo. Check the repo ID and try again.".to_string(),
                                                ));
                                                wizard_step.set(WizardStep::RepoInput);
                                            } else {
                                                let mut model_quants: Vec<QuantEntry> = Vec::new();
                                                let mut mmprojs: Vec<QuantEntry> = Vec::new();
                                                for q in quants {
                                                    if q.kind == QuantKind::Mmproj {
                                                        mmprojs.push(q);
                                                    } else {
                                                        model_quants.push(q);
                                                    }
                                                }
                                                available_quants.set(model_quants);
                                                available_mmprojs.set(mmprojs);
                                                wizard_step.set(WizardStep::SelectQuants);
                                            }
                                        }
                                        Err(e) => {
                                            error_msg.set(Some(format!("Failed to parse response: {e}")));
                                            wizard_step.set(WizardStep::RepoInput);
                                        }
                                    },
                                    Err(e) => {
                                        error_msg.set(Some(format!("Request failed: {e}")));
                                        wizard_step.set(WizardStep::RepoInput);
                                    }
                                }
                            });
                        })
                    />
                }.into_any(),

                WizardStep::LoadingQuants => {
                    // Folded into RepoInput — stub model created during search.
                    // This arm is unreachable in normal flow, retained for safety.
                    view! { <div></div> }.into_any()
                },

                WizardStep::SelectQuants => view! {
                    <SelectionStep
                        repo_id=repo_id.into()
                        available_quants=available_quants.into()
                        available_mmprojs=available_mmprojs.into()
                        selected_filenames=selected_filenames
                        selected_mmproj_filenames=selected_mmproj_filenames
                        on_next=Callback::new(move |_| {
                            let rid = repo_id.get();
                            let filenames: Vec<String> = selected_filenames.get().into_iter().collect();
                            let mmproj_filenames: Vec<String> = selected_mmproj_filenames
                                .get()
                                .into_iter()
                                .collect();

                            let body = PullRequest {
                                repo_id: rid,
                                model_id: model_id.get_untracked(),
                                filenames,
                                mmproj_filenames,
                            };

                            wasm_bindgen_futures::spawn_local(async move {
                                let build_result = post_request("/tama/v1/pulls")
                                    .json(&body);
                                let resp = match build_result {
                                    Ok(req) => req.send().await,
                                    Err(e) => {
                                        error_msg.set(Some(format!("Failed to build request: {e}")));
                                        return;
                                    }
                                };
                                match resp {
                                    Ok(r) => {
                                        match r.json::<Vec<PullJobEntry>>().await {
                                            Ok(entries) => {
                                                let jobs: Vec<JobProgress> = entries
                                                    .iter()
                                                    .map(|e| JobProgress {
                                                        job_id: e.job_id.clone(),
                                                        filename: e.filename.clone(),
                                                        status: e.status.clone(),
                                                        bytes_downloaded: 0,
                                                        total_bytes: None,
                                                        error: None,
                                                    })
                                                    .collect();
                                                download_jobs.set(jobs);
                                                wizard_step.set(WizardStep::Downloading);

                                                // Open SSE stream for each job with per-job reconnection via sse_stream.
                                                #[cfg(not(feature = "ssr"))]
                                                spawn_sse_streams(entries.clone(), download_jobs.clone(), wizard_step.clone(), cancelled.clone(), gguf_context_length.clone());
                                                // Polling fallback: poll job status every 2s to catch any SSE events missed.
                                                #[cfg(not(feature = "ssr"))]
                                                spawn_poll_fallback(entries, download_jobs, wizard_step, cancelled, gguf_context_length);
                                                #[cfg(feature = "ssr")]
                                                let _ = entries;
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!("Failed to parse response: {e}")));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error_msg.set(Some(format!("Request failed: {e}")));
                                    }
                                }
                            });
                        })
                        on_back=Callback::new(move |_| {
                            wizard_step.set(WizardStep::RepoInput);
                        })
                    />
                }.into_any(),

                WizardStep::SetContext => view! {
                    <ContextStep
                        gguf_context_length=gguf_context_length.into()
                        download_jobs=download_jobs.into()
                        settings=context_settings
                        on_next=Callback::new(move |_| {
                            let settings = context_settings.get();
                            let mid = model_id.get_untracked();
                            let repo = repo_id.get_untracked();

                            wasm_bindgen_futures::spawn_local(async move {
                                let payload = serde_json::json!({
                                    "backend": "llama_cpp",
                                    "context_length": settings.context_length,
                                    "kv_unified": Some(settings.kv_unified),
                                    "cache_type_k": settings.cache_type_k,
                                    "cache_type_v": settings.cache_type_v,
                                });

                                // Use numeric DB id for the PUT
                                let model_key = if let Some(id) = mid {
                                    id.to_string()
                                } else {
                                    repo.replace('/', "--").to_lowercase()
                                };

                                match put_request(&format!("/tama/v1/models/{}", model_key))
                                    .json(&payload)
                                {
                                    Ok(req) => {
                                        match req.send().await {
                                            Ok(resp) => {
                                                if resp.status() < 400 {
                                                    wizard_step.set(WizardStep::Done);
                                                } else {
                                                    error_msg.set(Some(format!("Failed to save settings (HTTP {})", resp.status())));
                                                }
                                            }
                                            Err(e) => {
                                                error_msg.set(Some(format!("Failed to save settings: {}", e)));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error_msg.set(Some(format!("Failed to build request: {}", e)));
                                    }
                                }
                            });
                        })
                        on_back=Callback::new(move |_| {
                            wizard_step.set(WizardStep::Downloading);
                        })
                    />
                }.into_any(),

                WizardStep::Downloading => view! {
                    <DownloadStep
                        download_jobs=download_jobs.into()
                        on_close=on_close
                        error_msg=error_msg
                    />
                }.into_any(),

                WizardStep::Done => view! {
                    <DoneStep
                        download_jobs=download_jobs.into()
                        on_close=on_close
                    />
                }.into_any(),
            }}
        </div>
    }
}

/// Helper: advance to Done step when all jobs are in a terminal state.
fn advance_if_all_terminal(dj: &RwSignal<Vec<JobProgress>>, ws: &RwSignal<WizardStep>) {
    let jobs = dj.get_untracked();
    if !jobs.is_empty()
        && jobs
            .iter()
            .all(|j| j.status == "completed" || j.status == "failed")
    {
        ws.set(WizardStep::Done);
    }
}

/// Polling fallback: poll job status via REST GET every 2s.
/// Catches any SSE events missed (e.g., job completes before SSE connects).
#[cfg(not(feature = "ssr"))]
fn spawn_poll_fallback(
    entries: Vec<PullJobEntry>,
    dj: RwSignal<Vec<JobProgress>>,
    ws: RwSignal<WizardStep>,
    cancel: RwSignal<bool>,
    gguf_ctx: RwSignal<Option<u64>>,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let job_ids: Vec<String> = entries.iter().map(|e| e.job_id.clone()).collect();
        web_sys::console::log_1(&format!("[poll] started for {} jobs", job_ids.len()).into());

        loop {
            if cancel.get_untracked() {
                break;
            }

            // Check if all jobs are terminal — if so, stop polling
            let all_terminal = dj.with_untracked(|jobs| {
                jobs.iter().all(|j| {
                    let id_match = job_ids.contains(&j.job_id);
                    id_match && (j.status == "completed" || j.status == "failed")
                })
            });
            web_sys::console::log_1(
                &format!(
                    "[poll] check: all_terminal={}, jobs={}",
                    all_terminal,
                    job_ids.len()
                )
                .into(),
            );
            if all_terminal && !job_ids.is_empty() {
                web_sys::console::log_1("[poll] all terminal, stopping".into());
                break;
            }

            gloo_timers::future::TimeoutFuture::new(2000).await;

            // Poll each non-terminal job
            for job_id in &job_ids {
                if cancel.get_untracked() {
                    break;
                }

                // Check if this job is already terminal
                let is_terminal = dj.with_untracked(|jobs| {
                    jobs.iter().any(|j| {
                        j.job_id == *job_id && (j.status == "completed" || j.status == "failed")
                    })
                });
                if is_terminal {
                    continue;
                }

                // Poll via REST GET
                web_sys::console::log_1(&format!("[poll] polling {}", job_id).into());
                match gloo_net::http::Request::get(&format!("/tama/v1/pulls/{}", job_id))
                    .send()
                    .await
                {
                    Ok(resp) if (200..300).contains(&resp.status()) => {
                        if let Ok(p) = resp.json::<SsePayload>().await {
                            let old_status = dj.with_untracked(|jobs| {
                                jobs.iter()
                                    .find(|j| j.job_id == p.job_id)
                                    .map(|j| j.status.clone())
                            });
                            dj.update(|jobs| {
                                if let Some(j) = jobs.iter_mut().find(|j| j.job_id == p.job_id) {
                                    j.bytes_downloaded = p.bytes_downloaded;
                                    j.total_bytes = p.total_bytes;
                                    j.status = p.status.clone();
                                    j.error = p.error.clone();
                                }
                            });
                            if let Some(ctx) = p.gguf_context_length {
                                gguf_ctx.set(Some(ctx));
                            }
                            // Log status changes for debugging
                            if old_status.as_deref() != Some(&p.status) {
                                web_sys::console::log_1(
                                    &format!(
                                        "[poll] {} {} -> {}",
                                        job_id,
                                        old_status.unwrap_or_default(),
                                        p.status
                                    )
                                    .into(),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        web_sys::console::warn_1(&format!("[poll] GET failed: {}", e).into());
                    }
                    _ => {}
                }
            }
        }

        // After polling stops, check if all jobs are terminal and advance
        advance_if_all_terminal(&dj, &ws);
    });
}

/// Spawn an SSE stream per download job using sse_stream with max 10 reconnect attempts.
#[cfg(not(feature = "ssr"))]
fn spawn_sse_streams(
    entries: Vec<PullJobEntry>,
    dj: RwSignal<Vec<JobProgress>>,
    ws: RwSignal<WizardStep>,
    cancel: RwSignal<bool>,
    gguf_ctx: RwSignal<Option<u64>>,
) {
    for entry in entries {
        let job_id_str = entry.job_id.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let config = sse_stream::SseReconnectConfig {
                max_attempts: Some(10),
                ..Default::default()
            };
            let url = format!("/tama/v1/pulls/{}/stream", job_id_str);
            let conn = sse_stream::create(url, cancel, Some(config));

            loop {
                if cancel.get_untracked() {
                    break;
                }

                // Connect (or reconnect) with exponential backoff
                match conn.connect_once().await {
                    Ok(()) => {
                        // Reset error on successful connection
                        dj.update(|jobs| {
                            if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                j.error = None;
                                if j.status == "reconnecting" {
                                    j.status = "downloading".to_string();
                                }
                            }
                        });

                        // Subscribe to channels
                        let mut progress_stream = match conn.subscribe("progress") {
                            Ok(s) => s,
                            Err(_) => {
                                continue; // connection dropped, loop back
                            }
                        };
                        let mut done_stream = match conn.subscribe("done") {
                            Ok(s) => s,
                            Err(_) => {
                                continue; // connection dropped, loop back
                            }
                        };

                        // Inner event processing loop
                        let mut job_done = false;
                        loop {
                            if cancel.get_untracked() {
                                break;
                            }

                            let next_progress = progress_stream.next();
                            let next_done = done_stream.next();
                            futures_util::pin_mut!(next_progress, next_done);

                            match futures_util::future::select(next_progress, next_done).await {
                                futures_util::future::Either::Left((Some(Ok(event)), _)) => {
                                    let data = event.data.clone();
                                    if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
                                        dj.update(|jobs| {
                                            if let Some(j) =
                                                jobs.iter_mut().find(|j| j.job_id == p.job_id)
                                            {
                                                j.bytes_downloaded = p.bytes_downloaded;
                                                j.total_bytes = p.total_bytes;
                                                j.status = p.status.clone();
                                                j.error = p.error.clone();
                                            }
                                        });
                                        // Capture GGUF context_length from any job (they're all the same model)
                                        if let Some(ctx) = p.gguf_context_length {
                                            gguf_ctx.set(Some(ctx));
                                        }
                                    }
                                }
                                futures_util::future::Either::Right((Some(Ok(event)), _)) => {
                                    let data = event.data.clone();
                                    if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
                                        dj.update(|jobs| {
                                            if let Some(j) =
                                                jobs.iter_mut().find(|j| j.job_id == p.job_id)
                                            {
                                                j.bytes_downloaded = p.bytes_downloaded;
                                                j.total_bytes = p.total_bytes;
                                                j.status = p.status.clone();
                                                j.error = p.error.clone();
                                            }
                                        });
                                        // Capture GGUF context_length from any job (they're all the same model)
                                        if let Some(ctx) = p.gguf_context_length {
                                            gguf_ctx.set(Some(ctx));
                                        }
                                    }
                                    // Done event received — mark job as complete
                                    web_sys::console::log_1(
                                        &format!("[sse] done event for {}", job_id_str).into(),
                                    );
                                    job_done = true;
                                    break;
                                }
                                _ => {
                                    // Stream ended — loop back for reconnect
                                    break;
                                }
                            }
                        }
                        // If the job received a "done" event, stop reconnecting.
                        // Otherwise (stream ended unexpectedly), outer loop reconnects.
                        if job_done {
                            break;
                        }
                    }
                    Err(e) => {
                        // Max attempts exhausted — mark as terminal failure
                        dj.update(|jobs| {
                            if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                j.status = "failed".to_string();
                                j.error = Some(format!(
                                    "SSE stream for job {} failed after max attempts: {e}",
                                    job_id_str
                                ));
                            }
                        });
                        advance_if_all_terminal(&dj, &ws);
                        break;
                    }
                }
            }
        });
    }
}
