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
    /// Per-model loaded/idle status mirrored from `koji_core::gpu::MetricSample.models`.
    ///
    /// `#[serde(default)]` keeps the dashboard resilient if the backend is
    /// slightly out of sync (e.g. during a partial rollout) or if older cached
    /// payloads without this field are encountered — missing arrays decode as
    /// an empty `Vec` rather than failing the whole sample.
    #[serde(default)]
    pub models: Vec<ModelStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VramInfo {
    used_mib: u64,
    total_mib: u64,
}

/// Frontend mirror of `koji_core::gpu::ModelStatus`.
///
/// Kept private to this module so the dashboard owns its wire shape; the only
/// contract with the backend is the JSON field names, which must match the
/// server-side struct exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelStatus {
    id: String,
    #[serde(default)]
    api_name: Option<String>,
    backend: String,
    loaded: bool,
}

/// Count how many models in `models` are currently loaded.
///
/// Extracted as a free function so the dashboard view and the unit tests can
/// share a single implementation — the view formats the result into the
/// "Active Models" summary line, and the tests assert the counting logic
/// without needing to render Leptos components.
fn loaded_model_count(models: &[ModelStatus]) -> usize {
    models.iter().filter(|m| m.loaded).count()
}

/// CSS class string used for the per-model status badge in the
/// "Active Models" grid. Loaded models get the success colour, idle ones
/// get the muted colour. Extracted so the rendering branch and the unit
/// tests share a single source of truth.
fn model_status_badge_class(loaded: bool) -> &'static str {
    if loaded {
        "badge badge-success"
    } else {
        "badge badge-muted"
    }
}

/// Human-readable label that pairs with [`model_status_badge_class`].
fn model_status_badge_label(loaded: bool) -> &'static str {
    if loaded {
        "Loaded"
    } else {
        "Idle"
    }
}

/// CSS class string for the load/unload action button in a model card.
/// Loaded models render a destructive "Unload" button (`btn-danger`),
/// idle models render an affirmative "Load" button (`btn-success`).
fn model_action_button_class(loaded: bool) -> &'static str {
    if loaded {
        "btn btn-danger btn-sm"
    } else {
        "btn btn-success btn-sm"
    }
}

/// Human-readable label that pairs with [`model_action_button_class`].
fn model_action_button_label(loaded: bool) -> &'static str {
    if loaded {
        "Unload"
    } else {
        "Load"
    }
}

/// Returns the preferred display name for a model, preferring `api_name` if
/// available, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelStatus) -> String {
    m.api_name.as_deref().unwrap_or(m.id.as_str()).to_string()
}

/// Partition model statuses into loaded and unloaded vectors, sorted by ID.
///
/// This is extracted to avoid duplicating the partition logic between the
/// "Loaded Models" and "Idle Models" sections.
fn partition_model_statuses(models: Vec<ModelStatus>) -> (Vec<ModelStatus>, Vec<ModelStatus>) {
    let (mut loaded, mut unloaded): (Vec<_>, Vec<_>) = models.into_iter().partition(|m| m.loaded);
    loaded.sort_by(|a, b| a.id.cmp(&b.id));
    unloaded.sort_by(|a, b| a.id.cmp(&b.id));
    (loaded, unloaded)
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

    // Per-model load/unload actions wired to the same REST endpoints used by
    // the `/models` page. Both actions are unsync because `gloo_net::Request`
    // returns `!Send` futures in the WASM target. We deliberately do **not**
    // refresh anything on completion: the dashboard's SSE stream pushes a new
    // `MetricSample` every tick, so the freshly toggled `loaded` flag flows
    // back into the UI without us having to manage cache invalidation here.
    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(|id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/koji/v1/models/{}/load", id))
                .send()
                .await;
        }
    });
    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(|id: &String| {
        let id = id.clone();
        async move {
            let _ = gloo_net::http::Request::post(&format!("/koji/v1/models/{}/unload", id))
                .send()
                .await;
        }
    });

    // Capture the pending signals once so the per-card buttons can disable
    // themselves while a load/unload request is in flight — this prevents
    // double-clicks from queuing duplicate requests against the proxy.
    let load_pending = load_action.pending();
    let unload_pending = unload_action.pending();

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

                    </div>

                    // Active Models section — replaces the old single-value
                    // "Models Loaded" card. Shows a summary line of how many
                    // models are loaded, and either an empty-state card or a
                    // grid of model entries.
                    //
                    // The wrapping `.dashboard-models` class hooks into the
                    // CSS in `style.css` (section 24) for vertical spacing
                    // between this section and the system metrics grid above.
                    // The inner `.page-header` reuses the global header layout
                    // (title on the left, status on the right) so the section
                    // visually matches every other page header in the app.
                    <section class="dashboard-models">
                        <div class="page-header">
                            <h2>"Active Models"</h2>
                            <span class="text-muted">
                                {format!("{} loaded", loaded_model_count(&h.models))}
                            </span>
                        </div>
                        {
                            if h.models.is_empty() {
                                view! {
                                    <div class="card card--centered">
                                        <p class="text-muted">"No models configured yet."</p>
                                    </div>
                                }.into_any()
                            } else {
                                // Partition models into loaded and idle sections
                                let (loaded, idle) = partition_model_statuses(h.models);
                                view! {
                                    // Plain wrapper div (NOT `.models-grid`) so the two
                                    // `.model-section` children stack vertically, matching
                                    // the Models page. The inner `.models-grid` inside each
                                    // section is what flows the model cards horizontally.
                                    <div>
                                        // Loaded models section
                                        {if !loaded.is_empty() {
                                            view! {
                                                <div class="model-section">
                                                    <h2 class="model-section__title">"Loaded Models"</h2>
                                                    <div class="models-grid">
                                                        {loaded.into_iter().map(|m| {
                                                            let id_for_action = m.id.clone();
                                                            let badge_class = model_status_badge_class(m.loaded);
                                                            let badge_label = model_status_badge_label(m.loaded);
                                                            let button_class = model_action_button_class(m.loaded);
                                                            let button_label = model_action_button_label(m.loaded);
                                                            view! {
                                                                <div class="model-card card">
                                                                    <div class="model-card__header">
                                                                        <span class="model-card__id">{model_display_name(&m)}</span>
                                                                        <span class={badge_class}>{badge_label}</span>
                                                                    </div>
                                                                    <div class="model-card__body">
                                                                        <div class="model-card__field">
                                                                            <span class="model-card__label">"Backend"</span>
                                                                            <span class="model-card__value text-mono">{m.backend}</span>
                                                                        </div>
                                                                    </div>
                                                                    <div class="model-card__actions">
                                                                        {if m.loaded {
                                                                            view! {
                                                                                <button
                                                                                    class={button_class}
                                                                                    prop:disabled=move || unload_pending.get()
                                                                                    on:click=move |_| { unload_action.dispatch(id_for_action.clone()); }
                                                                                >
                                                                                    {button_label}
                                                                                </button>
                                                                            }.into_any()
                                                                        } else {
                                                                            view! {
                                                                                <button
                                                                                    class={button_class}
                                                                                    prop:disabled=move || load_pending.get()
                                                                                    on:click=move |_| { load_action.dispatch(id_for_action.clone()); }
                                                                                >
                                                                                    {button_label}
                                                                                </button>
                                                                            }.into_any()
                                                                        }}
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            let _: () = view! { <></> };
                                            ().into_any()
                                        }}
                                        // Idle models section
                                        {if !idle.is_empty() {
                                            view! {
                                                <div class="model-section">
                                                    <h2 class="model-section__title">"Idle Models"</h2>
                                                    <div class="models-grid">
                                                        {idle.into_iter().map(|m| {
                                                            let id_for_action = m.id.clone();
                                                            let badge_class = model_status_badge_class(m.loaded);
                                                            let badge_label = model_status_badge_label(m.loaded);
                                                            let button_class = model_action_button_class(m.loaded);
                                                            let button_label = model_action_button_label(m.loaded);
                                                            view! {
                                                                <div class="model-card card">
                                                                    <div class="model-card__header">
                                                                        <span class="model-card__id">{model_display_name(&m)}</span>
                                                                        <span class={badge_class}>{badge_label}</span>
                                                                    </div>
                                                                    <div class="model-card__body">
                                                                        <div class="model-card__field">
                                                                            <span class="model-card__label">"Backend"</span>
                                                                            <span class="model-card__value text-mono">{m.backend}</span>
                                                                        </div>
                                                                    </div>
                                                                    <div class="model-card__actions">
                                                                        {if m.loaded {
                                                                            view! {
                                                                                <button
                                                                                    class={button_class}
                                                                                    prop:disabled=move || unload_pending.get()
                                                                                    on:click=move |_| { unload_action.dispatch(id_for_action.clone()); }
                                                                                >
                                                                                    {button_label}
                                                                                </button>
                                                                            }.into_any()
                                                                        } else {
                                                                            view! {
                                                                                <button
                                                                                    class={button_class}
                                                                                    prop:disabled=move || load_pending.get()
                                                                                    on:click=move |_| { load_action.dispatch(id_for_action.clone()); }
                                                                                >
                                                                                    {button_label}
                                                                                </button>
                                                                            }.into_any()
                                                                        }}
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>
                                            }.into_any()
                                        } else {
                                            let _: () = view! { <></> };
                                            ().into_any()
                                        }}
                                    </div>
                                }.into_any()
                            }
                        }
                    </section>
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `MetricSample` must deserialize a payload that has no `models` field at
    /// all (older backend builds, cached responses) by defaulting to an empty
    /// `Vec`. The `#[serde(default)]` attribute on the field is what makes this
    /// work — without it, deserialization would fail with a `missing field`
    /// error and break the dashboard during a partial rollout.
    #[test]
    fn metric_sample_deserializes_without_models_field() {
        let json = r#"{
            "ts_unix_ms": 1700000000000,
            "cpu_usage_pct": 12.5,
            "ram_used_mib": 2048,
            "ram_total_mib": 16384,
            "gpu_utilization_pct": null,
            "vram": null,
            "models_loaded": 0
        }"#;

        let sample: MetricSample = serde_json::from_str(json)
            .expect("MetricSample without `models` must deserialize via #[serde(default)]");

        assert_eq!(sample.ts_unix_ms, 1_700_000_000_000);
        assert_eq!(sample.cpu_usage_pct, 12.5);
        assert_eq!(sample.ram_used_mib, 2048);
        assert_eq!(sample.ram_total_mib, 16_384);
        assert!(sample.gpu_utilization_pct.is_none());
        assert!(sample.vram.is_none());
        assert_eq!(sample.models_loaded, 0);
        assert!(
            sample.models.is_empty(),
            "missing `models` field must default to an empty Vec"
        );
    }

    /// The dashboard's "Active Models" summary line shows how many of the
    /// configured models are currently loaded. The helper that backs that line
    /// must only count entries whose `loaded` flag is `true`, regardless of
    /// backend or id.
    #[test]
    fn loaded_model_count_only_counts_loaded_entries() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                api_name: None,
                backend: "llama_cpp".into(),
                loaded: true,
            },
            ModelStatus {
                id: "b".into(),
                api_name: None,
                backend: "llama_cpp".into(),
                loaded: false,
            },
            ModelStatus {
                id: "c".into(),
                api_name: None,
                backend: "ik_llama".into(),
                loaded: true,
            },
            ModelStatus {
                id: "d".into(),
                api_name: None,
                backend: "ik_llama".into(),
                loaded: false,
            },
        ];

        assert_eq!(loaded_model_count(&models), 2);
    }

    /// With no models configured the helper returns `0`, which is what the
    /// empty-state UI renders alongside the "No models configured yet." copy.
    #[test]
    fn loaded_model_count_is_zero_for_empty_slice() {
        assert_eq!(loaded_model_count(&[]), 0);
    }

    /// Loaded models must use the success badge class so they visually pop
    /// against idle entries in the Active Models grid.
    #[test]
    fn model_status_badge_class_uses_success_when_loaded() {
        assert_eq!(model_status_badge_class(true), "badge badge-success");
    }

    /// Idle models must use the muted badge class so they recede compared to
    /// loaded entries — matching the convention used elsewhere on the
    /// `/models` page.
    #[test]
    fn model_status_badge_class_uses_muted_when_idle() {
        assert_eq!(model_status_badge_class(false), "badge badge-muted");
    }

    /// Badge text mirrors the badge colour: "Loaded" for loaded models,
    /// "Idle" for everything else. Tests both branches so a future renaming
    /// can't silently drift one of them.
    #[test]
    fn model_status_badge_label_distinguishes_loaded_and_idle() {
        assert_eq!(model_status_badge_label(true), "Loaded");
        assert_eq!(model_status_badge_label(false), "Idle");
    }

    /// Loaded models surface an Unload action — destructive styling so the
    /// user understands clicking it tears down a running server.
    #[test]
    fn model_action_button_class_uses_danger_when_loaded() {
        assert_eq!(model_action_button_class(true), "btn btn-danger btn-sm");
    }

    /// Idle models surface a Load action — affirmative styling so the user
    /// understands clicking it spins up a server.
    #[test]
    fn model_action_button_class_uses_success_when_idle() {
        assert_eq!(model_action_button_class(false), "btn btn-success btn-sm");
    }

    /// Action button labels must match their visual styling: "Unload" for
    /// loaded models, "Load" for idle ones. Tests both branches so the
    /// label and class helpers stay in lockstep.
    #[test]
    fn model_action_button_label_distinguishes_loaded_and_idle() {
        assert_eq!(model_action_button_label(true), "Unload");
        assert_eq!(model_action_button_label(false), "Load");
    }

    /// When the backend includes a populated `models` array, every `ModelStatus`
    /// must round-trip with its `id`, `backend`, and `loaded` fields preserved.
    /// This is the wire format the dashboard's UI rendering depends on.
    #[test]
    fn metric_sample_deserializes_models_field() {
        let json = r#"{
            "ts_unix_ms": 1700000000000,
            "cpu_usage_pct": 0.0,
            "ram_used_mib": 0,
            "ram_total_mib": 0,
            "gpu_utilization_pct": null,
            "vram": null,
            "models_loaded": 1,
            "models": [
                { "id": "alpha", "api_name": "org/alpha", "backend": "llama_cpp", "loaded": true },
                { "id": "beta",  "api_name": "org/beta",  "backend": "ik_llama",  "loaded": false }
            ]
        }"#;

        let sample: MetricSample =
            serde_json::from_str(json).expect("MetricSample with `models` must deserialize");

        assert_eq!(sample.models.len(), 2);

        assert_eq!(sample.models[0].id, "alpha");
        assert_eq!(sample.models[0].api_name, Some("org/alpha".to_string()));
        assert_eq!(sample.models[0].backend, "llama_cpp");
        assert!(sample.models[0].loaded);

        assert_eq!(sample.models[1].id, "beta");
        assert_eq!(sample.models[1].api_name, Some("org/beta".to_string()));
        assert_eq!(sample.models[1].backend, "ik_llama");
        assert!(!sample.models[1].loaded);
    }
}
