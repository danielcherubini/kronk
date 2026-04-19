//! Benchmarks page — run llama-bench benchmarks from the web UI.

mod types;
mod utils;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;

use self::types::{BenchmarkPreset, HistoryEntry};
use self::utils::format_timestamp;
use crate::components::job_log_panel::JobLogPanel;

#[component]
pub fn Benchmarks() -> impl IntoView {
    // Model selection
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<(String, String, String)>::new());

    // Backend selection — which backend's llama-bench to use
    let selected_backend = RwSignal::new(String::new());
    let available_backends = RwSignal::new(Vec::<(String, String)>::new()); // (name, display_name)

    // Test configuration
    let pp_sizes_str = RwSignal::new("512".to_string());
    let tg_sizes_str = RwSignal::new("128".to_string());
    let runs = RwSignal::new(3u32);
    let warmup = RwSignal::new(1u32);
    let threads_str = RwSignal::new("auto".to_string());
    let ngl_range = RwSignal::new("".to_string());
    let ctx_override = RwSignal::new("".to_string());

    // Job state — is_running tracks whether a benchmark is currently running
    let is_running = RwSignal::new(false);
    let current_job_id = RwSignal::new(Option::<String>::None);
    let benchmark_results = RwSignal::new(Option::<serde_json::Value>::None);
    let results_refresh = RwSignal::new(0u32);

    // History state
    let history = RwSignal::new(Vec::<HistoryEntry>::new());
    let show_history = RwSignal::new(false);

    // Refresh trigger — increment to force a refetch
    let model_refresh = RwSignal::new(0u32);

    // Fetch available models on mount. Uses Effect + spawn_local so it runs
    // reliably after client-side hydration (spawn_local alone is a no-op in SSR).
    Effect::new(move |_| {
        let _ = model_refresh.get();
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/koji/v1/models").send().await {
                if let Ok(root) = resp.json::<serde_json::Value>().await {
                    if let Some(models_arr) = root.get("models").and_then(|v| v.as_array()) {
                        let model_list: Vec<(String, String, String)> =
                            models_arr.iter().filter_map(types::parse_model).collect();
                        available_models.update(|list| *list = model_list);
                    }
                }
            }
        });
    });

    // Fetch available backends for llama-bench selection.
    {
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/api/backends").send().await {
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

    // Poll for benchmark results every 2 seconds while a job is running.
    // Once results appear, stop polling.
    let _results_poll = Effect::new(move |_| {
        let _ = results_refresh.get();
        let job_id = current_job_id.get();
        if job_id.is_none() || !is_running.get() {
            return;
        }

        let jid = job_id.unwrap();
        spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(2000).await;
                if !is_running.get() {
                    break; // Job finished, stop polling
                }
                if let Ok(resp) =
                    gloo_net::http::Request::get(&format!("/api/benchmarks/jobs/{jid}"))
                        .send()
                        .await
                {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(results) = body.get("benchmark_results") {
                            benchmark_results.set(Some(results.clone()));
                            break; // Results found, stop polling
                        }
                    }
                }
            }
        });
    });

    // Fetch benchmark history on mount
    {
        let history_signal = history;
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/api/benchmarks/history")
                .send()
                .await
            {
                if let Ok(entries) = resp.json::<Vec<HistoryEntry>>().await {
                    history_signal.set(entries);
                }
            }
        });
    }

    // Parse helpers (captures nothing, pure functions)
    let parse_sizes = move |s: &str| -> Vec<u32> {
        s.split(',')
            .map(|v| v.trim().parse::<u32>().unwrap_or(0))
            .filter(|v| *v > 0)
            .collect()
    };

    let parse_threads = move |s: &str| -> Option<Vec<u32>> {
        if s.trim().to_lowercase() == "auto" || s.trim().is_empty() {
            None
        } else {
            Some(
                s.split(',')
                    .map(|v| v.trim().parse::<u32>().unwrap_or(0))
                    .filter(|v| *v > 0)
                    .collect(),
            )
        }
    };

    // Apply preset handler
    let apply_preset_handler = move |preset: BenchmarkPreset| {
        pp_sizes_str.set(
            preset
                .pp_sizes
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        tg_sizes_str.set(
            preset
                .tg_sizes
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        runs.set(preset.runs);
        threads_str.set(
            preset
                .threads
                .map(|t| {
                    t.iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or("auto".to_string()),
        );
        ngl_range.set(preset.ngl_range.unwrap_or("").to_string());
    };

    // Submit benchmark and connect SSE
    let submit_benchmark = move || {
        let model_id = selected_model.get();
        let pp = parse_sizes(&pp_sizes_str.get());
        let tg = parse_sizes(&tg_sizes_str.get());
        let runs_val = runs.get();
        let warmup_val = warmup.get();
        let threads = parse_threads(&threads_str.get());
        let ngl = if ngl_range.get().is_empty() {
            None
        } else {
            Some(ngl_range.get())
        };
        let ctx = if ctx_override.get().is_empty() {
            None
        } else {
            ctx_override.get().parse::<u32>().ok()
        };

        spawn_local(async move {
            let backend_name = if selected_backend.get().is_empty() {
                None
            } else {
                Some(selected_backend.get())
            };
            let body = serde_json::json!({
                "model_id": model_id,
                "backend_name": backend_name,
                "pp_sizes": pp,
                "tg_sizes": tg,
                "runs": runs_val,
                "warmup": warmup_val,
                "threads": threads,
                "ngl_range": ngl,
                "ctx_override": ctx,
            });

            if let Ok(builder) = gloo_net::http::Request::post("/api/benchmarks/run")
                .header("Content-Type", "application/json")
                .body(body.to_string())
            {
                if let Ok(resp) = builder.send().await {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        if let Some(job_id) = body.get("job_id").and_then(|v| v.as_str()) {
                            // JobLogPanel component handles SSE automatically
                            // for this job_id.
                            current_job_id.set(Some(job_id.to_string()));
                            results_refresh.update(|n| *n += 1);
                        }
                    }
                }
            }
        });
    };

    // Split signals for read-only view access (done before closures above capture them)
    let (available_models_sig, _) = available_models.split();
    let (selected_model_sig, _selected_model_rw) = selected_model.split();
    let (available_backends_sig, _) = available_backends.split();
    let (pp_sizes_sig, _) = pp_sizes_str.split();
    let (tg_sizes_sig, _) = tg_sizes_str.split();
    let (runs_sig, _) = runs.split();
    let (warmup_sig, _) = warmup.split();
    let (threads_sig, _) = threads_str.split();
    let (ngl_sig, _) = ngl_range.split();
    let (show_sig, _) = show_history.split();
    let (history_sig, _) = history.split();
    let (is_running_sig, _) = is_running.split();
    let (current_job_id_sig, _) = current_job_id.split();

    view! {
        <div class="page-header">
            <h1>"Benchmarks"</h1>
            <div class="flex-between gap-1">
                <button class="btn btn-secondary btn-sm" on:click=move |_| show_history.update(|v| *v = !*v)>
                    {move || if show_history.get() { "Hide History" } else { "Show History" }}
                </button>
            </div>
        </div>

        // Model selection
        <section class="card">
            <h3>"Model"</h3>
            <select
                class="form-select"
                on:change=move |e| {
                    let val = e.target().unwrap().dyn_into::<web_sys::HtmlSelectElement>().unwrap().value();
                    selected_model.set(val);
                }
            >
                <option value="" disabled>"Select a model..."</option>
                {move || {
                    let models = available_models_sig.get();
                    models.iter().map(|(id, name, quant)| {
                        let label = if !quant.is_empty() {
                            format!("{} ({})", name, quant)
                        } else {
                            name.clone()
                        };
                        let id_clone = id.clone();
                        view! {
                            <option value=id_clone>{label}</option>
                        }.into_any()
                    }).collect::<Vec<_>>()
                }}
            </select>
        </section>

        // Backend selection (which llama-bench to use)
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
            <small class="text-muted mt-1 d-block" style="font-size:0.8rem;">
                "Select a specific backend's llama-bench, or leave empty to use the model's backend."
            </small>
        </section>

        // Test configuration
        <section class="card">
            <h3>"Test Configuration"</h3>
            <div class="grid-2">
                <div class="form-group">
                    <label>"Prompt sizes (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || pp_sizes_sig.get()
                        on:input=move |e| { pp_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 128,256,512"</small>
                </div>
                <div class="form-group">
                    <label>"Generation lengths (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || tg_sizes_sig.get()
                        on:input=move |e| { tg_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 32,64,128"</small>
                </div>
                <div class="form-group">
                    <label>"Runs"</label>
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
                <div class="form-group">
                    <label>"Warmup runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || warmup_sig.get()
                        min="0" max="10"
                        on:input=move |e| {
                            let val = e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value();
                            if let Ok(n) = val.parse::<u32>() { warmup.set(n); }
                        }
                    />
                </div>
                <div class="form-group">
                    <label>"Threads"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || threads_sig.get()
                        on:input=move |e| { threads_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"auto, or comma-separated e.g. 4,8,16"</small>
                </div>
                <div class="form-group">
                    <label>"GPU layers range (sweet spot)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ngl_sig.get()
                        on:input=move |e| { ngl_range.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"e.g. 0-99+1 to sweep, or empty for all"</small>
                </div>
            </div>
        </section>

        // Presets
        <section class="card">
            <h3>"Presets"</h3>
            <div class="preset-buttons">
                {BenchmarkPreset::all().into_iter().map(|preset| {
                    view! {
                        <button
                            class="btn btn-outline-secondary btn-sm"
                            on:click=move |_| apply_preset_handler(preset.clone())
                        >
                            {preset.label}
                        </button>
                    }.into_any()
                }).collect::<Vec<_>>()}
            </div>
        </section>

        // Run button
        <div class="text-center my-3">
            <button
                class="btn btn-primary btn-lg"
                prop:disabled=move || selected_model_sig.get().is_empty() || is_running_sig.get()
                on:click=move |_| { submit_benchmark(); }
            >
                {move || if is_running_sig.get() { "Running..." } else { "▶ Run Benchmark" }}
            </button>
        </div>

        // Progress / logs — handled by JobLogPanel component
        {move || {
            if let Some(job_id) = current_job_id_sig.get() {
                view! {
                    <JobLogPanel job_id=job_id />
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // Benchmark results
        {move || {
            if let Some(results_val) = benchmark_results.get().clone() {
                // benchmark_results is a JSON string (array of summaries)
                let summaries: Vec<_> = if let Some(s) = results_val.as_str() {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                        if let Some(arr) = parsed.as_array() {
                            arr.iter().map(|s| {
                                let test_name = s.get("test_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                let prompt_tokens = s.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                let gen_tokens = s.get("gen_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                                let pp_mean = s.get("pp_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let tg_mean = s.get("tg_mean").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                (test_name, prompt_tokens, gen_tokens, pp_mean, tg_mean)
                            }).collect()
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };
                if !summaries.is_empty() {
                    view! {
                        <section class="card mt-3">
                            <h3>"Benchmark Results"</h3>
                            <table class="table table-striped">
                                <thead>
                                    <tr>
                                        <th>"Test"</th>
                                        <th>"Prompt Tokens"</th>
                                        <th>"Gen Tokens"</th>
                                        <th>"PP (tok/s)"</th>
                                        <th>"TG (tok/s)"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {summaries.into_iter().map(|(test_name, prompt_tokens, gen_tokens, pp_mean, tg_mean)| {
                                        view! {
                                            <tr>
                                                <td>{test_name}</td>
                                                <td class="text-mono">{prompt_tokens}</td>
                                                <td class="text-mono">{gen_tokens}</td>
                                                <td class="text-mono">{if pp_mean > 0.01 { format!("{:.1}", pp_mean) } else { "—".to_string() }}</td>
                                                <td class="text-mono">{if tg_mean > 0.01 { format!("{:.1}", tg_mean) } else { "—".to_string() }}</td>
                                            </tr>
                                        }.into_any()
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </section>
                    }.into_any()
                } else {
                    view! { <div></div> }.into_any()
                }
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // History
        {move || {
            if show_sig.get() && !history_sig.get().is_empty() {
                view! {
                    <section class="card mt-3">
                        <h3>"Benchmark History"</h3>
                        <table class="table table-striped">
                            <thead>
                                <tr>
                                    <th>"Date"</th>
                                    <th>"Model"</th>
                                    <th>"Quant"</th>
                                    <th>"PP sizes"</th>
                                    <th>"TG sizes"</th>
                                    <th>"Results"</th>
                                    <th>"Status"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {history_sig.get().into_iter().map(|entry| {
                                    let date = format_timestamp(entry.created_at);
                                    let badge_class = if entry.status == "success" {
                                        "badge badge-success"
                                    } else {
                                        "badge badge-danger"
                                    };
                                    view! {
                                        <tr>
                                            <td>{date}</td>
                                            <td>{entry.model_id}</td>
                                            <td>{entry.quant.unwrap_or_else(|| "—".to_string())}</td>
                                            <td class="text-mono">{entry.pp_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")}</td>
                                            <td class="text-mono">{entry.tg_sizes.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")}</td>
                                            <td>{entry.results_count}</td>
                                            <td><span class={badge_class}>{entry.status}</span></td>
                                        </tr>
                                    }.into_any()
                                }).collect::<Vec<_>>()}
                            </tbody>
                        </table>
                    </section>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}
    }
}
