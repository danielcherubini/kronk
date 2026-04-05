use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// Helper function to determine the color class based on percentage
fn color_for_pct(pct: f32) -> &'static str {
    if pct < 60.0 {
        "var(--accent-green)"
    } else if pct < 85.0 {
        "var(--accent-yellow)"
    } else {
        "var(--accent-red)"
    }
}

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
        let resp = gloo_net::http::Request::get("/kronk/v1/system/health")
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
        let _ = gloo_net::http::Request::post("/kronk/v1/system/restart")
            .send()
            .await;
    });

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            <Suspense>
                {move || {
                    health.get().map(|guard| {
                        let h = guard.take();
                        h.map(|h| view! {
                            <div class="flex-between gap-1">
                                <span class={format!("badge {}", status_badge_class(&h.status))}>{h.status.clone()}</span>
                                <button class="btn btn-secondary btn-sm" on:click=move |_| { restart.dispatch(()); }>"Restart"</button>
                            </div>
                        })
                    })
                }}
            </Suspense>
        </div>

        <Suspense fallback=|| view! {
            <div class="card card--centered">
                <span class="spinner">"Loading dashboard..."</span>
            </div>
        }>
            {move || {
                health.get().map(|guard| {
                    let h = guard.take();
                    match h {
                        Some(h) => view! {
                            <div class="grid-stats">
                                <div class="card">
                                    <div class="card-header">"CPU Usage"</div>
                                    <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                                    <div class="gauge">
                                        <div class="gauge-fill" style={format!("width:{}%; background:{}", h.cpu_usage_pct, color_for_pct(h.cpu_usage_pct))} />
                                    </div>
                                </div>

                                <div class="card">
                                    <div class="card-header">"Memory"</div>
                                    <div class="card-value">{format!("{} / {} MiB", h.ram_used_mib, h.ram_total_mib)}</div>
                                    <div class="gauge">
                                        <div class="gauge-fill" style={format!("width:{}%; background:var(--accent-blue)", if h.ram_total_mib > 0 { (h.ram_used_mib as f32 / h.ram_total_mib as f32) * 100.0 } else { 0.0 })} />
                                    </div>
                                </div>

                                {h.gpu_utilization_pct.map(|pct| view! {
                                    <div class="card">
                                        <div class="card-header">"GPU"</div>
                                        <div class="card-value">{format!("{}%", pct)}</div>
                                        <div class="gauge">
                                            <div class="gauge-fill" style={format!("width:{}%; background:{}", pct, color_for_pct(pct as f32))} />
                                        </div>
                                    </div>
                                })}

                                {h.vram.map(|v| {
                                    let usage_pct = if v.total_mib > 0 { (v.used_mib as f32 / v.total_mib as f32) * 100.0 } else { 0.0 };
                                    view! {
                                        <div class="card">
                                            <div class="card-header">"VRAM"</div>
                                            <div class="card-value">{format!("{} / {} MiB", v.used_mib, v.total_mib)}</div>
                                            <div class="gauge">
                                                <div class="gauge-fill" style={format!("width:{}%; background:{}", usage_pct, color_for_pct(usage_pct))} />
                                            </div>
                                        </div>
                                    }
                                })}

                                <div class="card">
                                    <div class="card-header">"Models Loaded"</div>
                                    <div class="card-value">{h.models_loaded}</div>
                                </div>
                            </div>
                        }.into_any(),
                        None => view! {
                            <div class="card">
                                <p class="text-error">"Failed to load health data. Is Kronk running?"</p>
                                <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
                            </div>
                        }.into_any(),
                    }
                })
            }}
        </Suspense>
    }
}
