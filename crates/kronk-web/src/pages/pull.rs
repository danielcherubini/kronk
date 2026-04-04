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
        <div class="pull-wizard">
            {move || match wizard_step.get() {
                // ── Step 1: Repo Input ────────────────────────────────────────
                WizardStep::RepoInput => view! {
                    <h1>"Pull Model"</h1>
                    <p>"Enter a HuggingFace repo ID to search for available quantisations."</p>
                    <div>
                        <label>"Repo ID: "</label>
                        <input
                            type="text"
                            prop:value=move || repo_id.get()
                            on:input=move |e| repo_id.set(event_target_value(&e))
                            placeholder="e.g. meta-llama/Llama-3.2-1B"
                        />
                    </div>

                    {move || error_msg.get().map(|e| view! {
                        <p style="color:red">"Error: " {e}</p>
                    })}

                    <button
                        on:click=move |_| {
                            let rid = repo_id.get();
                            if rid.is_empty() { return; }
                            error_msg.set(None);
                            wizard_step.set(WizardStep::LoadingQuants);
                            wasm_bindgen_futures::spawn_local(async move {
                                let url = format!("/kronk/v1/hf/{}", rid);
                                match gloo_net::http::Request::get(&url).send().await {
                                    Ok(resp) => {
                                        match resp.json::<Vec<QuantEntry>>().await {
                                            Ok(quants) => {
                                                available_quants.set(quants);
                                                wizard_step.set(WizardStep::SelectQuants);
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
                }.into_any(),

                // ── Step 2: Loading ───────────────────────────────────────────
                WizardStep::LoadingQuants => view! {
                    <h1>"Searching HuggingFace..."</h1>
                    <p class="spinner">"⏳ Fetching available quants, please wait..."</p>
                }.into_any(),

                // ── Step 3: Select Quants ─────────────────────────────────────
                WizardStep::SelectQuants => view! {
                    <h1>"Select Quants"</h1>
                    <h2>{move || repo_id.get()}</h2>

                    <div style="margin-bottom:0.5em">
                        <button on:click=move |_| {
                            let all: HashSet<String> = available_quants
                                .get()
                                .iter()
                                .map(|q| q.filename.clone())
                                .collect();
                            selected_filenames.set(all);
                        }>"Select All"</button>
                        {" "}
                        <button on:click=move |_| {
                            selected_filenames.set(HashSet::new());
                        }>"Deselect All"</button>
                    </div>

                    <table>
                        <thead>
                            <tr>
                                <th></th>
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
                                        <td>{label}</td>
                                        <td><code>{q.filename.clone()}</code></td>
                                        <td>{size_str}</td>
                                    </tr>
                                }
                            }).collect::<Vec<_>>()}
                        </tbody>
                    </table>

                    <div style="margin-top:1em">
                        <button on:click=move |_| {
                            wizard_step.set(WizardStep::RepoInput);
                        }>"Back"</button>
                        {" "}
                        <button
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
                        >"Next"</button>
                    </div>
                }.into_any(),

                // ── Step 4: Set Context ───────────────────────────────────────
                WizardStep::SetContext => view! {
                    <h1>"Set Context Length"</h1>
                    <p>"Set the context length for each selected quant."</p>

                    <table>
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
                                                <td>{label}</td>
                                                <td><code>{q.filename.clone()}</code></td>
                                                <td>
                                                    <input
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

                    <div style="margin-top:1em">
                        <button on:click=move |_| {
                            wizard_step.set(WizardStep::SelectQuants);
                        }>"Back"</button>
                        {" "}
                        <button on:click=move |_| {
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
                                                                    // Check if all jobs are done
                                                                    let all_done = dj.get().iter().all(|j| {
                                                                        j.status == "completed" || j.status == "failed"
                                                                    });
                                                                    if all_done {
                                                                        ws.set(WizardStep::Done);
                                                                    }
                                                                    break;
                                                                }
                                                                // Stream ended or error
                                                                _ => { es.close(); break; }
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
                    <h1>"Downloading..."</h1>

                    {move || error_msg.get().map(|e| view! {
                        <p style="color:red">"Error: " {e}</p>
                    })}

                    <div>
                        {move || download_jobs.get().into_iter().map(|job| {
                            let progress_val = job.bytes_downloaded as f64;
                            let progress_max = job.total_bytes.map(|b| b as f64);
                            let status_text = if job.status == "completed" {
                                "completed ✓".to_string()
                            } else if job.status == "failed" {
                                format!("failed: {}", job.error.unwrap_or_default())
                            } else if job.status == "running" {
                                let dl = format_bytes(job.bytes_downloaded as i64);
                                let total = job.total_bytes
                                    .map(|b| format_bytes(b as i64))
                                    .unwrap_or_else(|| "?".to_string());
                                format!("running: {dl} / {total}")
                            } else {
                                job.status.clone()
                            };
                            view! {
                                <div style="margin-bottom:1em">
                                    <p><strong>{job.filename}</strong></p>
                                    {if let Some(max) = progress_max {
                                        view! {
                                            <progress value=progress_val max=max />
                                        }.into_any()
                                    } else {
                                        // Indeterminate — omit value/max so the browser
                                        // renders an animated "unknown length" bar.
                                        view! { <progress /> }.into_any()
                                    }}
                                    <p>{status_text}</p>
                                </div>
                            }
                        }).collect::<Vec<_>>()}
                    </div>

                    {move || {
                        let jobs = download_jobs.get();
                        let all_done = !jobs.is_empty() && jobs.iter().all(|j| {
                            j.status == "completed" || j.status == "failed"
                        });
                        if all_done {
                            Some(view! {
                                <div>
                                    <p>"Setup complete!"</p>
                                    <a href="/models">"Go to Models →"</a>
                                </div>
                            })
                        } else {
                            None
                        }
                    }}
                }.into_any(),

                // ── Step 6: Done ──────────────────────────────────────────────
                WizardStep::Done => view! {
                    <h1>"All downloads complete!"</h1>
                    <ul>
                        {move || download_jobs.get().into_iter().map(|job| view! {
                            <li>{job.filename}</li>
                        }).collect::<Vec<_>>()}
                    </ul>
                    <a href="/models">"View Models →"</a>
                }.into_any(),
            }}
        </div>
    }
}
