#[cfg(feature = "ssr")]
use leptos::ev::MouseEvent;
#[cfg(feature = "ssr")]
use leptos::prelude::*;
#[cfg(feature = "ssr")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendCardDto {
    pub r#type: String,
    pub display_name: String,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<BackendInfoDto>,
    #[serde(skip_serializing_if = "UpdateStatusDto::is_default")]
    pub update: UpdateStatusDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes_url: Option<String>,
}

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendInfoDto {
    pub name: String,
    pub version: String,
    pub path: String,
    pub installed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_type: Option<GpuTypeDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<BackendSourceDto>,
}

#[cfg(feature = "ssr")]
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

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum BackendSourceDto {
    Prebuilt {
        version: String,
    },
    SourceCode {
        version: String,
        git_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
}

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct UpdateStatusDto {
    pub checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_available: Option<bool>,
}

#[cfg(feature = "ssr")]
impl UpdateStatusDto {
    pub fn is_default(&self) -> bool {
        !self.checked
    }
}

#[cfg(feature = "ssr")]
/// Backend card component
#[component]
pub fn BackendCard(
    #[prop(into)] name: String,
    #[prop(into)] path: Signal<String>,
    #[prop(into)] version: Signal<String>,
    on_install: EventHandler<MouseEvent>,
    on_update: EventHandler<MouseEvent>,
    on_delete: EventHandler<MouseEvent>,
    #[prop(into)] last_request: Option<InstallRequest>,
    on_cache_request: EventHandler<InstallRequest>,
) -> impl IntoView {
    let is_installed = move || !path().is_empty();

    let gpu_type = move || {
        // For now, just show based on backend type
        match name.as_str() {
            "llama_cpp" => "CPU".to_string(),
            "ik_llama" => "CPU (builds from source)".to_string(),
            _ => "Custom".to_string(),
        }
    };

    let update_available = move || {
        // For now, always show update available if installed
        is_installed().then_some(true)
    };

    view! {
        <div class="card">
            <div class="card-header">
                <div class="flex items-center justify-between">
                    <h3 class="font-semibold text-lg">{name.clone()}</h3>
                    {if is_installed() {
                        view! {
                            <span class="badge badge-success">
                                "Installed: " {version()}
                            </span>
                        }
                        .into_any()
                    } else {
                        view! {
                            <span class="badge badge-secondary">
                                "Not installed"
                            </span>
                        }
                        .into_any()
                    }}
                </div>
            </div>

            <div class="card-body">
                {if is_installed() {
                    view! {
                        <div class="mb-3">
                            {if let Some(gpu) = Some(gpu_type()) {
                                view! { <p class="text-sm text-gray-600">"GPU: " {gpu}</p> }.into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}
                            <p class="text-sm text-gray-600">"Path: " {path()}</p>
                        </div>

                        {if update_available() {
                            view! {
                                <div class="mb-3 p-3 bg-amber-50 border border-amber-200 rounded">
                                    <p class="text-sm text-amber-800">
                                        "Update available"
                                    </p>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! { <p class="text-sm text-green-600">"Up to date"</p> }.into_any()
                        }}

                        <div class="flex gap-2">
                            {if update_available() {
                                view! {
                                    <button
                                        class="btn btn-sm btn-primary"
                                        on:click=move |_| on_update.dispatch(MouseEvent::default())
                                    >
                                        "Update"
                                    </button>
                                }
                                .into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}
                            <button
                                class="btn btn-sm btn-danger"
                                on:click=move |_| on_delete.dispatch(MouseEvent::default())
                            >
                                "Delete"
                            </button>
                        </div>
                    }
                    .into_any()
                } else {
                    view! {
                        <p class="text-sm text-gray-600 mb-3">
                            {match name.as_str() {
                                "llama_cpp" => "llama.cpp inference backend",
                                "ik_llama" => "ik_llama.cpp inference backend (builds from source)",
                                _ => "Custom backend",
                            }}
                        </p>
                        <div class="flex gap-2">
                            <button
                                class="btn btn-sm btn-primary"
                                on:click=move |_| on_install.dispatch(MouseEvent::default())
                            >
                                "Install"
                            </button>
                        </div>
                    }
                    .into_any()
                }}
            </div>
        </div>
    }
}

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRequest {
    pub backend_type: String,
    pub version: Option<String>,
    pub gpu_type: GpuTypeDto,
    pub build_from_source: bool,
    pub force: bool,
}
