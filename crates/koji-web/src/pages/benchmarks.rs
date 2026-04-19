//! Benchmarks page — run llama-bench benchmarks from the web UI.

use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRequest {
    pub model_id: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub warmup: u32,
    pub threads: Option<Vec<u32>>,
    pub ngl_range: Option<String>,
    pub ctx_override: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub pp_sizes: Vec<u32>,
    pub tg_sizes: Vec<u32>,
    pub runs: u32,
    pub results_count: usize,
    pub status: String,
}

/// Preset configurations for quick benchmark setup.
#[derive(Debug, Clone)]
pub struct BenchmarkPreset {
    pub label: &'static str,
    pub pp_sizes: &'static [u32],
    pub tg_sizes: &'static [u32],
    pub runs: u32,
    pub threads: Option<Vec<u32>>,
    pub ngl_range: Option<&'static str>,
    #[allow(dead_code)]
    pub ctx_override: Option<u32>,
}

impl BenchmarkPreset {
    pub fn all() -> Vec<Self> {
        vec![
            Self {
                label: "Quick",
                pp_sizes: &[512],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: None,
                ctx_override: None,
            },
            Self {
                label: "VRAM Sweet Spot",
                pp_sizes: &[512],
                tg_sizes: &[128],
                runs: 3,
                threads: None,
                ngl_range: Some("0-99+1"),
                ctx_override: Some(4096),
            },
            Self {
                label: "Thread Scaling",
                pp_sizes: &[64],
                tg_sizes: &[16],
                runs: 3,
                threads: Some(vec![1, 2, 4, 8, 16, 32]),
                ngl_range: None,
                ctx_override: None,
            },
        ]
    }
}

/// Format a Unix timestamp to "YYYY-MM-DD HH:MM" using js_sys (WASM-compatible).
fn format_timestamp(ts: i64) -> String {
    // Compute day offset from Unix timestamp (seconds since epoch)
    let secs = ts as u64;
    let days_since_epoch = (secs / 60 / 60 / 24) as i64;

    // Compute year, month, day from days since Unix epoch (1970-01-01)
    let mut days = days_since_epoch;
    let mut year: i64 = 1970;
    loop {
        let ydays = if is_leap_year(year) { 366i64 } else { 365i64 };
        if days < ydays {
            break;
        }
        days -= ydays;
        year += 1;
    }
    let leap = is_leap_year(year);
    let month_lengths: [i32; 12] = match leap {
        true => [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
        false => [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    };
    let mut month_idx: i32 = 0;
    for (i, &ml) in month_lengths.iter().enumerate() {
        if days < ml as i64 {
            month_idx = i as i32;
            break;
        }
        days -= ml as i64;
    }
    let day = (days + 1) as i32;

    // Verify with js_sys Date to handle timezone correctly
    let date = js_sys::Date::new_with_year_month_day(year as u32, month_idx, day);
    let month = date.get_month() + 1;
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        month,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

/// Format a stat as "mean ± stddev" or just "mean" if stddev is 0.
#[allow(dead_code)]
fn format_stat(mean: f64, stddev: f64) -> String {
    if stddev > 0.01 {
        format!("{:.1} ± {:.1}", mean, stddev)
    } else {
        format!("{:.1}", mean)
    }
}

#[component]
pub fn Benchmarks() -> impl IntoView {
    // Model selection
    let selected_model = RwSignal::new(String::new());
    let available_models = RwSignal::new(Vec::<(String, String, String)>::new()); // (id, display_name, quant)

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
    let job_status = RwSignal::new(String::new());
    let has_results = RwSignal::new(false);
    let error_message = RwSignal::new(Option::<String>::None);

    // History state
    let history = RwSignal::new(Vec::<HistoryEntry>::new());
    let show_history = RwSignal::new(false);

    // Fetch available models on mount
    {
        spawn_local(async move {
            if let Ok(resp) = gloo_net::http::Request::get("/koji/v1/models").send().await {
                if let Ok(models) = resp.json::<Vec<serde_json::Value>>().await {
                    let model_list: Vec<(String, String, String)> = models
                        .iter()
                        .filter_map(|m| {
                            let id = m.get("id")?.as_str()?.to_string();
                            let name = m
                                .get("display_name")
                                .or_else(|| m.get("api_name"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| id.clone());
                            let quant = m
                                .get("quant")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            Some((id, name, quant))
                        })
                        .collect();
                    available_models.update(|list| *list = model_list);
                }
            }
        });
    }

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

    // Apply preset
    let apply_preset = move |preset: BenchmarkPreset| {
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

    // Connect to SSE for a given job_id
    let connect_to_sse = move |job_id: String| {
        let log_lines_signal = log_lines;
        let status_signal = job_status;
        let is_running_signal = is_running;
        let has_results_signal = has_results;
        let error_signal = error_message;

        spawn_local(async move {
            let es =
                match web_sys::EventSource::new(&format!("/api/benchmarks/jobs/{}/events", job_id))
                {
                    Ok(es) => es,
                    Err(_) => return,
                };

            // Handle log events
            let on_log =
                Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                    if let Some(data_str) = evt.data().as_string() {
                        if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&data_str)
                        {
                            if event_json.get("type").and_then(|t| t.as_str()) == Some("log") {
                                if let Some(line) = event_json.get("line").and_then(|l| l.as_str())
                                {
                                    log_lines_signal.update(|lines| {
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
                        if let Ok(event_json) = serde_json::from_str::<serde_json::Value>(&data_str)
                        {
                            if let Some(status) = event_json.get("status").and_then(|s| s.as_str())
                            {
                                status_signal.set(status.to_string());
                                let terminal = status == "Succeeded" || status == "Failed";
                                is_running_signal.set(!terminal);
                                has_results_signal.set(terminal);
                                if status == "Failed" {
                                    error_signal.set(Some(
                                        "Benchmark failed. Check logs above.".to_string(),
                                    ));
                                }
                            }
                        }
                    }
                });
            let _ =
                es.add_event_listener_with_callback("status", on_status.as_ref().unchecked_ref());
            on_status.forget();

            on_cleanup(move || {
                es.close();
            });
        });
    };

    // Run benchmark action (unsync because gloo_net returns !Send futures in WASM)
    let run_action: Action<BenchmarkRequest, (), LocalStorage> =
        Action::new_unsync(move |req: &BenchmarkRequest| {
            let req_clone = req.clone();
            async move {
                // Serialize request body
                let body = serde_json::json!({
                    "model_id": req_clone.model_id,
                    "pp_sizes": req_clone.pp_sizes,
                    "tg_sizes": req_clone.tg_sizes,
                    "runs": req_clone.runs,
                    "warmup": req_clone.warmup,
                    "threads": req_clone.threads,
                    "ngl_range": req_clone.ngl_range,
                    "ctx_override": req_clone.ctx_override,
                });
                // Submit benchmark job
                if let Ok(builder) = gloo_net::http::Request::post("/api/benchmarks/run")
                    .header("Content-Type", "application/json")
                    .body(body.to_string())
                {
                    if let Ok(resp) = builder.send().await {
                        if let Ok(body) = resp.json::<serde_json::Value>().await {
                            if let Some(job_id) = body.get("job_id").and_then(|v| v.as_str()) {
                                // Connect to SSE for progress streaming
                                connect_to_sse(job_id.to_string());
                            }
                        }
                    }
                }
            }
        });

    // Parse comma-separated strings into Vec<u32> for the request
    let parse_sizes = move |s: &str| -> Vec<u32> {
        s.split(',')
            .map(|v| v.trim().parse::<u32>().unwrap_or(0))
            .filter(|v| *v > 0)
            .collect()
    };

    // Parse threads string
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
                    let models = available_models.get();
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
                        prop:value=move || pp_sizes_str.get()
                        on:input=move |e| { pp_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 128,256,512"</small>
                </div>
                <div class="form-group">
                    <label>"Generation lengths (tokens)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || tg_sizes_str.get()
                        on:input=move |e| { tg_sizes_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"Comma-separated, e.g. 32,64,128"</small>
                </div>
                <div class="form-group">
                    <label>"Runs"</label>
                    <input
                        type="number"
                        class="form-control"
                        prop:value=move || runs.get()
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
                        prop:value=move || warmup.get()
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
                        prop:value=move || threads_str.get()
                        on:input=move |e| { threads_str.set(e.target().unwrap().dyn_into::<web_sys::HtmlInputElement>().unwrap().value()); }
                    />
                    <small class="text-muted">"auto, or comma-separated e.g. 4,8,16"</small>
                </div>
                <div class="form-group">
                    <label>"GPU layers range (sweet spot)"</label>
                    <input
                        type="text"
                        class="form-control"
                        prop:value=move || ngl_range.get()
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
                            on:click=move |_| apply_preset(preset.clone())
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
                prop:disabled=move || selected_model.get().is_empty() || is_running.get()
                on:click=move |_| {
                    let pp = parse_sizes(&pp_sizes_str.get());
                    let tg = parse_sizes(&tg_sizes_str.get());
                    let threads = parse_threads(&threads_str.get());
                    let ngl = if ngl_range.get().is_empty() { None } else { Some(ngl_range.get()) };
                    let ctx = if ctx_override.get().is_empty() { None } else {
                        ctx_override.get().parse::<u32>().ok()
                    };

                    let _ = run_action.dispatch(BenchmarkRequest {
                        model_id: selected_model.get(),
                        pp_sizes: pp,
                        tg_sizes: tg,
                        runs: runs.get(),
                        warmup: warmup.get(),
                        threads,
                        ngl_range: ngl,
                        ctx_override: ctx,
                    });
                }
            >
                {move || if is_running.get() { "Running..." } else { "▶ Run Benchmark" }}
            </button>
        </div>

        // Progress / logs
        {move || {
            if !log_lines.get().is_empty() {
                view! {
                    <section class="card">
                        <h3>"Progress"</h3>
                        <div class="log-panel">
                            {log_lines.get().into_iter().map(|line| {
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
            if let Some(err) = error_message.get() {
                view! {
                    <div class="alert alert-danger mt-3">{err}</div>
                }.into_any()
            } else {
                view! { <div></div> }.into_any()
            }
        }}

        // History
        {move || {
            if show_history.get() && !history.get().is_empty() {
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
                                {history.get().into_iter().map(|entry| {
                                    let date = format_timestamp(entry.created_at);
                                    let badge_class = if entry.status == "success" { "badge badge-success" } else { "badge badge-danger" };
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
