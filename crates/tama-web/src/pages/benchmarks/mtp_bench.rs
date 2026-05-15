//! MTP (Multi-Token Prediction) benchmark form and results display.

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::components::job_log_panel::JobLogPanel;
use crate::pages::benchmarks::types::parse_model;
use crate::utils::{extract_and_store_csrf_token, post_request};

/// Parse a comma-separated string of integers into a Vec<u32>.
fn parse_sizes(s: &str) -> Vec<u32> {
    s.split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .filter_map(|v| v.parse::<u32>().ok())
        .collect()
}

#[component]
pub fn MtpBench() -> impl IntoView {
    // ── Model selection ────────────────────────────────────────────────
    let selected_display_name = RwSignal::new(String::new());
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<(String, String, Vec<String>)>::new());

    // ── Backend selection ──────────────────────────────────────────────
    let selected_backend = RwSignal::new(String::new());
    let available_backends = RwSignal::new(Vec::<(String, String)>::new());

    // ── MTP configuration ──────────────────────────────────────────────
    let draft_max_str = RwSignal::new("0,1,2,3,4,5,6,7,8".to_string());
    let ngl_str = RwSignal::new("99".to_string());
    let flash_attn = RwSignal::new(true);

    // ── Job state ──────────────────────────────────────────────────────
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);
    let error_msg = RwSignal::new(String::new());

    // ── Refresh trigger for model fetch ────────────────────────────────
    let model_refresh = RwSignal::new(0u32);

    // Fetch available models on mount.
    Effect::new(move |_| {
        let _ = model_refresh.get();
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/tama/v1/models").send().await {
                extract_and_store_csrf_token(&resp);
                if let Ok(root) = resp.json::<serde_json::Value>().await {
                    if let Some(models_arr) = root.get("models").and_then(|v| v.as_array()) {
                        let mut seen: std::collections::HashSet<(String, String)> =
                            std::collections::HashSet::new();
                        let model_list: Vec<(String, String, Vec<String>)> = models_arr
                            .iter()
                            .filter_map(parse_model)
                            .flatten()
                            .filter(|(_, name, quant)| seen.insert((name.clone(), quant.clone())))
                            .map(|(id, name, quant)| (id, name, vec![quant]))
                            .collect();
                        available_models.update(|list| *list = model_list);
                    }
                }
            }
        });
    });

    // Fetch available backends.
    {
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/tama/v1/backends")
                .send()
                .await
            {
                extract_and_store_csrf_token(&resp);
                if let Ok(root) = resp.json::<serde_json::Value>().await {
                    let mut backend_list: Vec<(String, String)> = Vec::new();
                    for arr_key in ["backends", "custom"] {
                        if let Some(arr) = root.get(arr_key).and_then(|v| v.as_array()) {
                            for b in arr {
                                let installed = b
                                    .get("installed")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                if !installed {
                                    continue;
                                }
                                let name = b
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let display = b
                                    .get("display_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&name)
                                    .to_string();
                                let variant = b
                                    .get("gpu_variant")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                if !variant.is_empty() {
                                    let value = format!("{}:{}", name, variant);
                                    let label = if variant == "cpu" {
                                        display
                                    } else {
                                        format!("{} ({})", display, variant)
                                    };
                                    backend_list.push((value, label));
                                }
                            }
                        }
                    }
                    available_backends.update(|list| *list = backend_list);
                }
            }
        });
    }

    // Auto-select the first quant when display_name changes.
    Effect::new(move |_| {
        let dn = selected_display_name.get();
        let models = available_models.get();
        if let Some((id, _, quants)) = models.iter().find(|(_, name, _)| name == &dn) {
            if let Some(first_quant) = quants.first() {
                selected_model.set(format!("{}:{}", id, first_quant));
            } else {
                selected_model.set(id.clone());
            }
        } else {
            selected_model.set(String::new());
        }
    });

    // ── Submit handler ─────────────────────────────────────────────────
    let submit_benchmark = move || {
        let raw_model = selected_model.get();
        if raw_model.is_empty() {
            return;
        }
        let (model_id, quant) = if let Some(colon) = raw_model.find(':') {
            (
                raw_model[..colon].to_string(),
                Some(raw_model[colon + 1..].to_string()),
            )
        } else {
            (raw_model, None)
        };

        let raw_backend = selected_backend.get();
        let (backend_name, gpu_variant) = if raw_backend.is_empty() {
            (None, None)
        } else if let Some((name, variant)) = raw_backend.split_once(':') {
            (Some(name.to_string()), Some(variant.to_string()))
        } else {
            (Some(raw_backend), None)
        };

        let draft_max_values = parse_sizes(&draft_max_str.get());
        let ngl_val: Option<u32> = if ngl_str.get().is_empty() {
            None
        } else {
            ngl_str.get().parse::<u32>().ok()
        };
        let flash = flash_attn.get();

        benchmark_results.set(None);
        is_running.set(true);
        current_job_id.set(None);

        spawn_local(async move {
            let body = serde_json::json!({
                "model_id": model_id,
                "quant": quant,
                "backend_name": backend_name,
                "gpu_variant": gpu_variant,
                "draft_max_values": draft_max_values,
                "ngl": ngl_val,
                "draft_ngl": Some(99u32),
                "flash_attn": flash,
            });

            let submitted = async {
                let builder = post_request("/tama/v1/benchmarks/mtp-run")
                    .header("Content-Type", "application/json")
                    .body(body.to_string())
                    .ok()?;
                let resp = builder.send().await.ok()?;
                if resp.status() >= 400 {
                    let err_text =
                        resp.text().await.ok().unwrap_or_else(|| {
                            format!("Request failed with status {}", resp.status())
                        });
                    return Some(Err(err_text));
                }
                let body = resp.json::<serde_json::Value>().await.ok()?;
                body.get("job_id")
                    .and_then(|v| v.as_str())
                    .map(|s| Ok(s.to_string()))
            }
            .await;

            match submitted {
                Some(Ok(job_id)) => {
                    current_job_id.set(Some(job_id));
                }
                Some(Err(err)) => {
                    error_msg.set(err);
                    is_running.set(false);
                }
                None => {
                    error_msg
                        .set("Failed to submit benchmark — check network connection.".to_string());
                    is_running.set(false);
                }
            }
        });
    };

    // ── SSE callbacks ──────────────────────────────────────────────────
    let on_result_cb = Callback::new(move |results_json: String| {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&results_json) {
            benchmark_results.set(Some(parsed));
        }
        // Receiving a result event means the job is done.
        is_running.set(false);
    });
    let on_status_cb = Callback::new(move |status: String| {
        if status != "running" {
            is_running.set(false);
        }
    });

    // ── Read-only splits for views ─────────────────────────────────────
    let (available_models_sig, _) = available_models.split();
    let (selected_display_sig, _) = selected_display_name.split();
    let (selected_model_sig, _) = selected_model.split();
    let (available_backends_sig, _) = available_backends.split();
    let (draft_max_sig, _) = draft_max_str.split();
    let (ngl_sig, _) = ngl_str.split();
    let (flash_sig, _) = flash_attn.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();
    let (error_sig, _) = error_msg.split();
    let (benchmark_results_sig, _) = benchmark_results.split();

    view! {
        <div>
            // ── Model selection ───────────────────────────────────────
            <section class="card">
                <h3>"Model"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Model"</label>
                        <select
                            class="form-select"
                            on:change=move |e| {
                                let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                                selected_display_name.set(val);
                            }
                        >
                            <option value="" disabled selected=move || selected_display_sig.get().is_empty()>"Select a model..."</option>
                            {move || {
                                let models = available_models_sig.get();
                                let mut grouped: BTreeMap<String, ()> = BTreeMap::new();
                                for (_, name, _) in models.iter() {
                                    grouped.insert(name.clone(), ());
                                }
                                grouped.keys().map(|name| {
                                    let value = name.clone();
                                    let label = name.clone();
                                    view! {
                                        <option value=value>{label}</option>
                                    }.into_any()
                                }).collect::<Vec<_>>()
                            }}
                        </select>
                    </div>
                    <div class="form-group">
                        <label>"Quant"</label>
                        <select
                            class="form-select"
                            prop:disabled=move || selected_display_sig.get().is_empty()
                            on:change=move |e| {
                                let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                                selected_model.set(val);
                            }
                        >
                            <option value="" disabled>"Select quant..."</option>
                            {move || {
                                let models = available_models_sig.get();
                                let dn = selected_display_sig.get();
                                let selected_id = selected_model_sig.get();
                                models.iter()
                                    .filter(|(_, name, _)| name == &dn)
                                    .flat_map(|(id, _, quants)| {
                                        quants.iter().map(move |quant| (id.clone(), quant.clone()))
                                    })
                                    .map(|(id_clone, quant)| {
                                        let value = format!("{}:{}", id_clone, quant);
                                        let is_selected = value == selected_id;
                                        view! {
                                            <option value=value selected=is_selected>{quant}</option>
                                        }.into_any()
                                    }).collect::<Vec<_>>()
                            }}
                        </select>
                    </div>
                </div>
            </section>

            // ── Backend selection ─────────────────────────────────────
            <section class="card">
                <h3>"Backend"</h3>
                <select
                    class="form-select"
                    on:change=move |e| {
                        let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                        selected_backend.set(val);
                    }
                >
                    <option value="">"Auto (model's backend)"</option>
                    {move || {
                        available_backends_sig.get().iter().map(|(value, label)| {
                            let value2 = value.clone();
                            view! {
                                <option value=value2>
                                    {label.clone()}
                                </option>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </select>
                <small class="bench-hint">
                    "Select a specific backend variant, or leave empty to use the model's backend."
                </small>
            </section>

            // ── MTP Configuration ─────────────────────────────────────
            <section class="card">
                <h3>"MTP Configuration"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Draft-n-max values"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || draft_max_sig.get()
                            on:input=move |e| { draft_max_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                        />
                        <small class="text-muted">"Comma-separated, e.g. 0,1,2,3,4,5,6,7,8"</small>
                    </div>
                    <div class="form-group">
                        <label>"GPU layers"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || ngl_sig.get()
                            on:input=move |e| { ngl_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                        />
                        <small class="text-muted">"GPU layers for the draft model (default 99)"</small>
                    </div>
                    <div class="form-group">
                        <div class="form-check">
                            <input
                                id="mtp-flash-attn"
                                type="checkbox"
                                prop:checked=move || flash_sig.get()
                                on:change=move |e| {
                                    let checked = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().checked();
                                    flash_attn.set(checked);
                                }
                            />
                            <label class="form-check-label" for="mtp-flash-attn">"Flash attention"</label>
                        </div>
                    </div>
                </div>
            </section>

            // ── Run button ────────────────────────────────────────────
            <div class="text-center my-3">
                <button
                    class="btn btn-primary btn-lg"
                    prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                    on:click=move |_| { submit_benchmark(); }
                >
                    {move || if is_running_sig.get() { "Running..." } else { "▶ Run MTP Benchmark" }}
                </button>
            </div>

            // ── Error display ─────────────────────────────────────────
            {move || {
                let err = error_sig.get();
                if !err.is_empty() {
                    view! {
                        <div class="alert alert-danger mt-2">
                            <p class="mb-0">{err}</p>
                        </div>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // ── Progress / logs ───────────────────────────────────────
            {move || {
                if let Some(job_id) = current_job_id_sig.get() {
                    view! {
                        <JobLogPanel
                            job_id=job_id
                            on_result=on_result_cb
                            on_status=on_status_cb
                        />
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            }}

            // ── Results display ───────────────────────────────────────
            {move || {
                let Some(result) = benchmark_results_sig.get() else {
                    return view! { <div></div> }.into_any();
                };

                let entries: Vec<serde_json::Value> = result
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .map(|a| a.to_vec())
                    .unwrap_or_default();

                let aggregate = result.get("aggregate");
                let agg_accept_rate = aggregate
                    .and_then(|a| a.get("aggregate_accept_rate"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let agg_total_predicted = aggregate
                    .and_then(|a| a.get("total_predicted"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agg_total_draft = aggregate
                    .and_then(|a| a.get("total_draft"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agg_total_draft_accepted = aggregate
                    .and_then(|a| a.get("total_draft_accepted"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let agg_wall_total = aggregate
                    .and_then(|a| a.get("wall_s_total"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                if entries.is_empty() {
                    return view! { <div></div> }.into_any();
                }

                // Group entries by draft_max value
                let mut groups: std::collections::BTreeMap<u64, Vec<serde_json::Value>> =
                    std::collections::BTreeMap::new();
                for entry in &entries {
                    let draft_max = entry
                        .get("draft_max")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    groups.entry(draft_max).or_default().push(entry.clone());
                }

                view! {
                    <section class="card mt-3">
                        <h3>"MTP Benchmark Results"</h3>

                        // Aggregate summary
                        <div class="bench-summary">
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Total Predicted"</div>
                                <div class="bench-summary__value">{agg_total_predicted.to_string()}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Total Draft"</div>
                                <div class="bench-summary__value">{agg_total_draft.to_string()}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Total Accepted"</div>
                                <div class="bench-summary__value">{agg_total_draft_accepted.to_string()}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Accept Rate"</div>
                                <div class="bench-summary__value">{format!("{:.1}%", agg_accept_rate * 100.0)}</div>
                            </div>
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Wall Time"</div>
                                <div class="bench-summary__value">{format!("{:.1} s", agg_wall_total)}</div>
                            </div>
                        </div>

                        // Per-draft_max group tables
                        {groups.into_iter().map(|(draft_max, group_entries)| {
                            let is_baseline = draft_max == 0;
                            let group_label = if is_baseline {
                                "Baseline (draft-n-max: 0)".to_string()
                            } else {
                                format!("Draft-n-max: {}", draft_max)
                            };

                            // Compute group aggregates
                            let group_wall_total: f64 = group_entries.iter()
                                .filter_map(|e| e.get("wall_s").and_then(|v| v.as_f64()))
                                .sum();
                            let group_pred_total: u64 = group_entries.iter()
                                .filter_map(|e| e.get("predicted_n").and_then(|v| v.as_u64()))
                                .sum();
                            let group_draft_total: u64 = group_entries.iter()
                                .filter_map(|e| e.get("draft_n").and_then(|v| v.as_u64()))
                                .sum();
                            let group_draft_accepted: u64 = group_entries.iter()
                                .filter_map(|e| e.get("draft_n_accepted").and_then(|v| v.as_u64()))
                                .sum();
                            let group_accept_rate = if group_draft_total > 0 {
                                group_draft_accepted as f64 / group_draft_total as f64
                            } else {
                                0.0
                            };
                            let group_avg_tok_s: f64 = group_entries.iter()
                                .filter_map(|e| e.get("predicted_per_second").and_then(|v| v.as_f64()))
                                .sum::<f64>() / group_entries.len() as f64;

                            view! {
                                <div class="mt-3">
                                    <h4>{group_label.clone()}</h4>
                                    <table class="table table-striped">
                                        <thead>
                                            <tr>
                                                <th>"Prompt"</th>
                                                <th class="text-right">"Wall (s)"</th>
                                                <th class="text-right">"Pred"</th>
                                                <th class="text-right">"Draft"</th>
                                                <th class="text-right">"Acc"</th>
                                                <th class="text-right">"Rate"</th>
                                                <th class="text-right">"tok/s"</th>
                                            </tr>
                                        </thead>
                                        <tbody>
                                            {group_entries.into_iter().map(|entry| {
                                                let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                                let wall_s = entry.get("wall_s").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                                let predicted_n = entry.get("predicted_n").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let draft_n = entry.get("draft_n").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let draft_n_accepted = entry.get("draft_n_accepted").and_then(|v| v.as_u64()).unwrap_or(0);
                                                let accept_rate = entry.get("accept_rate").and_then(|v| v.as_f64());
                                                let tok_per_s = entry.get("predicted_per_second").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                                let error: Option<String> = entry
                                                    .get("error")
                                                    .and_then(|v| v.as_str())
                                                    .map(|s| s.to_string());

                                                let rate_display = accept_rate
                                                    .map(|r| format!("{:.0}%", r * 100.0))
                                                    .unwrap_or_else(|| "—".to_string());

                                                let row_class = if error.is_some() {
                                                    "table-danger"
                                                } else {
                                                    ""
                                                };

                                                view! {
                                                    <tr class=row_class>
                                                        <td>
                                                            {name.clone()}
                                                            {if let Some(err) = error {
                                                                view! { <br /><small class="text-danger">{err}</small> }.into_any()
                                                            } else {
                                                                view! { <div></div> }.into_any()
                                                            }}
                                                        </td>
                                                        <td class="text-mono text-right">{format!("{:.2}", wall_s)}</td>
                                                        <td class="text-mono text-right">{predicted_n}</td>
                                                        <td class="text-mono text-right">{draft_n}</td>
                                                        <td class="text-mono text-right">{draft_n_accepted}</td>
                                                        <td class="text-mono text-right">{rate_display}</td>
                                                        <td class="text-mono text-right">{format!("{:.1}", tok_per_s)}</td>
                                                    </tr>
                                                }.into_any()
                                            }).collect::<Vec<_>>()}
                                            // Group aggregate row
                                            <tr class="table-active">
                                                <td><strong>"Group Total"</strong></td>
                                                <td class="text-mono text-right"><strong>{format!("{:.2}", group_wall_total)}</strong></td>
                                                <td class="text-mono text-right"><strong>{group_pred_total}</strong></td>
                                                <td class="text-mono text-right"><strong>{group_draft_total}</strong></td>
                                                <td class="text-mono text-right"><strong>{group_draft_accepted}</strong></td>
                                                <td class="text-mono text-right"><strong>{format!("{:.0}%", group_accept_rate * 100.0)}</strong></td>
                                                <td class="text-mono text-right"><strong>{format!("{:.1}", group_avg_tok_s)}</strong></td>
                                            </tr>
                                        </tbody>
                                    </table>
                                </div>
                            }.into_any()
                        }).collect::<Vec<_>>()}
                    </section>
                }.into_any()
            }}
        </div>
    }
}
