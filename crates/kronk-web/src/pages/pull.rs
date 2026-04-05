use futures_util::StreamExt;
use gloo_net::eventsource::futures::EventSource;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── Local types ──────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone, Debug)]
struct QuantEntry {
    filename: String,
    quant: Option<String>,
    size_bytes: Option<i64>,
}

#[derive(Clone, Debug)]
struct JobProgress {
    job_id: String,
    filename: String,
    status: String,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
}

/// Returned by `POST /kronk/v1/pulls` (each element of the array)
#[derive(Deserialize, Clone)]
struct PullJobEntry {
    job_id: String,
    filename: String,
    status: String,
}

/// SSE event data payload
#[derive(Deserialize, Clone)]
struct SsePayload {
    job_id: String,
    status: String,
    bytes_downloaded: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
}

// ── Wizard step enum ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
enum WizardStep {
    RepoInput,
    LoadingQuants,
    SelectQuants,
    SetContext,
    Downloading,
    Done,
}

// ── Helper ───────────────────────────────────────────────────────────────────

fn format_bytes(bytes: i64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GiB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{bytes} B")
    }
}

// ── Request body type ────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PullRequest {
    repo_id: String,
    quants: Vec<QuantRequest>,
}

#[derive(Serialize)]
struct QuantRequest {
    filename: String,
    quant: Option<String>,
    context_length: u32,
}

// ── Wizard step indicator helper ─────────────────────────────────────────────

fn step_class(current: &WizardStep, target: &WizardStep, target_idx: usize) -> &'static str {
    let order = [
        WizardStep::RepoInput,
        WizardStep::LoadingQuants,
        WizardStep::SelectQuants,
        WizardStep::SetContext,
        WizardStep::Downloading,
        WizardStep::Done,
    ];
    let current_idx = order.iter().position(|s| s == current).unwrap_or(0);
    if current == target {
        "wizard-step active"
    } else if current_idx > target_idx {
        "wizard-step completed"
    } else {
        "wizard-step"
    }
}

// ── Component ────────────────────────────────────────────────────────────────

#[component]
pub fn Pull() -> impl IntoView {
    // ── Signals ──────────────────────────────────────────────────────────────
    let wizard_step = RwSignal::new(WizardStep::RepoInput);
    let repo_id = RwSignal::new(String::new());
    let available_quants = RwSignal::new(Vec::<QuantEntry>::new());
    let selected_filenames = RwSignal::new(HashSet::<String>::new());
    let context_lengths = RwSignal::new(HashMap::<String, u32>::new());
    let download_jobs = RwSignal::new(Vec::<JobProgress>::new());
    let error_msg = RwSignal::new(Option::<String>::None);

    // ── Step dispatch ─────────────────────────────────────────────────────────
    view! {
        <div class="page-header">
            <h1>"Pull Model"</h1>
        </div>

        // Wizard step indicator
        <div class="wizard-steps mb-3">
            {move || {
                let step = wizard_step.get();
                view! {
                    <div class=step_class(&step, &WizardStep::RepoInput, 0)>"1. Repo"</div>
                    <div class=step_class(&step, &WizardStep::LoadingQuants, 1)>"2. Loading"</div>
                    <div class=step_class(&step, &WizardStep::SelectQuants, 2)>"3. Select"</div>
                    <div class=step_class(&step, &WizardStep::SetContext, 3)>"4. Context"</div>
                    <div class=step_class(&step, &WizardStep::Downloading, 4)>"5. Download"</div>
                    <div class=step_class(&step, &WizardStep::Done, 5)>"6. Done"</div>
                }
            }}
        </div>

        <div class="form-card card">
            {move || match wizard_step.get() {
                // ── Step 1: Repo Input ────────────────────────────────────────
                WizardStep::RepoInput => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Enter Repository"</h2>
                        <p class="form-card__desc text-muted">
                            "Enter a HuggingFace repo ID to search for available quantisations."
                        </p>
                    </div>

                    {move || error_msg.get().map(|e| view! {
                        <div class="alert alert--error mb-2">
                            <span class="alert__icon">"✕"</span>
                            <span>{e}</span>
                        </div>
                    })}

                    <div class="form-group">
                        <label class="form-label" for="repo-id">"Repo ID"</label>
                        <input
                            id="repo-id"
                            class="form-input"
                            type="text"
                            prop:value=move || repo_id.get()
                            on:input=move |e| repo_id.set(event_target_value(&e))
                            placeholder="e.g. meta-llama/Llama-3.2-1B"
                        />
                    </div>

                    <div class="form-actions mt-3">
                        <button
                            class="btn btn-primary"
                            on:click=move |_| {
                                let rid = repo_id.get();
                                if rid.is_empty() { return; }
                                // Clear any stale per-repo state from a previous search.
                                error_msg.set(None);
                                selected_filenames.set(std::collections::HashSet::new());
                                context_lengths.set(std::collections::HashMap::new());
                                available_quants.set(Vec::new());
                                wizard_step.set(WizardStep::LoadingQuants);
                                wasm_bindgen_futures::spawn_local(async move {
                                    let url = format!("/kronk/v1/hf/{}", rid);
                                    match gloo_net::http::Request::get(&url).send().await {
                                        Ok(resp) => {
                                            match resp.json::<Vec<QuantEntry>>().await {
                                                Ok(quants) => {
                                                    if quants.is_empty() {
                                                        error_msg.set(Some(
                                                            "No GGUF files found for this repo. Check the repo ID and try again.".to_string()
                                                        ));
                                                        wizard_step.set(WizardStep::RepoInput);
                                                    } else {
                                                        available_quants.set(quants);
                                                        wizard_step.set(WizardStep::SelectQuants);
                                                    }
                                                }
                                                Err(e) => {
                                                    error_msg.set(Some(format!("Failed to parse response: {e}")));
                                                    wizard_step.set(WizardStep::RepoInput);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error_msg.set(Some(format!("Request failed: {e}")));
                                            wizard_step.set(WizardStep::RepoInput);
                                        }
                                    }
                                });
                            }
                        >"Search"</button>
                    </div>
                }.into_any(),

                // ── Step 2: Loading ───────────────────────────────────────────
                WizardStep::LoadingQuants => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Searching HuggingFace..."</h2>
                    </div>
                    <div class="spinner-container">
                        <span class="spinner"></span>
                        <span class="text-muted">"Fetching available quants, please wait..."</span>
                    </div>
                }.into_any(),

                // ── Step 3: Select Quants ─────────────────────────────────────
                WizardStep::SelectQuants => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Select Quantisations"</h2>
                        <p class="form-card__desc text-muted">
                            "Choose one or more quantisation files to download from "
                            <code>{move || repo_id.get()}</code>"."
                        </p>
                    </div>

                    <div class="form-actions mb-2">
                        <button class="btn btn-secondary btn-sm" on:click=move |_| {
                            let all: HashSet<String> = available_quants
                                .get()
                                .iter()
                                .map(|q| q.filename.clone())
                                .collect();
                            selected_filenames.set(all);
                        }>"Select All"</button>
                        <button class="btn btn-secondary btn-sm" on:click=move |_| {
                            selected_filenames.set(HashSet::new());
                        }>"Deselect All"</button>
                    </div>

                    <table class="data-table">
                        <thead>
                            <tr>
                                <th class="icon-sm"></th>
                                <th>"Quant"</th>
                                <th>"Filename"</th>
                                <th>"Size"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || available_quants.get().into_iter().map(|q| {
                                let fname = q.filename.clone();
                                let fname_check = fname.clone();
                                let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                                let size_str = q.size_bytes
                                    .map(format_bytes)
                                    .unwrap_or_else(|| "?".to_string());
                                let is_checked = move || selected_filenames.get().contains(&fname_check);
                                view! {
                                    <tr>
                                        <td>
                                            <input
                                                type="checkbox"
                                                prop:checked=is_checked
                                                on:change=move |_| {
                                                    selected_filenames.update(|set| {
                                                        if set.contains(&fname) {
                                                            set.remove(&fname);
                                                        } else {
                                                            set.insert(fname.clone());
                                                        }
                                                    });
                                                }
                                            />
                                        </td>
                                        <td>
                                            <span class="badge badge-info">{label}</span>
                                        </td>
                                        <td><code>{q.filename.clone()}</code></td>
                                        <td class="text-muted">{size_str}</td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}
                        </tbody>
                    </table>

                    <div class="form-actions mt-3">
                        <button class="btn btn-secondary" on:click=move |_| {
                            wizard_step.set(WizardStep::RepoInput);
                        }>"Back"</button>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || selected_filenames.get().is_empty()
                            on:click=move |_| {
                                // Pre-fill context_lengths with 32768 for each selected filename
                                let sel = selected_filenames.get();
                                let mut ctx = HashMap::new();
                                for fname in &sel {
                                    ctx.insert(fname.clone(), 32768u32);
                                }
                                context_lengths.set(ctx);
                                wizard_step.set(WizardStep::SetContext);
                            }
                        >"Next →"</button>
                    </div>
                }.into_any(),

                // ── Step 4: Set Context ───────────────────────────────────────
                WizardStep::SetContext => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Set Context Length"</h2>
                        <p class="form-card__desc text-muted">
                            "Configure the context window size for each selected quantisation."
                        </p>
                    </div>

                    <table class="data-table">
                        <thead>
                            <tr>
                                <th>"Quant"</th>
                                <th>"Filename"</th>
                                <th>"Context Length"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let sel = selected_filenames.get();
                                available_quants.get().into_iter()
                                    .filter(|q| sel.contains(&q.filename))
                                    .map(|q| {
                                        let fname = q.filename.clone();
                                        let fname_input = fname.clone();
                                        let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                                        let current_ctx = move || context_lengths.get()
                                            .get(&fname_input)
                                            .copied()
                                            .unwrap_or(32768);
                                        view! {
                                            <tr>
                                                <td>
                                                    <span class="badge badge-info">{label}</span>
                                                </td>
                                                <td><code>{q.filename.clone()}</code></td>
                                                <td>
                                    <input
                                        class="form-input input-narrow"
                                        type="number"
                                                        min="512"
                                                        step="512"
                                                        prop:value=current_ctx
                                                        on:change=move |e| {
                                                            if let Ok(v) = event_target_value(&e).parse::<u32>() {
                                                                context_lengths.update(|m| {
                                                                    m.insert(fname.clone(), v);
                                                                });
                                                            }
                                                        }
                                                    />
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()
                            }}
                        </tbody>
                    </table>

                    <div class="form-actions mt-3">
                        <button class="btn btn-secondary" on:click=move |_| {
                            wizard_step.set(WizardStep::SelectQuants);
                        }>"Back"</button>
                        <button class="btn btn-primary" on:click=move |_| {
                            let rid = repo_id.get();
                            let sel = selected_filenames.get();
                            let quants_list = available_quants.get();
                            let ctx_map = context_lengths.get();

                            // Build request payload
                            let quants: Vec<QuantRequest> = sel.iter()
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

                            let body = PullRequest { repo_id: rid, quants };

                            wasm_bindgen_futures::spawn_local(async move {
                                let build_result = gloo_net::http::Request::post("/kronk/v1/pulls")
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

                                                // Open SSE stream for each job
                                                for entry in entries {
                                                    let job_id_str = entry.job_id.clone();
                                                    let dj = download_jobs;
                                                    let ws = wizard_step;
                                                    wasm_bindgen_futures::spawn_local(async move {
                                        let url = format!("/kronk/v1/pulls/{}/stream", job_id_str);
                                        // Helper: check if all jobs have finished and advance wizard.
                                        let advance_if_done = |dj: RwSignal<Vec<JobProgress>>, ws: RwSignal<WizardStep>| {
                                            let jobs = dj.get();
                                            if !jobs.is_empty() && jobs.iter().all(|j| {
                                                j.status == "completed" || j.status == "failed"
                                            }) {
                                                ws.set(WizardStep::Done);
                                            }
                                        };

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
                                                advance_if_done(dj, ws);
                                                return;
                                            }
                                        };
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
                                                advance_if_done(dj, ws);
                                                return;
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
                                                advance_if_done(dj, ws);
                                                return;
                                            }
                                        };

                                                        loop {
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
                                                                    advance_if_done(dj, ws);
                                                                    break;
                                                                }
                                                                // Stream ended unexpectedly — close and check if all jobs done.
                                                                _ => {
                                                                    es.close();
                                                                    advance_if_done(dj, ws);
                                                                    break;
                                                                }
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
                        }>"Start Download"</button>
                    </div>
                }.into_any(),

                // ── Step 5: Downloading ───────────────────────────────────────
                WizardStep::Downloading => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"Downloading"</h2>
                        <p class="form-card__desc text-muted">"Tracking download progress for each file."</p>
                    </div>

                    {move || error_msg.get().map(|e| view! {
                        <div class="alert alert--error mb-2">
                            <span class="alert__icon">"✕"</span>
                            <span>{e}</span>
                        </div>
                    })}

                    <div class="download-jobs">
                        {move || download_jobs.get().into_iter().map(|job| {
                            let progress_pct = job.total_bytes
                                .filter(|&total| total > 0)
                                .map(|total| (job.bytes_downloaded as f64 / total as f64 * 100.0) as u32);

                            let (status_class, status_text) = if job.status == "completed" {
                                ("badge badge-success", "Completed ✓".to_string())
                            } else if job.status == "failed" {
                                ("badge badge-error", format!("Failed: {}", job.error.clone().unwrap_or_default()))
                            } else if job.status == "running" {
                                let dl = format_bytes(job.bytes_downloaded as i64);
                                let total = job.total_bytes
                                    .map(|b| format_bytes(b as i64))
                                    .unwrap_or_else(|| "?".to_string());
                                ("badge badge-info", format!("{dl} / {total}"))
                            } else {
                                ("badge badge-muted", job.status.clone())
                            };

                            view! {
                                <div class="download-job-card card mb-2">
                                    <div class="flex-between mb-1">
                                        <span class="text-mono text-sm">{job.filename.clone()}</span>
                                        <span class=status_class>{status_text}</span>
                                    </div>
                                    <div class="progress-bar">
                                        {if let Some(pct) = progress_pct {
                                            view! {
                                                <div
                                                    class="progress-bar-fill"
                                                    style=format!("width:{}%", pct)
                                                />
                                            }.into_any()
                                        } else {
                                            view! {
                                                <div class="progress-bar-fill indeterminate" />
                                            }.into_any()
                                        }}
                                    </div>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>

                    {move || {
                        let jobs = download_jobs.get();
                        let all_finished = !jobs.is_empty() && jobs.iter().all(|j| {
                            j.status == "completed" || j.status == "failed"
                        });
                        if !all_finished {
                            return None;
                        }
                        let any_failed = jobs.iter().any(|j| j.status == "failed");
                        if any_failed {
                            let failures: Vec<_> = jobs.iter()
                                .filter(|j| j.status == "failed")
                                .map(|j| format!(
                                    "{}: {}",
                                    j.filename,
                                    j.error.as_deref().unwrap_or("unknown error")
                                ))
                                .collect();
                            Some(view! {
                                <div class="alert alert--error mt-3">
                                    <span class="alert__icon">"✕"</span>
                                    <div>
                                        <p>"Some downloads failed:"</p>
                                        <ul class="error-list">
                                            {failures.into_iter().map(|msg| view! {
                                                <li>{msg}</li>
                                            }).collect::<Vec<_>>()}
                                        </ul>
                                        <a href="/models" class="mt-2 d-inline">"Go to Models →"</a>
                                    </div>
                                </div>
                            }.into_any())
                        } else {
                            Some(view! {
                                <div class="alert alert--success mt-3">
                                    <span class="alert__icon">"✓"</span>
                                    <div>
                                        <p>"All downloads completed successfully!"</p>
                                        <a href="/models" class="mt-1 d-inline">"Go to Models →"</a>
                                    </div>
                                </div>
                            }.into_any())
                        }
                    }}
                }.into_any(),

                // ── Step 6: Done ──────────────────────────────────────────────
                WizardStep::Done => view! {
                    <div class="form-card__header">
                        <h2 class="form-card__title">"All Downloads Complete!"</h2>
                        <p class="form-card__desc text-muted">"The following models are now available."</p>
                    </div>

                    <div class="download-jobs mt-2">
                        {move || download_jobs.get().into_iter().map(|job| {
                            let badge_class = if job.status == "completed" {
                                "badge badge-success"
                            } else {
                                "badge badge-error"
                            };
                            view! {
                                <div class="flex-between card mb-1 p-cell">
                                    <span class="text-mono text-sm">{job.filename}</span>
                                    <span class=badge_class>
                                        {if job.status == "completed" { "Done ✓" } else { "Failed" }}
                                    </span>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>

                    <div class="form-actions mt-3">
                        <a href="/models">
                            <button class="btn btn-primary">"View Models →"</button>
                        </a>
                    </div>
                }.into_any(),
            }}
        </div>
    }
}
