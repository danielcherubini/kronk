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
pub struct BackendCardDto {
    pub r#type: String,
    pub display_name: String,
    pub installed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    #[serde(default)]
    pub update: UpdateStatusDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
    #[serde(default)]
    pub default_args: Vec<String>,
    /// Whether this specific version card is the active one.
    #[serde(default)]
    pub is_active: bool,
}

// ── Component ────────────────────────────────────────────────────────────────

/// BackendCard - displays one backend with install/update/delete actions.
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
    /// Called with (backend_type, version) when "Activate" is clicked.
    #[prop(optional)]
    on_activate: Option<Callback<(String, String)>>,
    /// Called with (backend_type, version) when "Remove Version" is clicked.
    #[prop(optional)]
    on_remove_version: Option<Callback<(String, String)>>,
) -> impl IntoView {
    let type_install = backend.r#type.clone();
    let type_update = backend.r#type.clone();
    let type_check = backend.r#type.clone();
    let type_delete = backend.r#type.clone();
    let type_activate = backend.r#type.clone();

    let installed = backend.installed;
    let is_active = backend.is_active;
    let display_name = backend.display_name.clone();
    let release_notes_url = backend.release_notes_url.clone();
    let backend_type = backend.r#type.clone();
    let bt_input = backend_type.clone();

    let update_available = backend.update.update_available.unwrap_or(false);
    let latest_version = backend.update.latest_version.clone();

    let default_args_initial = backend.default_args.join(" ");
    let default_args_signal = RwSignal::new(default_args_initial.clone());

    let (path_str, version_str, gpu_label) = match &backend.info {
        Some(info) => (
            Some(info.path.clone()),
            Some(info.version.clone()),
            info.gpu_type.as_ref().map(|g| g.label()),
        ),
        None => (None, None, None),
    };

    // Clone the version for the Activate button callback.
    let activate_version = backend.info.as_ref().map(|info| info.version.clone());

    view! {
        <fieldset style="border:1px solid var(--border,#ccc);padding:1rem;border-radius:6px;">
            <legend style="font-weight:600;display:flex;align-items:center;gap:0.5rem;">
                <span>{display_name}</span>
                {if is_active {
                    view! { <span class="badge" style="background:#22c55e;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Active"</span> }.into_any()
                } else if installed {
                    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Installed"</span> }.into_any()
                } else {
                    view! { <span class="badge" style="background:#94a3b8;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Not installed"</span> }.into_any()
                }}
                {if update_available {
                    view! { <span class="badge" style="background:#3b82f6;color:white;padding:0.125rem 0.5rem;border-radius:4px;font-size:0.75rem;font-weight:500;">"Update available"</span> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}
            </legend>

            <div style="display:flex;flex-direction:column;gap:0.5rem;">
                {if let Some(v) = version_str {
                    view! { <div style="font-size:0.875rem;"><strong>"Version: "</strong>{v}</div> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {if let Some(g) = gpu_label {
                    view! { <div style="font-size:0.875rem;"><strong>"GPU: "</strong>{g}</div> }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                {if let Some(p) = path_str {
                    view! { <div style="font-size:0.875rem;color:var(--muted,#666);"><strong>"Path: "</strong><code>{p}</code></div> }.into_any()
                } else {
                    view! { <span/> }.into_any()
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

                {if installed {
                    // Always show "Check for updates" button
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

                // Show "Uninstall" (removes ALL versions) only on the active version card
                {if installed && is_active {
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
                }}

                // Show "Remove Version" and "Activate" buttons on inactive versions (only if version info exists)
                {if installed && !is_active {
                    let cb_remove = on_remove_version;
                    let cb_activate = on_activate;
                    let bt = type_delete.clone();
                    let bt_activate = type_activate.clone();

                    // Only render buttons if we have a valid version string
                    if let Some(ref ver) = activate_version {
                        let ver_remove = ver.clone();
                        let ver_activate = ver.clone();
                        view! {
                            <>
                                <button
                                    type="button"
                                    class="btn btn-secondary"
                                    style="color:#6b7280;"
                                    on:click=move |_| {
                                        if let Some(c) = cb_remove {
                                            c.run((bt.clone(), ver_remove.clone()));
                                        }
                                    }
                                >
                                    "Remove Version"
                                </button>
                                <button
                                    type="button"
                                    class="btn btn-primary"
                                    style="background:#22c55e;"
                                    on:click=move |_| {
                                        if let Some(c) = cb_activate {
                                            c.run((bt_activate.clone(), ver_activate.clone()));
                                        }
                                    }
                                >
                                    "Activate"
                                </button>
                            </>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }
                } else {
                    view! { <span/> }.into_any()
                }}

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
