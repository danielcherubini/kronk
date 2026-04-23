//! Speculative decoding benchmark form and results display.

use std::collections::BTreeMap;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use crate::components::job_log_panel::JobLogPanel;
use crate::pages::benchmarks::types::parse_model;
use crate::utils::{extract_and_store_csrf_token, post_request};

/// Human-readable descriptions for each spec type.
const SPEC_TYPE_DESC: &[(&str, &str)] = &[
    ("ngram-simple", "Simple n-gram pattern matching"),
    ("ngram-mod", "Rolling hash pool (best for code/reasoning)"),
    ("ngram-map-k", "Hash map with key tracking"),
    (
        "ngram-map-k4v",
        "Key + 4 values (experimental, long repetitions)",
    ),
];

/// All spec type identifiers in display order.
const ALL_SPEC_TYPES: &[&str] = &["ngram-simple", "ngram-mod", "ngram-map-k", "ngram-map-k4v"];

/// Default spec types (checked on load).
const DEFAULT_SPEC_TYPES: &[&str] = &["ngram-simple", "ngram-mod"];

/// Preset configurations for quick form filling.
#[derive(Clone)]
struct SpecPreset {
    label: &'static str,
    description: &'static str,
    spec_types: &'static [&'static str],
    draft_max: &'static str,
    ngram_n: &'static str,
    ngram_m: &'static str,
    gen_tokens: u32,
    runs: u32,
}

impl SpecPreset {
    fn all() -> Vec<Self> {
        vec![
            Self {
                label: "Quick filter",
                description: "Test all 4 spec types with a single draft_max to find the fastest type quickly.",
                spec_types: ALL_SPEC_TYPES,
                draft_max: "16",
                ngram_n: "12",
                ngram_m: "48",
                gen_tokens: 256,
                runs: 3,
            },
            Self {
                label: "Draft sweep",
                description: "Sweep draft_max values for ngram-simple and ngram-mod to find the sweet spot.",
                spec_types: &["ngram-simple", "ngram-mod"],
                draft_max: "8,16,32,48,64",
                ngram_n: "12",
                ngram_m: "48",
                gen_tokens: 256,
                runs: 3,
            },
            Self {
                label: "N-gram sweep",
                description: "Sweep ngram N and M dimensions for ngram-mod to find optimal pattern sizes.",
                spec_types: &["ngram-mod"],
                draft_max: "32",
                ngram_n: "8,12,16,24",
                ngram_m: "32,48,64",
                gen_tokens: 256,
                runs: 3,
            },
            Self {
                label: "Depth test",
                description: "Same as N-gram sweep. Manually set depth in advanced settings for context-length testing.",
                spec_types: &["ngram-mod"],
                draft_max: "32",
                ngram_n: "8,12,16,24",
                ngram_m: "32,48,64",
                gen_tokens: 256,
                runs: 3,
            },
        ]
    }
}

/// Parse a comma-separated string of integers into a Vec<u32>.
fn parse_sizes(s: &str) -> Vec<u32> {
    s.split(',')
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .filter_map(|v| v.parse::<u32>().ok())
        .collect()
}

/// Format "mean ± stddev" with one decimal place, or a single value when
/// stddev rounds to zero.
fn format_mean_stddev(mean: f64, stddev: f64) -> String {
    if stddev > 0.05 {
        format!("{:.1} ± {:.1}", mean, stddev)
    } else {
        format!("{:.1}", mean)
    }
}

/// CSS class for delta badge based on percentage value.
fn delta_badge_class(delta_pct: f64) -> &'static str {
    if delta_pct > 0.5 {
        "badge badge-success"
    } else if delta_pct < -0.5 {
        "badge badge-danger"
    } else {
        "badge badge-muted"
    }
}

/// Format delta percentage with Unicode minus for negative values.
fn format_delta(delta_pct: f64) -> String {
    if delta_pct >= 0.0 {
        format!("+{:.1}%", delta_pct)
    } else {
        format!("−{:.1}%", (-delta_pct))
    }
}

#[component]
pub fn SpecBench() -> impl IntoView {
    // ── Model selection ────────────────────────────────────────────────
    let selected_display_name = RwSignal::new(String::new());
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<(String, String, String)>::new());

    // ── Backend selection ──────────────────────────────────────────────
    let selected_backend = RwSignal::new(String::new());
    let available_backends = RwSignal::new(Vec::<(String, String)>::new());

    // ── Spec type checkboxes ───────────────────────────────────────────
    let spec_types: RwSignal<Vec<String>> =
        RwSignal::new(DEFAULT_SPEC_TYPES.iter().map(|s| s.to_string()).collect());

    // ── Knob fields (comma-separated) ──────────────────────────────────
    let draft_max_str = RwSignal::new("8,16,32,64".to_string());
    let ngram_n_str = RwSignal::new("12,16,24".to_string());
    let ngram_m_str = RwSignal::new("32,48".to_string());

    // ── Run settings ───────────────────────────────────────────────────
    let gen_tokens = RwSignal::new(256u32);
    let runs = RwSignal::new(3u32);

    // ── Job state ──────────────────────────────────────────────────────
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);

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
                        let model_list: Vec<(String, String, String)> =
                            models_arr.iter().filter_map(parse_model).collect();
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
                    if let Some(backends_arr) = root.get("backends").and_then(|v| v.as_array()) {
                        let backend_list: Vec<(String, String)> = backends_arr
                            .iter()
                            .filter_map(|b| {
                                let name = b.get("name")?.as_str()?.to_string();
                                let display = b
                                    .get("display_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&name)
                                    .to_string();
                                Some((name, display))
                            })
                            .collect();
                        available_backends.update(|list| *list = backend_list);
                    }
                }
            }
        });
    }

    // Auto-select the first quant when display_name changes.
    Effect::new(move |_| {
        let dn = selected_display_name.get();
        let models = available_models.get();
        if let Some((id, _, _)) = models.iter().find(|(_, name, _)| name == &dn) {
            selected_model.set(id.clone());
        } else {
            selected_model.set(String::new());
        }
    });

    // ── Preset handler ─────────────────────────────────────────────────
    let apply_preset = move |preset: SpecPreset| {
        spec_types.set(preset.spec_types.iter().map(|s| s.to_string()).collect());
        draft_max_str.set(preset.draft_max.to_string());
        ngram_n_str.set(preset.ngram_n.to_string());
        ngram_m_str.set(preset.ngram_m.to_string());
        gen_tokens.set(preset.gen_tokens);
        runs.set(preset.runs);
    };

    // ── Submit handler ─────────────────────────────────────────────────
    let submit_benchmark = move || {
        let model_id = selected_model.get();
        if model_id.is_empty() {
            return;
        }

        let backend_name = if selected_backend.get().is_empty() {
            None
        } else {
            Some(selected_backend.get())
        };
        let types = spec_types.get();
        let draft_max = parse_sizes(&draft_max_str.get());
        let ngram_n = parse_sizes(&ngram_n_str.get());
        let ngram_m = parse_sizes(&ngram_m_str.get());
        let gen_tok = gen_tokens.get();
        let runs_val = runs.get();

        benchmark_results.set(None);
        is_running.set(true);
        current_job_id.set(None);

        spawn_local(async move {
            let body = serde_json::json!({
                "model_id": model_id,
                "backend_name": backend_name,
                "spec_types": types,
                "draft_max_values": draft_max,
                "ngram_n_values": ngram_n,
                "ngram_m_values": ngram_m,
                "gen_tokens": gen_tok,
                "runs": runs_val,
            });

            let submitted = async {
                let resp = post_request("/tama/v1/benchmarks/spec-run")
                    .header("Content-Type", "application/json")
                    .body(body.to_string())
                    .ok()?;
                let resp = resp.send().await.ok()?;
                let body = resp.json::<serde_json::Value>().await.ok()?;
                body.get("job_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            }
            .await;

            match submitted {
                Some(job_id) => {
                    current_job_id.set(Some(job_id));
                }
                None => {
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
    let (spec_types_sig, _) = spec_types.split();
    let (draft_max_sig, _) = draft_max_str.split();
    let (ngram_n_sig, _) = ngram_n_str.split();
    let (ngram_m_sig, _) = ngram_m_str.split();
    let (gen_tokens_sig, _) = gen_tokens.split();
    let (runs_sig, _) = runs.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();
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
                                    .map(|(id, _, quant)| {
                                        let id_clone = id.clone();
                                        let is_selected = id == &selected_id;
                                        let label = if quant.is_empty() { "—".to_string() } else { quant.clone() };
                                        view! {
                                            <option value=id_clone selected=is_selected>{label}</option>
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
                        let backends = available_backends_sig.get();
                        backends.iter().map(|(name, display)| {
                            let name_clone = name.clone();
                            let display_clone = display.clone();
                            view! {
                                <option value=name_clone>{display_clone}</option>
                            }.into_any()
                        }).collect::<Vec<_>>()
                    }}
                </select>
                <small class="bench-hint">
                    "Select a specific backend's llama-cli, or leave empty to use the model's backend."
                </small>
            </section>

            // ── Spec types checkboxes ─────────────────────────────────
            <section class="card">
                <h3>"Spec Types to Test"</h3>
                {SPEC_TYPE_DESC.iter().map(|(id, desc)| {
                    let id_str = id.to_string();
                    let for_attr = format!("spec-{}", id);
                    let label_text = format!("{} — {}", id, *desc);
                    let id_for_checked = id_str.clone();
                    view! {
                        <div class="form-check">
                            <input
                                type="checkbox"
                                id=for_attr.clone()
                                prop:checked=move || spec_types_sig.get().contains(&id_for_checked)
                                on:change=move |e| {
                                    let checked = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().checked();
                                    let id_inner = id_str.clone();
                                    spec_types.update(|types| {
                                        if checked {
                                            if !types.contains(&id_inner) {
                                                types.push(id_inner.clone());
                                            }
                                        } else {
                                            types.retain(|t| t != &id_inner);
                                        }
                                    });
                                }
                            />
                            <label class="form-check-label" for=for_attr>
                                {label_text}
                            </label>
                        </div>
                    }.into_any()
                }).collect::<Vec<_>>()}
            </section>

            // ── Knob fields ───────────────────────────────────────────
            <section class="card">
                <h3>"Knob Configuration"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Draft max values"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || draft_max_sig.get()
                            on:input=move |e| { draft_max_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                        />
                        <small class="text-muted">"Tokens to draft per round, e.g. 8,16,32,64"</small>
                    </div>
                    <div class="form-group">
                        <label>"N-gram size N"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || ngram_n_sig.get()
                            on:input=move |e| { ngram_n_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                        />
                        <small class="text-muted">"Lookup pattern length (for ngram-mod/map), e.g. 12,16,24"</small>
                    </div>
                    <div class="form-group">
                        <label>"N-gram size M"</label>
                        <input
                            type="text"
                            class="form-control"
                            prop:value=move || ngram_m_sig.get()
                            on:input=move |e| { ngram_m_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                        />
                        <small class="text-muted">"Draft pattern length (for ngram-map-k/k4v only), e.g. 32,48"</small>
                    </div>
                </div>
            </section>

            // ── Run settings ──────────────────────────────────────────
            <section class="card">
                <h3>"Run Settings"</h3>
                <div class="grid-2">
                    <div class="form-group">
                        <label>"Generation tokens"</label>
                        <input
                            type="number"
                            class="form-control"
                            prop:value=move || gen_tokens_sig.get()
                            min="1" max="4096"
                            on:input=move |e| {
                                let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                                if let Ok(n) = val.parse::<u32>() { gen_tokens.set(n); }
                            }
                        />
                    </div>
                    <div class="form-group">
                        <label>"Runs per config"</label>
                        <input
                            type="number"
                            class="form-control"
                            prop:value=move || runs_sig.get()
                            min="1" max="20"
                            on:input=move |e| {
                                let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                                if let Ok(n) = val.parse::<u32>() { runs.set(n); }
                            }
                        />
                    </div>
                </div>
            </section>

            // ── Preset buttons ────────────────────────────────────────
            <section class="card">
                <h3>"Presets"</h3>
                <small class="bench-hint">
                    "Click a preset to auto-fill the form with recommended values."
                </small>
                <div class="preset-buttons">
                    {SpecPreset::all().into_iter().map(|preset| {
                        let desc = preset.description;
                        view! {
                            <button
                                class="btn btn-outline-secondary btn-sm"
                                title=desc
                                on:click=move |_| apply_preset(preset.clone())
                            >
                                {preset.label}
                            </button>
                        }.into_any()
                    }).collect::<Vec<_>>()}
                </div>
            </section>

            // ── Run button ────────────────────────────────────────────
            <div class="text-center my-3">
                <button
                    class="btn btn-primary btn-lg"
                    prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                    on:click=move |_| { submit_benchmark(); }
                >
                    {move || if is_running_sig.get() { "Running..." } else { "▶ Run Spec Benchmark" }}
                </button>
            </div>

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

            // ── Results table ─────────────────────────────────────────
            {move || {
                let Some(result) = benchmark_results_sig.get() else {
                    return view! { <div></div> }.into_any();
                };

                let baseline_tg_ts = result.get("baseline_tg_ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let baseline_tg_stddev = result.get("baseline_tg_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let entries: Vec<&serde_json::Value> = result
                    .get("entries")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().collect())
                    .unwrap_or_default();

                if entries.is_empty() && baseline_tg_ts == 0.0 {
                    return view! { <div></div> }.into_any();
                }

                // Sort entries by delta_pct descending (best first).
                let mut sortable: Vec<_> = entries.into_iter().collect();
                sortable.sort_by(|a, b| {
                    let da = a.get("delta_pct").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
                    let db = b.get("delta_pct").and_then(|v| v.as_f64()).unwrap_or(f64::NEG_INFINITY);
                    db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
                });

                view! {
                    <section class="card mt-3">
                        <h3>"Spec Decoding Results"</h3>

                        // Baseline summary card
                        <div class="bench-summary">
                            <div class="bench-summary__item">
                                <div class="bench-summary__label">"Baseline TG t/s"</div>
                                <div class="bench-summary__value">{format_mean_stddev(baseline_tg_ts, baseline_tg_stddev)}</div>
                            </div>
                        </div>

                        // Results table
                        <table class="table table-striped">
                            <thead>
                                <tr>
                                    <th>"Spec Type"</th>
                                    <th>"Draft Max"</th>
                                    <th>"N"</th>
                                    <th>"M"</th>
                                    <th class="text-right">"t/s (± stddev)"</th>
                                    <th class="text-right">"Δ vs baseline"</th>
                                </tr>
                            </thead>
                            <tbody>
                                // Baseline row
                                <tr class="table-active">
                                    <td>"— (baseline)"</td>
                                    <td class="text-mono">"—"</td>
                                    <td class="text-mono">"—"</td>
                                    <td class="text-mono">"—"</td>
                                    <td class="text-mono text-right">{format_mean_stddev(baseline_tg_ts, baseline_tg_stddev)}</td>
                                    <td class="text-mono text-right">"—"</td>
                                </tr>
                                // Sorted spec entries
                                {sortable.into_iter().map(|entry| {
                                    let spec_type = entry.get("spec_type").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let draft_max = entry.get("draft_max").and_then(|v| v.as_u64()).unwrap_or(0);
                                    let ngram_n = entry.get("ngram_n").and_then(|v| v.as_u64());
                                    let ngram_m = entry.get("ngram_m").and_then(|v| v.as_u64());
                                    let tg_mean = entry.get("tg_ts_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let tg_stddev = entry.get("tg_ts_stddev").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let delta_pct = entry.get("delta_pct").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                    let status = entry.get("status").and_then(|v| v.as_str()).unwrap_or("");

                                    let n_display = ngram_n.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string());
                                    let m_display = ngram_m.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string());
                                    let ts_display = format_mean_stddev(tg_mean, tg_stddev);
                                    let delta_display = format_delta(delta_pct);
                                    let badge_class = delta_badge_class(delta_pct);

                                    let row_class = if status == "failed" {
                                        "table-danger"
                                    } else if status == "skipped_oom" {
                                        "table-warning"
                                    } else {
                                        ""
                                    };

                                    view! {
                                        <tr class=row_class>
                                            <td>{spec_type}</td>
                                            <td class="text-mono">{draft_max}</td>
                                            <td class="text-mono">{n_display}</td>
                                            <td class="text-mono">{m_display}</td>
                                            <td class="text-mono text-right">{ts_display}</td>
                                            <td class="text-mono text-right">
                                                <span class={badge_class}>{delta_display}</span>
                                            </td>
                                        </tr>
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    </section>
                }.into_any()
            }}
        </div>
    }
}
