use js_sys::{Date, Function, Reflect};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::components::modal::Modal;
use crate::components::pull_quant_wizard::{CompletedQuant, PullQuantWizard};
use crate::components::sparkline::SparklineChart;
use crate::utils::{
    extract_and_store_csrf_token, post_request, rw_signal_to_signal, CheckAllModelsApiResponse,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MetricSample {
    ts_unix_ms: i64,
    cpu_usage_pct: f32,
    ram_used_mib: u64,
    ram_total_mib: u64,
    gpu_utilization_pct: Option<u8>,
    vram: Option<VramInfo>,
    models_loaded: u64,
    /// Per-model loaded/idle status mirrored from `tama_core::gpu::MetricSample.models`.
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

/// Frontend mirror of the backend `MetricsHistoryEntry` response type.
///
/// Uses `i64` for memory and GPU fields to match the JSON wire format
/// (SQLite stores integers as i64). Converted to `MetricSample` on ingestion.
#[derive(Debug, Clone, Deserialize)]
struct MetricsHistoryEntry {
    ts_unix_ms: i64,
    cpu_usage_pct: f32,
    ram_used_mib: i64,
    ram_total_mib: i64,
    gpu_utilization_pct: Option<i64>,
    vram_used_mib: Option<i64>,
    vram_total_mib: Option<i64>,
}

impl From<MetricsHistoryEntry> for MetricSample {
    fn from(entry: MetricsHistoryEntry) -> Self {
        MetricSample {
            ts_unix_ms: entry.ts_unix_ms,
            cpu_usage_pct: entry.cpu_usage_pct,
            ram_used_mib: entry.ram_used_mib as u64,
            ram_total_mib: entry.ram_total_mib as u64,
            gpu_utilization_pct: entry.gpu_utilization_pct.map(|v| v as u8),
            vram: entry.vram_used_mib.and_then(|used| {
                entry.vram_total_mib.map(|total| VramInfo {
                    used_mib: used as u64,
                    total_mib: total as u64,
                })
            }),
            models_loaded: 0,
            models: vec![],
        }
    }
}

/// Frontend mirror of `tama_core::gpu::ModelStatus`.
///
/// Kept private to this module so the dashboard owns its wire shape; the only
/// contract with the backend is the JSON field names, which must match the
/// server-side struct exactly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(deprecated)]
struct ModelStatus {
    id: String,
    #[serde(default)]
    db_id: Option<i64>,
    #[serde(default)]
    api_name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    backend: String,
    #[deprecated(since = "1.45.0", note = "use state field instead")]
    #[serde(default)]
    loaded: bool,
    /// Lifecycle state: idle, loading, ready, unloading, failed.
    #[serde(default)]
    state: String,
    #[serde(default)]
    quant: Option<String>,
    #[serde(default)]
    context_length: Option<u32>,
}

/// Format a number with comma separators (e.g. `8460` → `"8,460"`).
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.insert(0, ',');
        }
        result.insert(0, c);
    }
    result
}

/// Filter models to only those that are currently active (ready, loading, or unloading).
///
/// Used by the dashboard to render the Active Models list and by the
/// "X loaded" summary heading. Extracted as a free function so it can
/// be unit-tested independently of the Leptos reactive view.
fn active_models(models: &[ModelStatus]) -> Vec<ModelStatus> {
    models
        .iter()
        .filter(|m| matches!(m.state.as_str(), "ready" | "loading" | "unloading"))
        .cloned()
        .collect()
}

/// Returns models whose state is NOT one of the "active" states.
/// These are models that are idle, failed, or otherwise not running.
/// Note: Models with an empty state string are treated as inactive.
/// This matches the behavior of `active_models()` which only considers
/// "ready", "loading", and "unloading" as active states.
fn inactive_models(models: &[ModelStatus]) -> Vec<ModelStatus> {
    models
        .iter()
        .filter(|m| !matches!(m.state.as_str(), "ready" | "loading" | "unloading"))
        .cloned()
        .collect()
}

/// CSS class string used for the per-model status badge in the
/// "Active Models" grid. Maps lifecycle states to colour classes.
fn model_status_badge_class(state: &str) -> &'static str {
    match state {
        "ready" => "badge badge-success",
        "loading" => "badge badge-info",
        "unloading" => "badge badge-warning",
        "failed" => "badge badge-error",
        _ => "badge badge-muted",
    }
}

/// Human-readable label that pairs with [`model_status_badge_class`].
fn model_status_badge_label(state: &str) -> &'static str {
    match state {
        "ready" => "Loaded",
        "loading" => "Loading",
        "unloading" => "Unloading",
        "failed" => "Failed",
        _ => "Idle",
    }
}

/// CSS class string for the load/unload action button in a model card.
/// Ready models render an "Unload" button (btn-danger),
/// loading/unloading/failed show muted buttons,
/// idle shows a "Load" button (btn-success).
fn model_action_button_class(state: &str) -> &'static str {
    match state {
        "ready" => "btn btn-danger btn-sm",
        "loading" => "btn btn-secondary btn-sm",
        "unloading" => "btn btn-secondary btn-sm",
        "failed" => "btn btn-warning btn-sm",
        _ => "btn btn-success btn-sm",
    }
}

/// Human-readable label that pairs with [`model_action_button_class`].
fn model_action_button_label(state: &str) -> &'static str {
    match state {
        "ready" => "Unload",
        "loading" => "Loading…",
        "unloading" => "Unloading…",
        "failed" => "Retry",
        _ => "Load",
    }
}

/// Returns the preferred display name for a model, preferring `display_name`,
/// then `api_name`, falling back to the model `id` otherwise.
fn model_display_name(m: &ModelStatus) -> String {
    m.display_name
        .as_deref()
        .or(m.api_name.as_deref())
        .unwrap_or(m.id.as_str())
        .to_string()
}

/// Pre-computed display values for a model row, used to deduplicate
/// the Active and Inactive model section rendering logic.
struct ModelDisplayData {
    id: String,
    db_id: Option<i64>,
    display_name: String,
    quant_display: String,
    context_display: String,
    backend_name: String,
    state: String,
}

/// Format context length in human-readable form (e.g., 8192 → "8k", 32768 → "32k").
/// Uses 1024 for binary kilobytes (KiB) and 1000 for decimal kilobytes (kB)
/// to handle both conventions used by different backends.
fn format_context_length(n: u32) -> String {
    const BINARY_K: u32 = 1024;
    const DECIMAL_K: u32 = 1000;
    if n >= BINARY_K && n.is_multiple_of(BINARY_K) {
        format!("{}k", n / BINARY_K)
    } else if n >= DECIMAL_K && n.is_multiple_of(DECIMAL_K) {
        format!("{}k", n / DECIMAL_K)
    } else {
        n.to_string()
    }
}

/// Normalize a slice of models: sort by id and compute display values.
///
/// Used by both the Active and Inactive model sections to deduplicate
/// the rendering logic. Returns models sorted by id in stable order.
fn normalize_models(models: &[ModelStatus]) -> Vec<ModelDisplayData> {
    let mut sorted: Vec<_> = models.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    sorted
        .into_iter()
        .map(|m: &ModelStatus| {
            let display_name = model_display_name(m);
            let quant_display: String = m.quant.as_deref().unwrap_or("\u{2014}").into();
            let context_display = m
                .context_length
                .map(format_context_length)
                .unwrap_or_else(|| "—".to_string());
            let backend_name = format!("{}_{}", m.backend, m.id);
            ModelDisplayData {
                id: m.id.clone(),
                db_id: m.db_id,
                display_name,
                quant_display,
                context_display,
                backend_name,
                state: m.state.clone(),
            }
        })
        .collect()
}

/// Merge new metric samples into the buffer.
/// Combines, sorts by timestamp, deduplicates (keeping the FIRST entry for each timestamp),
/// and trims to the last `max_len` samples.
///
/// Keeping the first entry is intentional: SSE entries (which include `models` data)
/// are already in the buffer, and backfill entries (which have `models: vec![]`)
/// are extended after. Keeping the first preserves the richer SSE entry.
fn merge_samples(buf: &mut Vec<MetricSample>, new: Vec<MetricSample>, max_len: usize) {
    buf.extend(new);
    buf.sort_by_key(|s| s.ts_unix_ms);
    buf.dedup_by(|a, b| a.ts_unix_ms == b.ts_unix_ms); // keeps a (first), removes b (subsequent)
    if buf.len() > max_len {
        buf.drain(..buf.len() - max_len);
    }
}

/// Fetch metric history from the backend and merge into the history signal.
///
/// Applies a 5-second cooldown (tracked by `last_backfill`) to avoid
/// redundant requests. Used by both the SSE `lagged` handler and the
/// `visibilitychange` handler so both paths behave identically.
async fn backfill_metrics(history: RwSignal<Vec<MetricSample>>, last_backfill: RwSignal<u64>) {
    // Cooldown: skip if backfilled in the last 5 seconds
    let now = Date::now() as u64;
    if (now - last_backfill.get()) < 5000 {
        return;
    }
    last_backfill.set(now);

    let url = "/tama/v1/system/metrics/history?limit=200";
    match gloo_net::http::Request::get(url).send().await {
        Ok(resp) => {
            extract_and_store_csrf_token(&resp);
            match resp.json::<Vec<MetricsHistoryEntry>>().await {
                Ok(entries) => {
                    let new: Vec<MetricSample> = entries.into_iter().map(Into::into).collect();
                    if !new.is_empty() {
                        history.update(|buf| {
                            merge_samples(buf, new, 450);
                        });
                    }
                }
                Err(e) => warn!("backfill: failed to parse history JSON: {}", e),
            }
        }
        Err(e) => warn!("backfill: failed to fetch /metrics/history: {}", e),
    }
}

/// Renders a single model row. Isolated component so only changed rows rebuild
/// when metrics update — prevents the entire list from being destroyed/recreated.
#[component]
fn ModelRow(
    id: String,
    db_id: Option<i64>,
    display_name: String,
    quant_display: String,
    context_display: String,
    backend_name: String,
    state: String,
    load_pending: RwSignal<bool>,
    unload_pending: RwSignal<bool>,
    on_load: Callback<String>,
    on_unload: Callback<String>,
) -> impl IntoView {
    let badge_class = model_status_badge_class(&state);
    let badge_label = model_status_badge_label(&state);
    let button_class = model_action_button_class(&state);
    let button_label = model_action_button_label(&state);
    // Clone values needed in closures before they're consumed by the view!
    let id_for_load = id.clone();
    let id_for_edit = id.clone();
    let id_for_unload = id.clone();
    let backend_for_logs = backend_name.clone();

    view! {
        <div class="model-row card">
            <span class="model-row__name">{display_name}</span>
            <span class="model-row__meta">{quant_display}</span>
            <span class="model-row__meta">{context_display}</span>
            <span class="model-row__backend text-mono">{backend_name}</span>
            <div class="model-row__actions">
                <span class={badge_class}>{badge_label}</span>
                {if matches!(state.as_str(), "ready") {
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=move || unload_pending.get()
                            on:click=move |_| { on_unload.run(id_for_unload.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else if matches!(state.as_str(), "loading" | "unloading") {
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=true
                        >
                            {button_label}
                        </button>
                    }.into_any()
                } else {
                    // idle, failed → Load or Retry
                    view! {
                        <button
                            class={button_class}
                            prop:disabled=move || load_pending.get()
                            on:click=move |_| { on_load.run(id_for_load.clone()); }
                        >
                            {button_label}
                        </button>
                    }.into_any()
                }}
                <A
                    href=format!("/logs?source={}", backend_for_logs)
                    attr:class="btn btn-secondary btn-sm"
                    attr:title="View backend logs"
                >
                    "Logs"
                </A>
                <A
                    href=format!("/models/{}/edit", db_id.map(|n| n.to_string()).unwrap_or_else(|| id_for_edit.clone()))
                    attr:class="btn btn-secondary btn-sm"
                >
                    "Edit"
                </A>
            </div>
        </div>
    }
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let history = RwSignal::new(Vec::<MetricSample>::new());
    let fetch_failed = RwSignal::new(false);
    // Incrementing this signal re-runs the Effect that opens the EventSource.
    let connect_trigger = RwSignal::new(0u32);
    // Tracks the last backfill timestamp to enforce a cooldown (prevents redundant fetches
    // when SSE lagged, visibilitychange, and reconnect fire together).
    let last_backfill = RwSignal::new(0u64);
    // Set to true when the SSE connection errors, cleared on the next sample event.
    // Used to detect reconnection after a disconnect and trigger backfill.
    let reconnect_pending = RwSignal::new(false);

    // Fetch historical metrics on mount, before connecting to SSE.
    // This populates the chart with up to 450 recent data points (15 minutes at 2s intervals).
    {
        let history_signal = history;
        let last_backfill_signal = last_backfill;
        spawn_local(async move {
            if let Ok(resp) =
                gloo_net::http::Request::get("/tama/v1/system/metrics/history?limit=450")
                    .send()
                    .await
            {
                extract_and_store_csrf_token(&resp);
                if let Ok(entries) = resp.json::<Vec<MetricsHistoryEntry>>().await {
                    let samples: Vec<MetricSample> = entries.into_iter().map(Into::into).collect();
                    if !samples.is_empty() {
                        history_signal.update(|buf| {
                            *buf = samples;
                        });
                        // Prevent immediate redundant backfill on the first SSE lagged/visibility event
                        last_backfill_signal.set(Date::now() as u64);
                    }
                }
            }
        });
    }

    // Open (or re-open) an EventSource each time connect_trigger changes.
    Effect::new(move |_| {
        let _ = connect_trigger.get(); // track signal

        let es = match web_sys::EventSource::new("/tama/v1/system/metrics/stream") {
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
                            if buf.len() > 450 {
                                buf.drain(..buf.len() - 450);
                            }
                        });
                        // If the SSE connection was lost and reconnected, backfill the gap
                        if reconnect_pending.get() {
                            reconnect_pending.set(false);
                            info!("SSE reconnected, backfilling metrics");
                            let history_copy = history;
                            let last_backfill_copy = last_backfill;
                            spawn_local(backfill_metrics(history_copy, last_backfill_copy));
                        }
                    }
                }
            });
        let _ = es.add_event_listener_with_callback("sample", on_sample.as_ref().unchecked_ref());
        on_sample.forget();

        // Handler for "lagged" events — the broadcast channel dropped messages.
        // Fetch recent history to fill the gap.
        let on_lagged =
            Closure::<dyn Fn(web_sys::MessageEvent)>::new(move |evt: web_sys::MessageEvent| {
                if let Some(data_str) = evt.data().as_string() {
                    info!("SSE lagged event received: {}", data_str);
                    let history_copy = history;
                    let last_backfill_copy = last_backfill;
                    spawn_local(backfill_metrics(history_copy, last_backfill_copy));
                }
            });
        let _ = es.add_event_listener_with_callback("lagged", on_lagged.as_ref().unchecked_ref());
        on_lagged.forget();

        // Error handler — flag for the empty-history retry UI and track disconnect for backfill.
        let on_error = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            fetch_failed.set(true);
            reconnect_pending.set(true);
        });
        es.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        // Close the EventSource when the effect re-runs or the component unmounts.
        on_cleanup(move || {
            es.close();
        });
    });

    // When the browser tab becomes visible again, backfill metrics that were missed
    // while the SSE connection was throttled or disconnected by the browser.
    Effect::new(move |_| {
        let history_sig = history;
        let last_backfill_sig = last_backfill;
        let on_visibility = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
            // Use Reflect to check document.hidden (avoids extra web-sys feature flags).
            // When hidden is false (or missing), the tab is visible.
            let is_hidden = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|doc| Reflect::get(&doc, &"hidden".into()).ok())
                .and_then(|v| v.as_bool());
            if is_hidden == Some(false) {
                spawn_local(backfill_metrics(history_sig, last_backfill_sig));
            }
        });
        // Clone the JS function reference (cheap, not the Closure itself) for both add and remove.
        // Closure does not implement Clone, so we extract the underlying Function.
        let js_fn: Function = on_visibility.as_ref().unchecked_ref::<Function>().clone();
        let doc = web_sys::window()
            .expect("window")
            .document()
            .expect("document");
        let _ = doc.add_event_listener_with_callback("visibilitychange", &js_fn);

        // on_cleanup owns on_visibility — it stays alive until cleanup runs,
        // then the Closure is dropped and WASM memory is freed.
        on_cleanup(move || {
            let _ = doc.remove_event_listener_with_callback("visibilitychange", &js_fn);
        });
    });

    // Manual retry: close and re-open the EventSource.
    let manual_refresh = move |_| {
        fetch_failed.set(false);
        reconnect_pending.set(false);
        connect_trigger.update(|n| *n += 1);
    };

    let restart: Action<(), (), LocalStorage> = Action::new_unsync(|_: &()| async move {
        let _ = post_request("/tama/v1/system/restart").send().await;
    });

    // Per-model load/unload actions wired to the same REST endpoints used by
    // the `/models` page. Both actions are unsync because `gloo_net::Request`
    // returns `!Send` futures in the WASM target.
    //
    // We use a manual "busy" signal instead of relying on Action::pending()
    // because in some WASM error scenarios (e.g. proxy returns 500 with no
    // backend configured), the pending flag can get stuck and never reset,
    // leaving buttons permanently disabled with "Loading…" text.
    let load_busy = RwSignal::new(false);
    let unload_busy = RwSignal::new(false);

    // Pull Model modal
    let pull_modal_open = RwSignal::new(false);

    // Check all for updates
    let check_all_busy = RwSignal::new(false);
    let check_all_status = RwSignal::new(Option::<(bool, String)>::None);

    let load_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            load_busy.set(true);
            // Ignore errors — the SSE stream will push updated model state.
            // Even if the request fails (e.g. no backend configured), we set
            // load_busy to false below so the button becomes clickable again.
            let _ = post_request(&format!("/tama/v1/models/{}/load", id))
                .send()
                .await;
            load_busy.set(false);
        }
    });
    let unload_action: Action<String, (), LocalStorage> = Action::new_unsync(move |id: &String| {
        let id = id.clone();
        async move {
            unload_busy.set(true);
            // Same as load — ignore errors, SSE will push the updated state.
            let _ = post_request(&format!("/tama/v1/models/{}/unload", id))
                .send()
                .await;
            unload_busy.set(false);
        }
    });

    let check_all_action: Action<(), (), LocalStorage> =
        Action::new_unsync(move |_: &()| async move {
            check_all_busy.set(true);
            check_all_status.set(None);

            // Fetch the list of models
            let resp = match gloo_net::http::Request::get("/tama/v1/models").send().await {
                Ok(r) => r,
                Err(e) => {
                    check_all_status.set(Some((false, format!("Failed to list models: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };

            // Store CSRF token from response for subsequent POST requests
            extract_and_store_csrf_token(&resp);

            // Surface non-2xx HTTP responses
            if !resp.ok() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                check_all_status.set(Some((
                    false,
                    format!("Failed to list models: HTTP {} {}", status, body),
                )));
                check_all_busy.set(false);
                return;
            }

            // Parse using typed struct (NOT serde_json::Value::as_str() — that returns None for JSON numbers)
            let list: CheckAllModelsApiResponse = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    check_all_status
                        .set(Some((false, format!("Failed to parse models list: {}", e))));
                    check_all_busy.set(false);
                    return;
                }
            };

            let ids: Vec<i64> = list.models.iter().map(|m| m.id).collect();
            let total = ids.len();

            // Safety valve: abort if there are too many models to prevent
            // the UI from being blocked indefinitely. Sequential refresh of
            // 100+ models would take minutes with no timeout mechanism in WASM.
            if total > 100 {
                check_all_status.set(Some((
                    false,
                    format!(
                        "Check all skipped: {} models exceeds the 100-model limit.\n\
                         Consider refreshing models in smaller batches.",
                        total
                    ),
                )));
                check_all_busy.set(false);
                return;
            }

            let mut ok_count = 0usize;
            let mut failed = Vec::<String>::new();
            for (index, id) in ids.into_iter().enumerate() {
                // Update progress for better UX during long operations
                if total > 5 && index % 5 == 0 {
                    check_all_status.set(Some((
                        false,
                        format!("Refreshing models... {}/{}", index.saturating_add(1), total),
                    )));
                }
                let url = format!("/tama/v1/models/{}/refresh", id);
                match post_request(&url).send().await {
                    Ok(r) if r.status() == 200 => ok_count += 1,
                    Ok(r) => {
                        let text = r.text().await.unwrap_or_default();
                        failed.push(format!("{}: {}", id, text));
                    }
                    Err(e) => failed.push(format!("{}: {}", id, e)),
                }
            }

            if failed.is_empty() {
                check_all_status.set(Some((
                    true,
                    format!("Refreshed {}/{} models successfully.", ok_count, total),
                )));
            } else {
                check_all_status.set(Some((
                    false,
                    format!(
                        "Refreshed {}/{} models. Failures: {}",
                        ok_count,
                        total,
                        failed.join("; ")
                    ),
                )));
            }
            check_all_busy.set(false);
            // Reconnect EventSource to pick up fresh model data from SSE stream
            connect_trigger.update(|n| *n += 1);
        });

    view! {
        <div class="page-header">
            <h1>"Dashboard"</h1>
            <div class="page-header-actions">
                // Existing status badge + Restart (inside conditional, only shown after SSE data arrives)
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
                // New buttons (always visible, outside conditional)
                <button class="btn btn-secondary" on:click=move |_| pull_modal_open.set(true)>"Pull Model"</button>
                <button
                    class="btn btn-secondary"
                    prop:disabled=move || check_all_busy.get()
                    on:click=move |_| { check_all_action.dispatch(()); }
                    title="Check HuggingFace for updated metadata on every model"
                >
                    {move || if check_all_busy.get() { "Checking..." } else { "Check all for updates" }}
                </button>
            </div>
        </div>

        // Alert banner — always visible, outside reactive closure
        {move || check_all_status.get().map(|(ok, msg)| {
            let cls = if ok { "alert alert--success" } else { "alert alert--error" };
            view! {
                <div class=cls>
                    <span>{msg}</span>
                    <button
                        class="btn btn-sm btn-link alert__dismiss"
                        on:click=move |_| { check_all_status.set(None); }
                        attr:aria-label="Dismiss alert"
                    >
                        "×"
                    </button>
                </div>
            }
        })}

        {move || {
            let buf = history.get();
            if fetch_failed.get() && buf.is_empty() {
                // Network error, no data yet — show error with retry button
                return view! {
                    <div class="card">
                        <p class="text-error">"Failed to load metrics stream. Is Tama running?"</p>
                        <button class="btn btn-secondary btn-sm mt-2" on:click=manual_refresh>"Retry"</button>
                    </div>
                }.into_any();
            }

            // Extract data for sparkline charts
            let cpu_data: Vec<f32> = buf.iter().map(|s| s.cpu_usage_pct).collect();
            let mem_data: Vec<f32> = buf.iter().map(|s| s.ram_used_mib as f32).collect();
            let timestamps: Vec<i64> = buf.iter().map(|s| s.ts_unix_ms).collect();
            let mem_max = buf.last().map(|h| h.ram_total_mib as f32).unwrap_or(1.0);
            let cpu_y_refs = vec![0.0, 100.0];
            let mem_y_refs = vec![mem_max];

            let gpu_data: Vec<f32> = buf.iter().map(|s| s.gpu_utilization_pct.unwrap_or(0) as f32).collect();
            let vram_data: Vec<f32> = buf.iter().map(|s| s.vram.as_ref().map(|v| v.used_mib as f32).unwrap_or(0.0)).collect();
            let vram_max = buf.last().and_then(|h| h.vram.as_ref().map(|v| v.total_mib as f32)).unwrap_or(1.0);
            let vram_y_refs = vec![vram_max];

            let all_models: Vec<ModelStatus> = buf.last().map(|h| h.models.clone()).unwrap_or_default();
            let active = active_models(&all_models);
            let inactive = inactive_models(&all_models);

            view! {
                <div class="grid-stats">
                    // CPU card
                    <div class="stat-card">
                        <div class="card-header">"CPU Usage"</div>
                        {match buf.last() {
                            Some(h) => view! {
                                <div class="card-value">{format!("{:.1}%", h.cpu_usage_pct)}</div>
                                <div class="card-secondary">"of 100%"</div>
                            }.into_any(),
                            None => view! {
                                <div class="card-value-empty">"—"</div>
                            }.into_any(),
                        }}
                        <div class="sparkline-container">
                            <SparklineChart
                                data=cpu_data
                                max_value=100.0
                                color="var(--accent-green)".to_string()
                                height=60.0
                                timestamps=timestamps.clone()
                                unit_label="%".to_string()
                                y_refs=cpu_y_refs
                            />
                        </div>
                    </div>

                    // Memory card
                    <div class="stat-card">
                        <div class="card-header">"Memory"</div>
                        {match buf.last() {
                            Some(h) => view! {
                                <div class="card-value">{format_number(h.ram_used_mib)}</div>
                                <div class="card-secondary">{format!("of {} MiB", format_number(h.ram_total_mib))}</div>
                            }.into_any(),
                            None => view! {
                                <div class="card-value-empty">"—"</div>
                            }.into_any(),
                        }}
                        <div class="sparkline-container">
                            <SparklineChart
                                data=mem_data
                                max_value=mem_max
                                color="var(--accent-blue)".to_string()
                                height=60.0
                                timestamps=timestamps.clone()
                                unit_label="MiB".to_string()
                                y_refs=mem_y_refs
                            />
                        </div>
                    </div>

                    // GPU card — only rendered if GPU data is present
                    {if let Some(gpu_pct) = buf.last().and_then(|h| h.gpu_utilization_pct) {
                        view! {
                            <div class="stat-card">
                                <div class="card-header">"GPU"</div>
                                <div class="card-value">{format!("{}%", gpu_pct)}</div>
                                <div class="card-secondary">"of 100%"</div>
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=gpu_data
                                        max_value=100.0
                                        color="var(--accent-yellow)".to_string()
                                        height=60.0
                                        timestamps=timestamps.clone()
                                        unit_label="%".to_string()
                                        y_refs=vec![0.0_f32, 100.0_f32]
                                    />
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}

                    // VRAM card — only rendered if VRAM data is present
                    {if let Some(vram_info) = buf.last().and_then(|h| h.vram.as_ref()) {
                        view! {
                            <div class="stat-card">
                                <div class="card-header">"VRAM"</div>
                                <div class="card-value">{format_number(vram_info.used_mib)}</div>
                                <div class="card-secondary">{format!("of {} MiB", format_number(vram_info.total_mib))}</div>
                                <div class="sparkline-container">
                                    <SparklineChart
                                        data=vram_data
                                        max_value=vram_max
                                        color="var(--accent-purple)".to_string()
                                        height=60.0
                                        timestamps=timestamps
                                        unit_label="MiB".to_string()
                                        y_refs=vram_y_refs
                                    />
                                </div>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }}
                </div>

                // Active Models section
                <section class="dashboard-models">
                    <div class="page-header">
                        <h2>"Active Models"</h2>
                        <span class="text-muted">
                            {format!("{} loaded", active.len())}
                        </span>
                    </div>
                    {
                        if all_models.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models configured yet."</p>
                                </div>
                            }.into_any()
                        } else if active.is_empty() {
                            view! {
                                <div class="card card--centered">
                                    <p class="text-muted">"No models currently loaded."</p>
                                </div>
                            }.into_any()
                        } else {
                            let sorted = normalize_models(&active);
                            view! {
                                <div class="models-list">
                                    {sorted.into_iter().map(|m| {
                                        let on_load_cb = Callback::new(move |id: String| {
                                            load_action.dispatch(id);
                                        });
                                        let on_unload_cb = Callback::new(move |id: String| {
                                            unload_action.dispatch(id);
                                        });
                                        view! {
                                            <ModelRow
                                                id=m.id
                                                db_id=m.db_id
                                                display_name=m.display_name
                                                quant_display=m.quant_display
                                                context_display=m.context_display
                                                backend_name=m.backend_name
                                                state=m.state
                                                load_pending=load_busy
                                                unload_pending=unload_busy
                                                on_load=on_load_cb
                                                on_unload=on_unload_cb
                                            />
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }
                </section>

                // Inactive Models section — only render when all_models is non-empty
                {if all_models.is_empty() {
                    view! { <div></div> }.into_any()
                } else {
                    view! {
                        <section class="dashboard-models">
                            <div class="page-header">
                                <h2>"Inactive Models"</h2>
                                <span class="text-muted">
                                    {format!("{} inactive", inactive.len())}
                                </span>
                            </div>
                            {
                                if inactive.is_empty() {
                                    view! {
                                        <div class="card card--centered">
                                            <p class="text-muted">"No inactive models."</p>
                                        </div>
                                    }.into_any()
                                } else {
                                    let sorted = normalize_models(&inactive);
                                    view! {
                                        <div class="models-list">
                                            {sorted.into_iter().map(|m| {
                                                let on_load_cb = Callback::new(move |id: String| {
                                                    load_action.dispatch(id);
                                                });
                                                let on_unload_cb = Callback::new(move |id: String| {
                                                    unload_action.dispatch(id);
                                                });
                                                view! {
                                                    <ModelRow
                                                        id=m.id
                                                        db_id=m.db_id
                                                        display_name=m.display_name
                                                        quant_display=m.quant_display
                                                        context_display=m.context_display
                                                        backend_name=m.backend_name
                                                        state=m.state
                                                        load_pending=load_busy
                                                        unload_pending=unload_busy
                                                        on_load=on_load_cb
                                                        on_unload=on_unload_cb
                                                    />
                                                }
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_any()
                                }
                            }
                        </section>
                    }.into_any()
                }}
            }.into_any()
        }}

        <Modal
            open=rw_signal_to_signal(pull_modal_open)
            on_close=Callback::new(move |_| pull_modal_open.set(false))
            title="Pull Model".to_string()
        >
            <PullQuantWizard
                initial_repo=Signal::derive(String::new)
                is_open=rw_signal_to_signal(pull_modal_open)
                on_complete=Callback::new(move |_completed: Vec<CompletedQuant>| {
                    pull_modal_open.set(false);
                    connect_trigger.update(|n| *n += 1);
                })
                on_close=Callback::new(move |_| pull_modal_open.set(false))
            />
        </Modal>
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

    /// `MetricsHistoryEntry` must correctly convert to `MetricSample`,
    /// mapping i64 fields to their corresponding types.
    #[test]
    fn metrics_history_entry_converts_to_metric_sample() {
        let entry = MetricsHistoryEntry {
            ts_unix_ms: 1_700_000_000_000,
            cpu_usage_pct: 45.5,
            ram_used_mib: 8192,
            ram_total_mib: 32768,
            gpu_utilization_pct: Some(85),
            vram_used_mib: Some(4096),
            vram_total_mib: Some(8192),
        };

        let sample: MetricSample = entry.into();

        assert_eq!(sample.ts_unix_ms, 1_700_000_000_000);
        assert!((sample.cpu_usage_pct - 45.5).abs() < f32::EPSILON);
        assert_eq!(sample.ram_used_mib, 8192);
        assert_eq!(sample.ram_total_mib, 32768);
        assert_eq!(sample.gpu_utilization_pct, Some(85));
        assert!(sample.vram.is_some());
        let vram = sample.vram.unwrap();
        assert_eq!(vram.used_mib, 4096);
        assert_eq!(vram.total_mib, 8192);
        assert_eq!(sample.models_loaded, 0);
        assert!(sample.models.is_empty());
    }

    /// `MetricsHistoryEntry` with null GPU/VRAM fields must produce a
    /// `MetricSample` with `None` for both.
    #[test]
    fn metrics_history_entry_converts_with_null_gpu() {
        let entry = MetricsHistoryEntry {
            ts_unix_ms: 1_700_000_000_000,
            cpu_usage_pct: 10.0,
            ram_used_mib: 2048,
            ram_total_mib: 16384,
            gpu_utilization_pct: None,
            vram_used_mib: None,
            vram_total_mib: None,
        };

        let sample: MetricSample = entry.into();

        assert!(sample.gpu_utilization_pct.is_none());
        assert!(sample.vram.is_none());
    }

    /// `MetricsHistoryEntry` with `vram_used_mib` present but
    /// `vram_total_mib` absent must produce `None` for `vram` (not a
    /// partial `VramInfo`).
    #[test]
    fn metrics_history_entry_partial_vram_produces_none() {
        let entry = MetricsHistoryEntry {
            ts_unix_ms: 1_700_000_000_000,
            cpu_usage_pct: 10.0,
            ram_used_mib: 2048,
            ram_total_mib: 16384,
            gpu_utilization_pct: Some(50),
            vram_used_mib: Some(4096),
            vram_total_mib: None,
        };

        let sample: MetricSample = entry.into();

        // vram should be None because total_mib is None
        assert!(sample.vram.is_none());
    }

    /// The `format_number` helper must produce comma-separated thousands.
    #[test]
    fn format_number_adds_commas() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(12345), "12,345");
        assert_eq!(format_number(123456), "123,456");
        assert_eq!(format_number(1234567), "1,234,567");
        assert_eq!(format_number(16384), "16,384");
        assert_eq!(format_number(65183), "65,183");
    }

    /// `active_models` returns entries whose state is "ready", "loading", or
    /// "unloading", preserving order and including all fields.
    #[test]
    fn active_models_returns_ready_loading_unloading_entries() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "c".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "ik_llama".into(),
                state: "loading".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "d".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "ik_llama".into(),
                state: "failed".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "e".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                state: "unloading".into(),
                ..Default::default()
            },
        ];

        let active = active_models(&models);
        assert_eq!(active.len(), 3);
        assert_eq!(active[0].id, "a");
        assert_eq!(active[0].state, "ready");
        assert_eq!(active[1].id, "c");
        assert_eq!(active[1].state, "loading");
        assert_eq!(active[2].id, "e");
        assert_eq!(active[2].state, "unloading");
    }

    /// Loaded (ready) models must use the success badge class so they visually pop
    /// against idle entries in the Active Models grid.
    #[test]
    fn model_status_badge_class_uses_success_when_ready() {
        assert_eq!(model_status_badge_class("ready"), "badge badge-success");
    }

    /// Idle models must use the muted badge class so they recede compared to
    /// loaded entries — matching the convention used elsewhere on the
    /// `/models` page.
    #[test]
    fn model_status_badge_class_uses_muted_when_idle() {
        assert_eq!(model_status_badge_class("idle"), "badge badge-muted");
    }

    /// Badge text mirrors the badge colour: "Loaded" for ready models,
    /// "Idle" for everything else. Tests both branches so a future renaming
    /// can't silently drift one of them.
    #[test]
    fn model_status_badge_label_distinguishes_ready_and_idle() {
        assert_eq!(model_status_badge_label("ready"), "Loaded");
        assert_eq!(model_status_badge_label("idle"), "Idle");
    }

    /// Ready models surface an Unload action — destructive styling so the
    /// user understands clicking it tears down a running server.
    #[test]
    fn model_action_button_class_uses_danger_when_ready() {
        assert_eq!(model_action_button_class("ready"), "btn btn-danger btn-sm");
    }

    /// Idle models surface a Load action — affirmative styling so the user
    /// understands clicking it spins up a server.
    #[test]
    fn model_action_button_class_uses_success_when_idle() {
        assert_eq!(model_action_button_class("idle"), "btn btn-success btn-sm");
    }

    /// Loading models get a disabled secondary button.
    #[test]
    fn model_action_button_class_uses_secondary_when_loading() {
        assert_eq!(
            model_action_button_class("loading"),
            "btn btn-secondary btn-sm"
        );
    }

    /// Unloading models get a disabled secondary button.
    #[test]
    fn model_action_button_class_uses_secondary_when_unloading() {
        assert_eq!(
            model_action_button_class("unloading"),
            "btn btn-secondary btn-sm"
        );
    }

    /// Failed models get a warning button (Retry).
    #[test]
    fn model_action_button_class_uses_warning_when_failed() {
        assert_eq!(
            model_action_button_class("failed"),
            "btn btn-warning btn-sm"
        );
    }

    /// Action button labels must match their visual styling.
    #[test]
    fn model_action_button_label_distinguishes_states() {
        assert_eq!(model_action_button_label("ready"), "Unload");
        assert_eq!(model_action_button_label("idle"), "Load");
        assert_eq!(model_action_button_label("loading"), "Loading…");
        assert_eq!(model_action_button_label("unloading"), "Unloading…");
        assert_eq!(model_action_button_label("failed"), "Retry");
    }

    /// Status badge class and label helpers map states correctly.
    #[test]
    fn model_status_badge_class_and_label_map_all_states() {
        assert_eq!(model_status_badge_class("ready"), "badge badge-success");
        assert_eq!(model_status_badge_label("ready"), "Loaded");

        assert_eq!(model_status_badge_class("loading"), "badge badge-info");
        assert_eq!(model_status_badge_label("loading"), "Loading");

        assert_eq!(model_status_badge_class("unloading"), "badge badge-warning");
        assert_eq!(model_status_badge_label("unloading"), "Unloading");

        assert_eq!(model_status_badge_class("failed"), "badge badge-error");
        assert_eq!(model_status_badge_label("failed"), "Failed");

        assert_eq!(model_status_badge_class("idle"), "badge badge-muted");
        assert_eq!(model_status_badge_label("idle"), "Idle");
    }

    /// `active_models` includes ready, loading, and unloading models.
    #[test]
    fn active_models_filters_to_active_states() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "c".into(),
                state: "loading".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "d".into(),
                state: "unloading".into(),
                ..Default::default()
            },
        ];

        let active = active_models(&models);
        assert_eq!(active.len(), 3);
        assert_eq!(active[0].id, "a");
        assert_eq!(active[1].id, "c");
        assert_eq!(active[2].id, "d");
    }

    /// `active_models` returns an empty vec when all models are idle or failed.
    #[test]
    fn active_models_returns_empty_when_none_active() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "failed".into(),
                ..Default::default()
            },
        ];

        let active = active_models(&models);
        assert!(active.is_empty());
    }

    /// `active_models` returns a clone of all models when all are active.
    #[test]
    fn active_models_returns_all_when_all_active() {
        let models = vec![
            ModelStatus {
                id: "x".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "y".into(),
                state: "loading".into(),
                ..Default::default()
            },
        ];

        let active = active_models(&models);
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].id, "x");
        assert_eq!(active[1].id, "y");
    }

    /// `active_models` returns an empty vec for an empty input slice.
    #[test]
    fn active_models_returns_empty_for_empty_input() {
        let models: Vec<ModelStatus> = vec![];
        let active = active_models(&models);
        assert!(active.is_empty());
    }

    /// `inactive_models` returns entries whose state is NOT "ready", "loading",
    /// or "unloading" — i.e. idle, failed, and any unknown states.
    #[test]
    fn inactive_models_returns_idle_failed_and_unknown_entries() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "c".into(),
                state: "loading".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "d".into(),
                state: "failed".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "e".into(),
                state: "unloading".into(),
                ..Default::default()
            },
        ];

        let inactive = inactive_models(&models);
        assert_eq!(inactive.len(), 2);
        assert_eq!(inactive[0].id, "b");
        assert_eq!(inactive[0].state, "idle");
        assert_eq!(inactive[1].id, "d");
        assert_eq!(inactive[1].state, "failed");
    }

    /// `inactive_models` returns an empty vec when all models are active
    /// (ready, loading, or unloading).
    #[test]
    fn inactive_models_returns_empty_when_all_active() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "loading".into(),
                ..Default::default()
            },
        ];

        let inactive = inactive_models(&models);
        assert!(inactive.is_empty());
    }

    /// `inactive_models` returns all models when none are active.
    #[test]
    fn inactive_models_returns_all_when_none_active() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "failed".into(),
                ..Default::default()
            },
        ];

        let inactive = inactive_models(&models);
        assert_eq!(inactive.len(), 2);
        assert_eq!(inactive[0].id, "a");
        assert_eq!(inactive[1].id, "b");
    }

    /// `inactive_models` returns an empty vec for an empty input slice.
    #[test]
    fn inactive_models_returns_empty_for_empty_input() {
        let models: Vec<ModelStatus> = vec![];
        let inactive = inactive_models(&models);
        assert!(inactive.is_empty());
    }

    /// `inactive_models` preserves all model fields (display_name, quant,
    /// context_length, db_id, backend) so the Inactive Models section can
    /// render them without any data loss.
    #[test]
    fn inactive_models_preserves_all_fields() {
        let models = vec![
            ModelStatus {
                id: "llama3-8b".into(),
                db_id: Some(1),
                api_name: Some("meta-llama/Llama-3-8B".into()),
                display_name: Some("Llama 3 8B".into()),
                backend: "llama_cpp".into(),
                state: "ready".into(),
                quant: Some("Q4_K_M".into()),
                context_length: Some(8192),
                ..Default::default()
            },
            ModelStatus {
                id: "mistral-7b".into(),
                db_id: Some(2),
                api_name: Some("mistralai/Mistral-7B".into()),
                display_name: Some("Mistral 7B".into()),
                backend: "llama_cpp".into(),
                state: "idle".into(),
                quant: Some("Q4_0".into()),
                context_length: Some(32768),
                ..Default::default()
            },
            ModelStatus {
                id: "gemma-2b".into(),
                db_id: Some(3),
                api_name: Some("google/gemma-2b".into()),
                display_name: Some("Gemma 2B".into()),
                backend: "llama_cpp".into(),
                state: "failed".into(),
                quant: Some("Q5_K_M".into()),
                context_length: Some(4096),
                ..Default::default()
            },
        ];

        let inactive = inactive_models(&models);
        assert_eq!(inactive.len(), 2);

        // Verify idle model fields are preserved
        let idle_model = &inactive
            .iter()
            .find(|m| m.state == "idle")
            .expect("idle model missing");
        assert_eq!(idle_model.id, "mistral-7b");
        assert_eq!(idle_model.db_id, Some(2));
        assert_eq!(idle_model.display_name, Some("Mistral 7B".into()));
        assert_eq!(idle_model.quant, Some("Q4_0".into()));
        assert_eq!(idle_model.context_length, Some(32768));
        assert_eq!(idle_model.backend, "llama_cpp");

        // Verify failed model fields are preserved
        let failed_model = &inactive
            .iter()
            .find(|m| m.state == "failed")
            .expect("failed model missing");
        assert_eq!(failed_model.id, "gemma-2b");
        assert_eq!(failed_model.db_id, Some(3));
        assert_eq!(failed_model.display_name, Some("Gemma 2B".into()));
        assert_eq!(failed_model.quant, Some("Q5_K_M".into()));
        assert_eq!(failed_model.context_length, Some(4096));
        assert_eq!(failed_model.backend, "llama_cpp");
    }

    /// `active_models` and `inactive_models` are symmetric complements:
    /// together they must contain exactly all input models, with no overlap.
    #[test]
    fn active_and_inactive_models_are_symmetric_complements() {
        let models = vec![
            ModelStatus {
                id: "a".into(),
                state: "ready".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "b".into(),
                state: "idle".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "c".into(),
                state: "loading".into(),
                ..Default::default()
            },
            ModelStatus {
                id: "d".into(),
                state: "failed".into(),
                ..Default::default()
            },
        ];

        let active = active_models(&models);
        let inactive = inactive_models(&models);

        assert_eq!(active.len() + inactive.len(), models.len());

        // No overlap: no model id appears in both lists.
        let active_ids: Vec<&str> = active.iter().map(|m| m.id.as_str()).collect();
        for inactive_model in &inactive {
            assert!(
                !active_ids.contains(&inactive_model.id.as_str()),
                "model '{}' should not be in both active and inactive",
                inactive_model.id
            );
        }
    }

    /// When the backend includes a populated `models` array, every `ModelStatus`
    /// must round-trip with its `id`, `backend`, and `state` fields preserved.
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
                { "id": "alpha", "api_name": "org/alpha", "backend": "llama_cpp", "loaded": true, "state": "ready" },
                { "id": "beta",  "api_name": "org/beta",  "backend": "ik_llama",  "loaded": false, "state": "idle" }
            ]
        }"#;

        let sample: MetricSample =
            serde_json::from_str(json).expect("MetricSample with `models` must deserialize");

        assert_eq!(sample.models.len(), 2);

        assert_eq!(sample.models[0].id, "alpha");
        assert_eq!(sample.models[0].api_name, Some("org/alpha".to_string()));
        assert_eq!(sample.models[0].backend, "llama_cpp");
        assert_eq!(sample.models[0].state, "ready");

        assert_eq!(sample.models[1].id, "beta");
        assert_eq!(sample.models[1].api_name, Some("org/beta".to_string()));
        assert_eq!(sample.models[1].backend, "ik_llama");
        assert_eq!(sample.models[1].state, "idle");
    }

    /// Two non-overlapping buffers merge, sort by timestamp, and preserve order.
    #[test]
    fn test_merge_samples_combines_two_buffers() {
        let mut buf = vec![
            MetricSample {
                ts_unix_ms: 100,
                cpu_usage_pct: 10.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 200,
                cpu_usage_pct: 20.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];
        let new = vec![
            MetricSample {
                ts_unix_ms: 50,
                cpu_usage_pct: 5.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 300,
                cpu_usage_pct: 30.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];

        merge_samples(&mut buf, new, 100);

        assert_eq!(buf.len(), 4);
        assert_eq!(buf[0].ts_unix_ms, 50);
        assert_eq!(buf[1].ts_unix_ms, 100);
        assert_eq!(buf[2].ts_unix_ms, 200);
        assert_eq!(buf[3].ts_unix_ms, 300);
    }

    /// Overlapping timestamps keep the first entry (SSE entry with models data).
    #[test]
    fn test_merge_samples_dedupes_by_timestamp_keeps_first() {
        let sse_entry = MetricSample {
            ts_unix_ms: 100,
            cpu_usage_pct: 50.0,
            ram_used_mib: 1024,
            ram_total_mib: 16384,
            gpu_utilization_pct: None,
            vram: None,
            models_loaded: 1,
            models: vec![ModelStatus {
                id: "alpha".into(),
                db_id: None,
                api_name: None,
                display_name: None,
                backend: "llama_cpp".into(),
                state: "ready".into(),
                ..Default::default()
            }],
        };
        let backfill_entry = MetricSample {
            ts_unix_ms: 100,
            cpu_usage_pct: 50.0,
            ram_used_mib: 1024,
            ram_total_mib: 16384,
            gpu_utilization_pct: None,
            vram: None,
            models_loaded: 0,
            models: vec![],
        };

        let mut buf = vec![sse_entry];
        merge_samples(&mut buf, vec![backfill_entry], 100);

        // Should keep the SSE entry (first) with models data
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].ts_unix_ms, 100);
        assert_eq!(buf[0].models_loaded, 1);
        assert_eq!(buf[0].models.len(), 1);
        assert_eq!(buf[0].models[0].id, "alpha");
    }

    /// Buffer exceeding max_len is trimmed from the front (oldest entries removed).
    #[test]
    fn test_merge_samples_trims_to_max_len() {
        let mut buf = vec![
            MetricSample {
                ts_unix_ms: 1,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 2,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 3,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];
        let new = vec![
            MetricSample {
                ts_unix_ms: 4,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 5,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];

        merge_samples(&mut buf, new, 3);

        assert_eq!(buf.len(), 3);
        // Oldest entries (ts 1, 2) should be trimmed; keep ts 3, 4, 5
        assert_eq!(buf[0].ts_unix_ms, 3);
        assert_eq!(buf[1].ts_unix_ms, 4);
        assert_eq!(buf[2].ts_unix_ms, 5);
    }

    /// Empty new leaves buffer unchanged.
    #[test]
    fn test_merge_samples_empty_new_does_nothing() {
        let mut buf = vec![
            MetricSample {
                ts_unix_ms: 100,
                cpu_usage_pct: 10.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 200,
                cpu_usage_pct: 20.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];

        merge_samples(&mut buf, vec![], 100);

        assert_eq!(buf.len(), 2);
        assert_eq!(buf[0].ts_unix_ms, 100);
        assert_eq!(buf[1].ts_unix_ms, 200);
    }

    /// Empty buffer gets populated from new entries.
    #[test]
    fn test_merge_samples_empty_buf_populates_from_new() {
        let mut buf: Vec<MetricSample> = vec![];
        let new = vec![
            MetricSample {
                ts_unix_ms: 200,
                cpu_usage_pct: 20.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 100,
                cpu_usage_pct: 10.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];

        merge_samples(&mut buf, new, 100);

        assert_eq!(buf.len(), 2);
        assert_eq!(buf[0].ts_unix_ms, 100);
        assert_eq!(buf[1].ts_unix_ms, 200);
    }

    /// When new has the same timestamps as buf but different data values,
    /// the existing (first) entries survive dedup.
    #[test]
    fn test_merge_samples_all_timestamps_overlap_keeps_existing() {
        let mut buf = vec![
            MetricSample {
                ts_unix_ms: 100,
                cpu_usage_pct: 50.0,
                ram_used_mib: 1024,
                ram_total_mib: 16384,
                gpu_utilization_pct: Some(80),
                vram: None,
                models_loaded: 2,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 200,
                cpu_usage_pct: 60.0,
                ram_used_mib: 2048,
                ram_total_mib: 16384,
                gpu_utilization_pct: Some(90),
                vram: None,
                models_loaded: 2,
                models: vec![],
            },
        ];
        let new = vec![
            MetricSample {
                ts_unix_ms: 100,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
            MetricSample {
                ts_unix_ms: 200,
                cpu_usage_pct: 0.0,
                ram_used_mib: 0,
                ram_total_mib: 0,
                gpu_utilization_pct: None,
                vram: None,
                models_loaded: 0,
                models: vec![],
            },
        ];

        merge_samples(&mut buf, new, 100);

        assert_eq!(buf.len(), 2);
        // Original values preserved (first entry wins)
        assert_eq!(buf[0].ts_unix_ms, 100);
        assert_eq!(buf[0].cpu_usage_pct, 50.0);
        assert_eq!(buf[0].models_loaded, 2);
        assert_eq!(buf[1].ts_unix_ms, 200);
        assert_eq!(buf[1].cpu_usage_pct, 60.0);
        assert_eq!(buf[1].models_loaded, 2);
    }
}
