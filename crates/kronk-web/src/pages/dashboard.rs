use leptos::prelude::*;
use serde::{Deserialize, Serialize};

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
    let health = LocalResource::new(|| async move {
        let resp = gloo_net::http::Request::get("/kronk/v1/system/health")
            .send()
            .await
            .ok()?;
        resp.json::<SystemHealth>().await.ok()
    });

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        let _ = gloo_net::http::Request::post("/kronk/v1/system/restart")
            .send()
            .await;
    });

    view! {
        <h1>"Dashboard"</h1>
        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || {
                health.get().map(|h| {
                    // Deref the SendWrapper to get the inner Option<SystemHealth>
                    let h = h.take();
                    match h {
                        Some(h) => view! {
                            <p>"Status: " {h.status}</p>
                            <p>"Models loaded: " {h.models_loaded}</p>
                            <p>{format!("CPU: {:.1}%", h.cpu_usage_pct)}</p>
                            <p>{format!("RAM: {} / {} MiB", h.ram_used_mib, h.ram_total_mib)}</p>
                            {h.gpu_utilization_pct.map(|pct| view! {
                                <p>{format!("GPU: {}%", pct)}</p>
                            })}
                            {h.vram.map(|v| view! {
                                <p>{format!("VRAM: {} / {} MiB", v.used_mib, v.total_mib)}</p>
                            })}
                        }.into_any(),
                        None => view! { <p>"Failed to load health data"</p> }.into_any(),
                    }
                })
            }}
        </Suspense>
        <button on:click=move |_| { restart.dispatch(()); }>"Restart Kronk"</button>
    }
}
