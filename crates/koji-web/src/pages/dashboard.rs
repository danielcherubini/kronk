use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::components::sparkline::SparklineChart;

// Helper function to determine the status badge class
fn status_badge_class(status: &str) -> &'static str {
    match status {
        "ok" => "badge-success",
        "degraded" => "badge-warning",
        _ => "badge-error",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SystemHealth {
    status: String,
    service: String,
    models_loaded: usize,
    cpu_usage_pct: f32,
    ram_used_mib: u64,
    ram_total_mib: u64,
    gpu_utilization_pct: Option<u8>,
    vram: Option<VramInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VramInfo {
    used_mib: u64,
    total_mib: u64,
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let refresh = RwSignal::new(0u32);
    let history = RwSignal::new(Vec::<SystemHealth>::new());
    let fetch_failed = RwSignal::new(false);

    // Auto-refresh every 3 seconds using web_sys interval; clear on cleanup to prevent leaks.
    let cb = Closure::<dyn Fn()>::new(move || {
        refresh.update(|n| *n += 1);
    });
    let interval_id = web_sys::window()
        .unwrap()
        .set_interval_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 3_000)
        .unwrap();
    cb.forget(); // keep closure alive

    on_cleanup(move || {
        web_sys::window()
            .unwrap()
            .clear_interval_with_handle(interval_id);
    });

    // Re-fetch when refresh signal changes.
    let health = LocalResource::new(move || async move {
        let _ = refresh.get(); // track the signal
        let resp = gloo_net::http::Request::get("/koji/v1/system/health")
            .send()
            .await
            .ok()?;
        resp.json::<SystemHealth>().await.ok()
    });

    // Action for manual refresh/retry
    let manual_refresh = move |_| {
        refresh.update(|n| *n += 1);
    };

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        let _ = gloo_net::http::Request::post("/koji/v1/system/restart")
            .send()
            .await;
    });

    // Effect to accumulate health snapshots into history ring buffer
    Effect::new(move |_| {
        if let Some(guard) = health.get() {
            if let Some(h) = (*guard).clone() {
                fetch_failed.set(false);
                history.update(|buf| {
                    buf.push(h);
                    if buf.len() > 100 {
                        buf.drain(..buf.len() - 100);
                    }
                });
            } else {
                fetch_failed.set(true);
            }
        }
    });

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            {move || {
                history.get().last().cloned().map(|h| view! {
                    <div class="flex-between gap-1">
                        <span class={format!("badge {}", status_badge_class(&h.status))}>{h.status.clone()}</span>
                        <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>"Restart"</button>
                    </div>
                })
            }}
        </div>

        {move || {
            let buf = history.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <p class="text-error">"Failed to load health data. Is Koji running?"</p>
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
