//! Benchmarks page — run llama-bench benchmarks from the web UI.

mod types;
mod utils;

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

use self::types::{BenchmarkPreset, HistoryEntry};
use self::utils::format_timestamp;

#[component]
pub fn Benchmarks() -> impl IntoView {
    // Model selection
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<(String, String, String)>::new());

    // Test configuration
    let pp_sizes_str = RwSignal::new("512".to_string());
    let tg_sizes_str = RwSignal::new("128".to_string());
    let runs = RwSignal::new(3u32);
    let warmup = RwSignal::new(1u32);
    let threads_str = RwSignal::new("auto".to_string());
    let ngl_range = RwSignal::new("".to_string());
    let ctx_override = RwSignal::new("".to_string());

    // Job state
    let is_running = RwSignal::new(false);
    let log_lines = RwSignal::new(Vec::<String>::new());
    let _job_status = RwSignal::new(String::new());
    let has_results = RwSignal::new(false);
    let error_message = RwSignal::new(Option::<String>::None);

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

        // Clone signals for async block (RwSignal is Copy)
        let log_lines = log_lines;
        let job_status = _job_status;
        let is_running = is_running;
        let has_results = has_results;
        let error_message = error_message;

        spawn_local(async move {
            let body = serde_json::json!({
                "model_id": model_id,
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
                            connect_to_sse(
                                job_id.to_string(),
                                log_lines,
                                job_status,
                                is_running,
                                has_results,
                                error_message,
                            );
                        }
                    }
                }
            }
        });
    };

    // Split signals for read-only view access (done before closures above capture them)
    let (available_models_sig, _) = available_models.split();
    let (selected_model_sig, _) = selected_model.split();
    let (pp_sizes_sig, _) = pp_sizes_str.split();
    let (tg_sizes_sig, _) = tg_sizes_str.split();
    let (runs_sig, _) = runs.split();
    let (warmup_sig, _) = warmup.split();
    let (threads_sig, _) = threads_str.split();
    let (ngl_sig, _) = ngl_range.split();
    let (log_lines_sig, _) = log_lines.split();
    let (error_sig, _) = error_message.split();
    let (show_sig, _) = show_history.split();
    let (history_sig, _) = history.split();
    let (is_running_sig, _) = is_running.split();

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

        // Progress / logs
        {move || {
            if !log_lines_sig.get().is_empty() {
                view! {
                    <section class="card">
                        <h3>"Progress"</h3>
                        <div class="log-panel">
                            {log_lines_sig.get().into_iter().map(|line| {
                                view! {
                                    <pre class="log-line">{line}</pre>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </div>
                    </section>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // Error message
        {move || {
            if let Some(err) = error_sig.get() {
                view! {
                    <div class="alert alert-danger mt-3">{err}</div>
                }.into_any()
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

/// Connect to SSE for a given job_id to receive progress updates.
fn connect_to_sse(
    job_id: String,
    log_lines: RwSignal<Vec<String>>,
    status_signal: RwSignal<String>,
    is_running_signal: RwSignal<bool>,
    has_results_signal: RwSignal<bool>,
    error_signal: RwSignal<Option<String>>,
) {
    spawn_local(async move {
        let es = match web_sys::EventSource::new(&format!("/api/benchmarks/jobs/{}/events", job_id))
        {
            Ok(es) => es,
            Err(_) => return,
        };

        // Handle log events
        let on_log =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        if event_json.get("type").and_then(|t| t.as_str()) == Some("log") {
                            if let Some(line) = event_json.get("line").and_then(|l| l.as_str()) {
                                log_lines.update(|lines| {
                                    lines.push(line.to_string());
                                });
                            }
                        }
                    }
                }
            });
        let _ = es.add_event_listener_with_callback("log", on_log.as_ref().unchecked_ref());
        on_log.forget();

        // Handle status events
        let on_status =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&data_str) {
                        if let Some(status) = event_json.get("status").and_then(|s| s.as_str()) {
                            status_signal.set(status.to_string());
                            let terminal = status == "Succeeded" || status == "Failed";
                            is_running_signal.set(!terminal);
                            has_results_signal.set(terminal);
                            if status == "Failed" {
                                error_signal
                                    .set(Some("Benchmark failed. Check logs above.".to_string()));
                            }
                        }
                    }
                }
            });
        let _ = es.add_event_listener_with_callback("status", on_status.as_ref().unchecked_ref());
        on_status.forget();

        on_cleanup(move || {
            es.close();
        });
    });
}
