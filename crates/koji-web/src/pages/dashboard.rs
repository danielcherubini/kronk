use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::components::sparkline::SparklineChart;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricSample {
    ts_unix_ms: i64,
    cpu_usage_pct: f32,
    ram_used_mib: u64,
    ram_total_mib: u64,
    gpu_utilization_pct: Option<u8>,
    vram: Option<VramInfo>,
    models_loaded: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VramInfo {
    used_mib: u64,
    total_mib: u64,
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let history = RwSignal::new(Vec::<MetricSample>::new());
    let fetch_failed = RwSignal::new(false);
    // Incrementing this signal re-runs the Effect that opens the EventSource.
    let connect_trigger = RwSignal::new(0u32);

    // Open (or re-open) an EventSource each time connect_trigger changes.
    Effect::new(move |_| {
        let _ = connect_trigger.get(); // track signal

        let es = match web_sys::EventSource::new("/koji/v1/system/metrics/stream") {
            Ok(es) => es,
            Err(_) => {
                fetch_failed.set(true);
                return;
            }
        };

        // Handler for "sample" events.
        let on_sample =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    if let Ok(sample) = serde_json::from_str::<MetricSample>(&data_str) {
                        fetch_failed.set(false);
                        history.update(|buf| {
                            buf.push(sample);
                            if buf.len() > 100 {
                                buf.drain(..buf.len() - 100);
                            }
                        });
                    }
                }
            });
        let _ = es.add_event_listener_with_callback("sample", on_sample.as_ref().unchecked_ref());
        on_sample.forget();

        // Error handler — flag for the empty-history retry UI.
        let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            fetch_failed.set(true);
        });
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        // Close the EventSource when the effect re-runs or the component unmounts.
        on_cleanup(move || {
            es.close();
        });
    });

    // Manual retry: close and re-open the EventSource.
    let manual_refresh = move |_| {
        fetch_failed.set(false);
        connect_trigger.update(|n| *n += 1);
    };

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        let _ = gloo_net::http::Request::post("/koji/v1/system/restart")
            .send()
            .await;
    });

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            {move || {
                history.get().last().cloned().map(|_h| {
                    let badge_class = if fetch_failed.get() { "badge badge-danger" } else { "badge badge-success" };
                    let badge_text = if fetch_failed.get() { "error" } else { "ok" };
                    view! {
                        <div class="flex-between gap-1">
                            <span class={badge_class}>{badge_text}</span>
                            <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>
                                "Restart"
                            </button>
                        </div>
                    }
                })
            }}
        </div>

        {move || {
            let buf = history.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <p class="text-error">"Failed to load metrics stream. Is Koji running?"</p>
                        <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
                    </div>
                }.into_any();
            }
            match buf.last().cloned() {
                Some(h) => view! {
                    <div class="grid-stats">
                        // CPU card
                        <div class="card">
                            <div class="card-header">"CPU Usage"</div>
                            <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                            <SparklineChart
                                data={buf.iter().map(|s| s.cpu_usage_pct).collect::<Vec<f32>>()}
                                max_value=100.0
                                color="var(--accent-green)".to_string()
                                height=60.0
                            />
                        </div>

                        // Memory card
                        <div class="card">
                            <div class="card-header">"Memory"</div>
                            <div class="card-value">{format!("{} / {} MiB", h.ram_used_mib, h.ram_total_mib)}</div>
                            <SparklineChart
                                data={buf.iter().map(|s| s.ram_used_mib as f32).collect::<Vec<f32>>()}
                                max_value={h.ram_total_mib as f32}
                                color="var(--accent-blue)".to_string()
                                height=60.0
                            />
                        </div>

                        // GPU card — only rendered if GPU data is present in the latest snapshot.
                        // For the data Vec, use .map() with unwrap_or(0) instead of .filter_map()
                        // to keep time-axis aligned with other charts.
                        {h.gpu_utilization_pct.map(|pct| view! {
                            <div class="card">
                                <div class="card-header">"GPU"</div>
                                <div class="card-value">{format!("{}%", pct)}</div>
                                <SparklineChart
                                    data={buf.iter().map(|s| s.gpu_utilization_pct.unwrap_or(0) as f32).collect::<Vec<f32>>()}
                                    max_value=100.0
                                    color="var(--accent-yellow)".to_string()
                                    height=60.0
                                />
                            </div>
                        })}

                        // VRAM card — only rendered if VRAM data is present in the latest snapshot.
                        {h.vram.as_ref().map(|v| {
                            let total = v.total_mib as f32;
                            view! {
                                <div class="card">
                                    <div class="card-header">"VRAM"</div>
                                    <div class="card-value">{format!("{} / {} MiB", v.used_mib, v.total_mib)}</div>
                                    <SparklineChart
                                        data={buf.iter().map(|s| s.vram.as_ref().map(|v| v.used_mib as f32).unwrap_or(0.0)).collect::<Vec<f32>>()}
                                        max_value=total
                                        color="var(--accent-purple)".to_string()
                                        height=60.0
                                    />
                                </div>
                            }
                        })}

                        // Models Loaded — keep as simple number, no chart
                        <div class="card">
                            <div class="card-header">"Models Loaded"</div>
                            <div class="card-value">{h.models_loaded}</div>
                        </div>
                    </div>
                }.into_any(),
                None => view! {
                    <div class="card card--centered">
                        <span class="spinner">"Loading dashboard..."</span>
                    </div>
                }.into_any(),
            }
        }}
    }
}
