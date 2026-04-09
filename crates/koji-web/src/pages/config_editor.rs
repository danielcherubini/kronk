use leptos::prelude::*;
use leptos::task::spawn_local;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ─── WASM-safe JSON mirror types ──────────────────────────────────────────
// These match the shape served by /api/config/structured and accepted by POST.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
    #[serde(default)]
    pub supervisor: Supervisor,
    #[serde(default)]
    pub sampling_templates: BTreeMap<String, SamplingParams>,
    #[serde(default)]
    pub proxy: ProxyConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct General {
    #[serde(default)]
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    #[serde(default)]
    pub health_check_url: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<SamplingParams>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_layers: Option<u32>,
    /// Forward-compat: preserve any additional fields we don't know about
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Supervisor {
    #[serde(default)]
    pub restart_policy: String,
    #[serde(default)]
    pub max_restarts: u32,
    #[serde(default)]
    pub restart_delay_ms: u64,
    #[serde(default)]
    pub health_check_interval_ms: u64,
    #[serde(default)]
    pub health_check_timeout_ms: u64,
    #[serde(default)]
    pub health_check_retries: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub idle_timeout_secs: u64,
    #[serde(default)]
    pub startup_timeout_secs: u64,
    #[serde(default)]
    pub circuit_breaker_threshold: u32,
    #[serde(default)]
    pub circuit_breaker_cooldown_seconds: u64,
    #[serde(default)]
    pub metrics_retention_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SamplingParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeat_penalty: Option<f64>,
}

// ─── Section tabs ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    General,
    Proxy,
    Supervisor,
    Sampling,
}

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Proxy => "Proxy",
            Section::Supervisor => "Supervisor",
            Section::Sampling => "Sampling Templates",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Section::General => "⚙️",
            Section::Proxy => "🌐",
            Section::Supervisor => "👀",
            Section::Sampling => "🎲",
        }
    }
}

// ─── Main Page ────────────────────────────────────────────────────────────

#[component]
pub fn ConfigEditor() -> impl IntoView {
    let current = RwSignal::new(Section::General);
    let config: RwSignal<Option<Config>> = RwSignal::new(None);
    let loading = RwSignal::new(true);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let save_status: RwSignal<Option<String>> = RwSignal::new(None);

    // Initial fetch
    Effect::new(move |_| {
        spawn_local(async move {
            loading.set(true);
            error.set(None);
            match gloo_net::http::Request::get("/api/config/structured")
                .send()
                .await
            {
                Ok(resp) => match resp.json::<Config>().await {
                    Ok(cfg) => config.set(Some(cfg)),
                    Err(e) => error.set(Some(format!("Failed to parse config: {}", e))),
                },
                Err(e) => error.set(Some(format!("Failed to fetch config: {}", e))),
            }
            loading.set(false);
        });
    });

    let save = move |_| {
        let Some(cfg) = config.get() else {
            return;
        };
        save_status.set(Some("Saving…".to_string()));
        spawn_local(async move {
            let body = match serde_json::to_string(&cfg) {
                Ok(s) => s,
                Err(e) => {
                    save_status.set(Some(format!("Serialize error: {}", e)));
                    return;
                }
            };
            let res = gloo_net::http::Request::post("/api/config/structured")
                .header("Content-Type", "application/json")
                .body(body)
                .expect("failed to build request")
                .send()
                .await;
            match res {
                Ok(resp) if resp.ok() => {
                    save_status.set(Some("✅ Saved".to_string()));
                }
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    save_status.set(Some(format!("❌ {} — {}", status, text)));
                }
                Err(e) => {
                    save_status.set(Some(format!("❌ {}", e)));
                }
            }
        });
    };

    view! {
        <div class="page-header">
            <h1>"Configuration"</h1>
            <div style="display:flex;gap:0.5rem;align-items:center;">
                {move || save_status.get().map(|s| view! { <span class="text-muted">{s}</span> })}
                <button class="btn btn-primary" on:click=save>"Save Changes"</button>
            </div>
        </div>

        {move || {
            if loading.get() {
                view! { <div class="card card--centered"><span class="spinner">"Loading config..."</span></div> }.into_any()
            } else if let Some(err) = error.get() {
                view! { <div class="card"><p class="text-error">{err}</p></div> }.into_any()
            } else if config.get().is_none() {
                view! { <div class="card"><p>"No config data"</p></div> }.into_any()
            } else {
                view! {
                    <div style="display:flex;gap:1.5rem;align-items:flex-start;">
                        // Side nav
                        <nav class="card" style="width:220px;flex-shrink:0;padding:0.75rem;">
                            <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:0.25rem;">
                                {[Section::General, Section::Proxy, Section::Supervisor, Section::Sampling]
                                    .into_iter().map(|s| {
                                        let active = move || current.get() == s;
                                        view! {
                                            <li>
                                                <button
                                                    class:btn=true
                                                    class:btn-primary=active
                                                    class:btn-secondary=move || !active()
                                                    style="width:100%;text-align:left;display:flex;gap:0.5rem;align-items:center;"
                                                    on:click=move |_| current.set(s)
                                                >
                                                    <span>{s.icon()}</span>
                                                    <span>{s.name()}</span>
                                                </button>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                            </ul>
                        </nav>

                        // Main form area
                        <div style="flex:1;min-width:0;">
                            {move || match current.get() {
                                Section::General => view! { <GeneralForm config=config /> }.into_any(),
                                Section::Proxy => view! { <ProxyForm config=config /> }.into_any(),
                                Section::Supervisor => view! { <SupervisorForm config=config /> }.into_any(),
                                Section::Sampling => view! { <SamplingForm config=config /> }.into_any(),
                            }}
                        </div>
                    </div>
                }.into_any()
            }
        }}
    }
}

// ─── Helper: get event.target.value as String ─────────────────────────────
fn target_value(ev: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    ev.target()
        .and_then(|t| {
            t.dyn_into::<web_sys::HtmlInputElement>()
                .ok()
                .map(|i| i.value())
                .or_else(|| {
                    ev.target().and_then(|t| {
                        t.dyn_into::<web_sys::HtmlSelectElement>()
                            .ok()
                            .map(|s| s.value())
                    })
                })
                .or_else(|| {
                    ev.target().and_then(|t| {
                        t.dyn_into::<web_sys::HtmlTextAreaElement>()
                            .ok()
                            .map(|s| s.value())
                    })
                })
        })
        .unwrap_or_default()
}

// ─── General Form ─────────────────────────────────────────────────────────

#[component]
fn GeneralForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let get_general = move || config.get().map(|c| c.general).unwrap_or_default();

    view! {
        <div class="card">
            <h2>"General Settings"</h2>
            <p class="text-muted">"Global Koji settings."</p>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Log Level"</label>
                    <select
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.general.log_level = v; });
                        }
                        prop:value=move || get_general().log_level
                    >
                        <option value="trace">"trace"</option>
                        <option value="debug">"debug"</option>
                        <option value="info">"info"</option>
                        <option value="warn">"warn"</option>
                        <option value="error">"error"</option>
                    </select>
                </div>

                <div>
                    <label>"Models Directory"</label>
                    <input
                        type="text"
                        placeholder="/path/to/models"
                        prop:value=move || get_general().models_dir.unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.general.models_dir = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                </div>

                <div>
                    <label>"Logs Directory"</label>
                    <input
                        type="text"
                        placeholder="/path/to/logs"
                        prop:value=move || get_general().logs_dir.unwrap_or_default()
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c {
                                c.general.logs_dir = if v.is_empty() { None } else { Some(v) };
                            });
                        }
                    />
                </div>
            </div>
        </div>
    }
}

// ─── Proxy Form ───────────────────────────────────────────────────────────

#[component]
fn ProxyForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let get_proxy = move || config.get().map(|c| c.proxy).unwrap_or_default();

    view! {
        <div class="card">
            <h2>"Proxy Settings"</h2>
            <p class="text-muted">"Configure the proxy server that routes OpenAI/Ollama-compatible requests."</p>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Host"</label>
                    <input
                        type="text"
                        prop:value=move || get_proxy().host
                        on:input=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.proxy.host = v; });
                        }
                    />
                </div>

                <div>
                    <label>"Port"</label>
                    <input
                        type="number"
                        min="1"
                        max="65535"
                        prop:value=move || get_proxy().port.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u16>() {
                                config.update(|c| if let Some(c) = c { c.proxy.port = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Idle Timeout (seconds)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_proxy().idle_timeout_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.proxy.idle_timeout_secs = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Startup Timeout (seconds)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_proxy().startup_timeout_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.proxy.startup_timeout_secs = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Circuit Breaker Threshold"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_proxy().circuit_breaker_threshold.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.proxy.circuit_breaker_threshold = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Circuit Breaker Cooldown (seconds)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_proxy().circuit_breaker_cooldown_seconds.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.proxy.circuit_breaker_cooldown_seconds = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Metrics Retention (seconds)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_proxy().metrics_retention_secs.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.proxy.metrics_retention_secs = v; });
                            }
                        }
                    />
                </div>
            </div>
        </div>
    }
}

// ─── Supervisor Form ──────────────────────────────────────────────────────

#[component]
fn SupervisorForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let get_sup = move || config.get().map(|c| c.supervisor).unwrap_or_default();

    view! {
        <div class="card">
            <h2>"Supervisor"</h2>
            <p class="text-muted">"Process restart and health-check behavior for managed models."</p>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Restart Policy"</label>
                    <select
                        prop:value=move || get_sup().restart_policy
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| if let Some(c) = c { c.supervisor.restart_policy = v; });
                        }
                    >
                        <option value="always">"always"</option>
                        <option value="on-failure">"on-failure"</option>
                        <option value="never">"never"</option>
                    </select>
                </div>

                <div>
                    <label>"Max Restarts"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().max_restarts.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.max_restarts = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Restart Delay (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().restart_delay_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.restart_delay_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Interval (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_interval_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_interval_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Timeout (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_timeout_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_timeout_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Retries"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_retries.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_retries = v; });
                            }
                        }
                    />
                </div>
            </div>
        </div>
    }
}

// ─── Sampling Templates Form ──────────────────────────────────────────────

macro_rules! sampling_float {
    ($config:expr, $key:expr, $label:expr, $field:ident) => {{
        let key = $key.clone();
        let key2 = $key.clone();
        let config = $config;
        view! {
            <div>
                <label>{$label}</label>
                <input
                    type="number"
                    step="0.01"
                    prop:value=move || config.get()
                        .and_then(|c| c.sampling_templates.get(&key).and_then(|t| t.$field))
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                    on:input=move |ev| {
                        let v = target_value(&ev);
                        let k = key2.clone();
                        config.update(|c| if let Some(c) = c {
                            if let Some(t) = c.sampling_templates.get_mut(&k) {
                                t.$field = if v.is_empty() { None } else { v.parse::<f64>().ok() };
                            }
                        });
                    }
                />
            </div>
        }
    }};
}

macro_rules! sampling_u32 {
    ($config:expr, $key:expr, $label:expr, $field:ident) => {{
        let key = $key.clone();
        let key2 = $key.clone();
        let config = $config;
        view! {
            <div>
                <label>{$label}</label>
                <input
                    type="number"
                    min="0"
                    step="1"
                    prop:value=move || config.get()
                        .and_then(|c| c.sampling_templates.get(&key).and_then(|t| t.$field))
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                    on:input=move |ev| {
                        let v = target_value(&ev);
                        let k = key2.clone();
                        config.update(|c| if let Some(c) = c {
                            if let Some(t) = c.sampling_templates.get_mut(&k) {
                                t.$field = if v.is_empty() { None } else { v.parse::<u32>().ok() };
                            }
                        });
                    }
                />
            </div>
        }
    }};
}

#[component]
fn SamplingForm(config: RwSignal<Option<Config>>) -> impl IntoView {
    let template_keys = move || {
        config
            .get()
            .map(|c| c.sampling_templates.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default()
    };

    view! {
        <div class="card">
            <h2>"Sampling Templates"</h2>
            <p class="text-muted">"Reusable named sets of LLM sampling parameters."</p>

            {move || {
                let keys = template_keys();
                if keys.is_empty() {
                    view! { <p class="text-muted">"No sampling templates defined."</p> }.into_any()
                } else {
                    view! {
                        <div style="display:flex;flex-direction:column;gap:1.5rem;margin-top:1rem;">
                            {keys.into_iter().map(|key| {
                                view! {
                                    <fieldset style="border:1px solid var(--border,#ccc);padding:1rem;border-radius:6px;">
                                        <legend style="font-weight:600;">{key.clone()}</legend>
                                        <div style="display:grid;grid-template-columns:1fr 1fr;gap:0.75rem;">
                                            {sampling_float!(config, key, "Temperature", temperature)}
                                            {sampling_u32!(config, key, "Top K", top_k)}
                                            {sampling_float!(config, key, "Top P", top_p)}
                                            {sampling_float!(config, key, "Min P", min_p)}
                                            {sampling_float!(config, key, "Presence Penalty", presence_penalty)}
                                            {sampling_float!(config, key, "Frequency Penalty", frequency_penalty)}
                                            {sampling_float!(config, key, "Repeat Penalty", repeat_penalty)}
                                        </div>
                                    </fieldset>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}
