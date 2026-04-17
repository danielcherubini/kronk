//! Backend card component - displays a single backend with action buttons.

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;

// ── DTOs (mirror of koji-web::api::backends DTOs) ────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatusDto {
    #[serde(default)]
    pub checked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GpuTypeDto {
    Cuda { version: String },
    Vulkan,
    Metal,
    Rocm { version: String },
    CpuOnly,
    Custom,
}

impl GpuTypeDto {
    pub fn label(&self) -> String {
        match self {
            GpuTypeDto::Cuda { version } => format!("CUDA {version}"),
            GpuTypeDto::Vulkan => "Vulkan".to_string(),
            GpuTypeDto::Metal => "Metal".to_string(),
            GpuTypeDto::Rocm { version } => format!("ROCm {version}"),
            GpuTypeDto::CpuOnly => "CPU".to_string(),
            GpuTypeDto::Custom => "Custom".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BackendSourceDto {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendInfoDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<GpuTypeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendVersionDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<GpuTypeDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BackendCardDto {
    pub r#type: String,
    pub display_name: String,
    pub installed: bool,
    /// Info for the currently selected version (default: active version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    /// All installed versions of this backend.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<BackendVersionDto>,
    #[serde(default)]
    pub update: UpdateStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    /// Whether the active version is currently selected for display.
    #[serde(default)]
    pub is_active: bool,
}

// ── Component ────────────────────────────────────────────────────────────────

/// BackendCard - displays one backend with action buttons and a version selector.
#[component]
#[allow(dead_code)]
pub fn BackendCard(
    backend: BackendCardDto,
    /// Called with the backend type when "Install" is clicked.
    #[prop(optional)]
    on_install: Option<Callback<String>>,
    /// Called with the backend type when "Update" is clicked.
    #[prop(optional)]
    on_update: Option<Callback<String>>,
    /// Called with the backend type when "Check for updates" is clicked.
    #[prop(optional)]
    on_check_updates: Option<Callback<String>>,
    /// Called with the backend type when "Uninstall" is clicked.
    #[prop(optional)]
    on_delete: Option<Callback<String>>,
    /// Called when default_args input changes with (backend_type, new_value)
    #[prop(optional)]
    on_default_args_change: Option<Callback<(String, String)>>,
    /// Called with (backend_type, version) when a version is activated via dropdown.
    #[prop(optional)]
    on_activate: Option<Callback<(String, String)>>,
    /// Called with (backend_type, version) when "Remove Version" is clicked from dropdown.
    #[prop(optional)]
    on_remove_version: Option<Callback<(String, String)>>,
) -> impl IntoView {
    let type_install = backend.r#type.clone();
    let type_update = backend.r#type.clone();
    let type_check = backend.r#type.clone();
    let type_delete = backend.r#type.clone();

    let installed = backend.installed;
    let display_name = backend.display_name.clone();
    let release_notes_url = backend.release_notes_url.clone();
    let backend_type = backend.r#type.clone();
    let bt_input = backend_type.clone();

    let update_available = backend.update.update_available.unwrap_or(false);
    let latest_version = backend.update.latest_version.clone();

    let default_args_initial = backend.default_args.join(" ");
    let default_args_signal = RwSignal::new(default_args_initial.clone());

    // All installed versions (sorted by installed_at DESC)
    let versions = backend.versions.clone();
    let version_count = versions.len();

    // Find the index of the active version to select it by default
    let active_index = versions.iter().position(|v| v.is_active).unwrap_or(0);

    // Track which version is currently selected for display
    let selected_version_idx = RwSignal::new(active_index);

    // Clone for selected info closure
    let versions_for_info = versions.clone();
    let selected_info = move || versions_for_info.get(selected_version_idx.get()).cloned();

    // Clone for active check closure
    let versions_for_active = versions.clone();
    let is_selected_active = move || {
        selected_version_idx.get() < version_count
            && versions_for_active[selected_version_idx.get()].is_active
    };

    view! {
        <fieldset style="border:1px solid var(--border,#ccc);padding:1rem;border-radius:6px;">
            <legend style="font-weight:600;display:flex;align-items:center;gap:0.5rem;flex-wrap:wrap;">
                <span>{display_name}</span>
                {if !installed {
                    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Not installed"</span> }.into_any()
                } else if is_selected_active() && version_count == 1 {
                    // Single installed version = it's the active one
                    view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Active"</span> }.into_any()
                } else if is_selected_active() {
                    view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Active"</span> }.into_any()
                } else {
                    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Installed"</span> }.into_any()
                }}
                {if update_available {
                    view! { <span class="badge" style="background:#3b82f6;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Update available"</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Version count badge when multiple versions exist */}
                {if version_count > 1 {
                    let count = version_count;
                    view! { <span class="badge" style="background:#64748b;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;">{format!("{} versions", count)}</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
            </legend>

            {/* Version selector dropdown */}
            {if installed && version_count > 1 {
                let activate_cb = on_activate;
                let remove_cb = on_remove_version;
                let vts = backend.r#type.clone();
                view! {
                    <div style="display:flex;align-items:center;gap:0.5rem;margin-bottom:0.75rem;">
                        <label style="font-size:0.8125rem;font-weight:600;">"Version:"</label>
                        <select
                            class="form-select"
                            style="font-size:0.8125rem;padding:0.25rem 0.5rem;min-width:180px;"
                            prop:value=move || selected_version_idx.get().to_string()
                            on:change=move |ev| {
                                if let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                    if let Ok(idx) = input.value().parse::<usize>() {
                                        selected_version_idx.set(idx);
                                    }
                                }
                            }
                        >
                            {versions.clone().iter().enumerate().map(|(i, v)| {
                                let label = if v.is_active {
                                    format!("{} (active)", v.version)
                                } else {
                                    v.version.clone()
                                };
                                view! {
                                    <option value=i.to_string()>{label}</option>
                                }.into_any()
                            }).collect::<Vec<_>>()}
                        </select>
                        {/* Version actions — only show when not on active version */}
                        {move || {
                            let idx = selected_version_idx.get();
                            if idx < version_count && !versions[idx].is_active {
                                let ver = versions[idx].version.clone();
                                let bt = vts.clone();
                                view! {
                                    <div style="display:flex;gap:0.375rem;margin-left:auto;">
                                        {if let Some(cb) = activate_cb {
                                            let ver_act = ver.clone();
                                            let bt_act = bt.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    class="btn btn-sm"
                                                    style="background:#22c55e;color:white;font-size:0.75rem;padding:0.25rem 0.625rem;"
                                                    on:click=move |_| {
                                                        cb.run((bt_act.clone(), ver_act.clone()));
                                                    }
                                                >
                                                    "Activate"
                                                </button>
                                            }.into_any()
                                        } else { view! { <span/> }.into_any() }}
                                        {if let Some(cb) = remove_cb {
                                            let ver_rem = ver.clone();
                                            let bt_rem = bt.clone();
                                            view! {
                                                <button
                                                    type="button"
                                                    class="btn btn-sm"
                                                    style="color:#dc2626;font-size:0.75rem;padding:0.25rem 0.625rem;"
                                                    on:click=move |_| {
                                                        cb.run((bt_rem.clone(), ver_rem.clone()));
                                                    }
                                                >
                                                    "Remove Version"
                                                </button>
                                            }.into_any()
                                        } else { view! { <span/> }.into_any() }}
                                    </div>
                                }.into_any()
                            } else {
                                view! { <span/> }.into_any()
                            }
                        }}
                    </div>
                }.into_any()
            } else {
                view! { <span/> }.into_any()
            }}

            <div style="display:flex;flex-direction:column;gap:0.5rem;">
                {/* Version info — derived from selected version */}
                {move || {
                    let info = selected_info();
                    view! {
                        {if let Some(ref v) = info {
                            let ver = v.version.clone();
                            view! { <div style="font-size:0.875rem;"><strong>"Version: "</strong>{ver}</div> }.into_any()
                        } else { view! { <span/> }.into_any() }}

                        {if let Some(ref v) = info {
                            if let Some(ref g) = v.gpu_type {
                                let label = g.label();
                                view! { <div style="font-size:0.875rem;"><strong>"GPU: "</strong>{label}</div> }.into_any()
                            } else { view! { <span/> }.into_any() }
                        } else { view! { <span/> }.into_any() }}

                        {if let Some(ref v) = info {
                            let path = v.path.clone();
                            view! { <div style="font-size:0.875rem;color:var(--muted,#666);"><strong>"Path: "</strong><code>{path}</code></div> }.into_any()
                        } else { view! { <span/> }.into_any() }}
                    }
                }}

                 <div style="display:flex;flex-direction:column;gap:0.25rem;">
                     <label style="font-size:0.875rem;font-weight:600;">"Default Args"</label>
                     <input
                         type="text"
                         placeholder="No default args set"
                         style="font-size:0.875rem;padding:0.375rem;border:1px solid var(--border,#ccc);border-radius:4px;font-family:monospace;"
                         prop:value=move || default_args_signal.get()
                         on:input=move |ev| {
                             if let Some(input) = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                                 default_args_signal.set(input.value());
                                 if let Some(cb) = &on_default_args_change {
                                     cb.run((bt_input.clone(), input.value()));
                                 }
                             }
                         }
                     />
                </div>

                {if update_available {
                    if let Some(lv) = latest_version {
                        view! { <div style="font-size:0.875rem;color:#3b82f6;"><strong>"Latest: "</strong>{lv}</div> }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                } else {
                    view! { <span/> }.into_any()
                }}
            </div>

            <div style="display:flex;gap:0.5rem;margin-top:1rem;flex-wrap:wrap;">
                {/* Install button */}
                {if !installed {
                    let cb = on_install;
                    let bt = type_install.clone();
                    view! {
                        <button
                            type="button"
                            class="btn btn-primary"
                            on:click=move |_| {
                                if let Some(c) = cb {
                                    c.run(bt.clone());
                                }
                            }
                        >
                            "Install"
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Check for updates — always when installed */}
                {if installed {
                    if let Some(cb) = on_check_updates {
                        let bt = type_check.clone();
                        view! {
                            <button
                                type="button"
                                class="btn btn-secondary"
                                on:click=move |_| {
                                    cb.run(bt.clone());
                                }
                            >
                                "Check for updates"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Update button — only when update available */}
                {if installed && update_available {
                    let cb = on_update;
                    let bt = type_update.clone();
                    view! {
                        <button
                            type="button"
                            class="btn btn-primary"
                            on:click=move |_| {
                                if let Some(c) = cb {
                                    c.run(bt.clone());
                                }
                            }
                        >
                            "Update"
                        </button>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {/* Uninstall — only when the selected version is active */}
                {move || {
                    if installed && is_selected_active() {
                        let cb = on_delete;
                        let bt = type_delete.clone();
                        view! {
                            <button
                                type="button"
                                class="btn btn-secondary"
                                style="color:#dc2626;"
                                on:click=move |_| {
                                    if let Some(c) = cb {
                                        c.run(bt.clone());
                                    }
                                }
                            >
                                "Uninstall"
                            </button>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                }}

                {/* Release notes */}
                {if let Some(url) = release_notes_url {
                    view! {
                        <a
                            href=url
                            target="_blank"
                            rel="noopener noreferrer"
                            class="btn btn-secondary"
                            style="text-decoration:none;"
                        >
                            "Release notes"
                        </a>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
            </div>
        </fieldset>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_type_label() {
        assert_eq!(
            GpuTypeDto::Cuda {
                version: "12.4".to_string()
            }
            .label(),
            "CUDA 12.4"
        );
        assert_eq!(GpuTypeDto::Vulkan.label(), "Vulkan");
        assert_eq!(GpuTypeDto::CpuOnly.label(), "CPU");
    }

    #[test]
    fn test_backend_card_dto_serialization() {
        let dto = BackendCardDto {
            r#type: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: false,
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: Some("https://example.com".to_string()),
            default_args: vec![],
            is_active: false,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("llama_cpp"));
        assert!(json.contains("\"installed\":false"));
    }

    #[test]
    fn test_backend_card_dto_is_active_field() {
        let dto_active = BackendCardDto {
            r#type: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: true,
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: None,
            default_args: vec![],
            is_active: true,
        };
        let json = serde_json::to_string(&dto_active).unwrap();
        assert!(json.contains("\"is_active\":true"));

        let dto_inactive = BackendCardDto {
            r#type: "llama_cpp".to_string(),
            display_name: "llama.cpp".to_string(),
            installed: true,
            info: None,
            versions: vec![],
            update: UpdateStatusDto::default(),
            release_notes_url: None,
            default_args: vec![],
            is_active: false,
        };
        let json2 = serde_json::to_string(&dto_inactive).unwrap();
        assert!(json2.contains("\"is_active\":false"));
    }

    #[test]
    fn test_backend_card_dto_is_active_default() {
        // Deserializing without is_active should default to false
        let json = r#"{
            "type": "llama_cpp",
            "display_name": "llama.cpp",
            "installed": true,
            "update": {},
            "default_args": []
        }"#;
        let dto: BackendCardDto = serde_json::from_str(json).unwrap();
        assert!(!dto.is_active);
    }
}
