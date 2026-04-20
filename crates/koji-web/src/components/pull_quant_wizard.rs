use leptos::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::utils::post_request;

use crate::components::pull_wizard::*;
use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;

// Re-export CompletedQuant for use in pages
use crate::components::pull_wizard::components::{
    context_step::ContextStep, done_step::DoneStep, download_step::DownloadStep,
    loading_step::LoadingStep, repo_input::RepoInput, selection_step::SelectionStep,
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
    let context_lengths = RwSignal::new(HashMap::<String, u32>::new());
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
            let ctx_map = context_lengths.get_untracked();
            let repo = repo_id.get_untracked();

            let completed: Vec<CompletedQuant> = jobs
                .into_iter()
                .filter(|j| j.status == "completed")
                .map(|j| {
                    let entry = quants_listing.iter().find(|q| q.filename == j.filename);
                    let quant = entry
                        .and_then(|e| e.quant.clone())
                        .or_else(|| infer_quant_from_filename(&j.filename));
                    let context_length = ctx_map.get(&j.filename).copied().unwrap_or(32768);
                    CompletedQuant {
                        repo_id: repo.clone(),
                        filename: j.filename.clone(),
                        quant,
                        size_bytes: Some(j.bytes_downloaded),
                        context_length,
                    }
                })
                .collect();

            cb.run(completed);
        });
    }

    // ── Done-step transition Effect ─────────────────────────────────────────
    // Watches download_jobs for terminal-state transitions and advances to
    // WizardStep::Done. This replaces the on_done callback that was previously
    // called inside a view closure.
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
            wizard_step.set(WizardStep::Done);
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
            context_lengths.set(std::collections::HashMap::new());
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
                let url = format!("/koji/v1/hf/{}", repo);
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
                    <div class=step_class(&step, &WizardStep::LoadingQuants, 1)>
                        "2. Loading"
                    </div>
                    <div class=step_class(&step, &WizardStep::SelectQuants, 2)>
                        "3. Select"
                    </div>
                    <div class=step_class(&step, &WizardStep::SetContext, 3)>
                        "4. Context"
                    </div>
                    <div class=step_class(&step, &WizardStep::Downloading, 4)>
                        "5. Download"
                    </div>
                    <div class=step_class(&step, &WizardStep::Done, 5)>
                        "6. Done"
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
                            context_lengths.set(std::collections::HashMap::new());
                            available_quants.set(Vec::new());
                            wizard_step.set(WizardStep::LoadingQuants);
                            wasm_bindgen_futures::spawn_local(async move {
                                let url = format!("/koji/v1/hf/{}", rid);
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
                        })
                    />
                }.into_any(),

                WizardStep::LoadingQuants => view! {
                    <LoadingStep />
                }.into_any(),

                WizardStep::SelectQuants => view! {
                    <SelectionStep
                        repo_id=repo_id.into()
                        available_quants=available_quants.into()
                        available_mmprojs=available_mmprojs.into()
                        selected_filenames=selected_filenames
                        selected_mmproj_filenames=selected_mmproj_filenames
                        on_next=Callback::new(move |_| {
                            let sel = selected_filenames.get();
                            let mut ctx = HashMap::new();
                            for fname in &sel {
                                ctx.insert(fname.clone(), 32768u32);
                            }
                            context_lengths.set(ctx);
                            wizard_step.set(WizardStep::SetContext);
                        })
                        on_back=Callback::new(move |_| {
                            wizard_step.set(WizardStep::RepoInput);
                        })
                    />
                }.into_any(),

                WizardStep::SetContext => view! {
                    <ContextStep
                        selected_filenames=selected_filenames.into()
                        available_quants=available_quants.into()
                        context_lengths=context_lengths
                        on_next=Callback::new(move |_| {
                            let rid = repo_id.get();
                            let sel = selected_filenames.get();
                            let quants_list = available_quants.get();
                            let ctx_map = context_lengths.get();

                            let mut quants: Vec<QuantRequest> = sel.iter()
                                .filter_map(|fname| {
                                    let entry = quants_list.iter().find(|q| &q.filename == fname)?;
                                    let ctx = ctx_map.get(fname).copied().unwrap_or(32768);
                                    Some(QuantRequest {
                                        filename: fname.clone(),
                                        quant: entry.quant.clone(),
                                        context_length: ctx,
                                    })
                                })
                                .collect();

                            let available_mmprojs_list = available_mmprojs.get();
                            let selected_mmprojs: Vec<QuantRequest> = selected_mmproj_filenames
                                .get()
                                .iter()
                                .filter_map(|fname| {
                                    let entry = available_mmprojs_list.iter().find(|q| &q.filename == fname)?;
                                    Some(QuantRequest {
                                        filename: fname.clone(),
                                        quant: entry.quant.clone(),
                                        context_length: 32768,
                                    })
                                })
                                .collect();

                            quants.extend(selected_mmprojs);

                            let body = PullRequest { repo_id: rid, quants };

                            wasm_bindgen_futures::spawn_local(async move {
                                let build_result = post_request("/koji/v1/pulls")
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

                                                // Open SSE stream for each job with per-job reconnection.
                                                for entry in entries {
                                                    let job_id_str = entry.job_id.clone();
                                                    let dj = download_jobs;
                                                    let ws = wizard_step;
                                                    let cancel = cancelled;

                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        // Exponential backoff constants for reconnection.
                                                        const INITIAL_DELAY_MS: u32 = 1_000;
                                                        const MAX_DELAY_MS: u32 = 30_000;

                                                        let mut delay_ms: u32 = INITIAL_DELAY_MS;

                                                        loop {
                                                            if cancel.get_untracked() {
                                                                break;
                                                            }

                                                            let url = format!("/koji/v1/pulls/{}/stream", job_id_str);
                                                            let mut es = match EventSource::new(&url) {
                                                                Ok(es) => es,
                                                                Err(e) => {
                                                                    let msg = format!("{e:?}");
                                                                    dj.update(|jobs| {
                                                                        if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                                                            j.status = "failed".to_string();
                                                                            j.error = Some(format!("Failed to open SSE stream: {msg}"));
                                                                        }
                                                                    });
                                                                    advance_if_all_terminal(&dj, &ws);
                                                                    delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                                                                    gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                                                                    continue;
                                                                }
                                                            };

                                                            delay_ms = INITIAL_DELAY_MS;

                                                            let mut progress_stream = match es.subscribe("progress") {
                                                                Ok(s) => s,
                                                                Err(e) => {
                                                                    let msg = format!("{e:?}");
                                                                    dj.update(|jobs| {
                                                                        if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                                                            j.status = "failed".to_string();
                                                                            j.error = Some(format!("Failed to subscribe to progress events: {msg}"));
                                                                        }
                                                                    });
                                                                    es.close();
                                                                    advance_if_all_terminal(&dj, &ws);
                                                                    delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                                                                    gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                                                                    continue;
                                                                }
                                                            };

                                                            let mut done_stream = match es.subscribe("done") {
                                                                Ok(s) => s,
                                                                Err(e) => {
                                                                    let msg = format!("{e:?}");
                                                                    dj.update(|jobs| {
                                                                        if let Some(j) = jobs.iter_mut().find(|j| j.job_id == job_id_str) {
                                                                            j.status = "failed".to_string();
                                                                            j.error = Some(format!("Failed to subscribe to done events: {msg}"));
                                                                        }
                                                                    });
                                                                    es.close();
                                                                    advance_if_all_terminal(&dj, &ws);
                                                                    delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                                                                    gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                                                                    continue;
                                                                }
                                                            };

                                                            loop {
                                                                if cancel.get_untracked() {
                                                                    es.close();
                                                                    return;
                                                                }

                                                                let next_progress = progress_stream.next();
                                                                let next_done = done_stream.next();
                                                                futures_util::pin_mut!(next_progress, next_done);

                                                                match futures_util::future::select(next_progress, next_done).await {
                                                                    futures_util::future::Either::Left((Some(Ok((_, msg))), _)) => {
                                                                        let data = msg.data().as_string().unwrap_or_default();
                                                                        if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
                                                                            dj.update(|jobs| {
                                                                                if let Some(j) = jobs.iter_mut().find(|j| j.job_id == p.job_id) {
                                                                                    j.bytes_downloaded = p.bytes_downloaded;
                                                                                    j.total_bytes = p.total_bytes;
                                                                                    j.status = p.status.clone();
                                                                                    j.error = p.error.clone();
                                                                                }
                                                                            });
                                                                        }
                                                                    }
                                                                    futures_util::future::Either::Right((Some(Ok((_, msg))), _)) => {
                                                                        let data = msg.data().as_string().unwrap_or_default();
                                                                        if let Ok(p) = serde_json::from_str::<SsePayload>(&data) {
                                                                            dj.update(|jobs| {
                                                                                if let Some(j) = jobs.iter_mut().find(|j| j.job_id == p.job_id) {
                                                                                    j.bytes_downloaded = p.bytes_downloaded;
                                                                                    j.total_bytes = p.total_bytes;
                                                                                    j.status = p.status.clone();
                                                                                    j.error = p.error.clone();
                                                                                }
                                                                            });
                                                                        }
                                                                        es.close();
                                                                        advance_if_all_terminal(&dj, &ws);
                                                                        break;
                                                                    }
                                                                    _ => {
                                                                        es.close();
                                                                        advance_if_all_terminal(&dj, &ws);
                                                                        break;
                                                                    }
                                                                }
                                                            }

                                                            // Stream ended — reconnect with backoff.
                                                            if !cancel.get_untracked() {
                                                                gloo_timers::future::TimeoutFuture::new(delay_ms).await;
                                                                delay_ms = (delay_ms * 2).min(MAX_DELAY_MS);
                                                            }
                                                        }
                                                    });
                                                }
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
                            wizard_step.set(WizardStep::SelectQuants);
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
    let jobs = dj.get();
    if !jobs.is_empty()
        && jobs
            .iter()
            .all(|j| j.status == "completed" || j.status == "failed")
    {
        ws.set(WizardStep::Done);
    }
}
