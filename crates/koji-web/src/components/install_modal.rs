#[cfg(feature = "ssr")]
use leptos::ev::MouseEvent;
#[cfg(feature = "ssr")]
use leptos::prelude::*;
#[cfg(feature = "ssr")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRequest {
    pub backend_type: String,
    pub version: Option<String>,
    pub gpu_type: GpuTypeDto,
    pub build_from_source: bool,
    pub force: bool,
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
pub struct CapabilitiesDto {
    pub os: String,
    pub arch: String,
    pub git_available: bool,
    pub cmake_available: bool,
    pub compiler_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_cuda_version: Option<String>,
    pub supported_cuda_versions: Vec<String>,
}

#[cfg(feature = "ssr")]
/// Install modal component
#[component]
pub fn InstallModal(
    visible: Signal<bool>,
    backend_type: Signal<String>,
    capabilities: Option<Signal<CapabilitiesDto>>,
    initial_request: Option<InstallRequest>,
    on_submit: EventHandler<InstallRequest>,
    on_cancel: EventHandler<MouseEvent>,
    #[prop(into)] on_retry: Option<EventHandler<InstallRequest>>,
    #[prop(into)] on_dismiss: Option<EventHandler<MouseEvent>>,
) -> impl IntoView {
    let is_visible = visible;
    let current_backend = backend_type;

    let (gpu_type, set_gpu_type) = create_signal(
        initial_request
            .as_ref()
            .map(|r| r.gpu_type.clone())
            .unwrap_or(GpuTypeDto::CpuOnly),
    );
    let (version, set_version) = create_signal(
        initial_request
            .as_ref()
            .and_then(|r| r.version.clone())
            .unwrap_or_else(|| "latest".to_string()),
    );
    let (build_from_source, set_build_from_source) = create_signal(
        initial_request
            .as_ref()
            .map(|r| r.build_from_source)
            .unwrap_or(false),
    );
    let (force_overwrite, set_force_overwrite) =
        create_signal(initial_request.as_ref().map(|r| r.force).unwrap_or(false));
    let (notices, set_notices) = create_signal(vec![]);

    let is_ik_llama = move || current_backend.get() == "ik_llama";
    let is_linux = move || std::env::consts::OS == "linux";
    let is_cuda = move || matches!(gpu_type.get(), GpuTypeDto::Cuda { .. });

    let force_source = move || {
        (is_ik_llama() || (is_linux() && is_cuda()))
            .then_some(true)
            .unwrap_or(build_from_source.get())
    };

    let can_build = move || {
        capabilities
            .as_ref()
            .map(|caps| {
                let c = caps.get();
                c.git_available && c.cmake_available && c.compiler_available
            })
            .unwrap_or(true)
    };

    let submit_disabled = move || force_source() && !can_build();

    let submit = move |ev: MouseEvent| {
        ev.prevent_default();
        let request = InstallRequest {
            backend_type: current_backend.get(),
            version: if version.get() == "latest" {
                None
            } else {
                Some(version.get())
            },
            gpu_type: gpu_type.get(),
            build_from_source: force_source(),
            force: force_overwrite.get(),
        };
        on_submit.dispatch(request);
        on_cancel.dispatch(MouseEvent::default());
    };

    let cancel = move |ev: MouseEvent| {
        ev.prevent_default();
        on_cancel.dispatch(ev);
    };

    let retry = move |ev: MouseEvent| {
        ev.prevent_default();
        if let Some(handler) = on_retry {
            let request = InstallRequest {
                backend_type: current_backend.get(),
                version: if version.get() == "latest" {
                    None
                } else {
                    Some(version.get())
                },
                gpu_type: gpu_type.get(),
                build_from_source: force_source(),
                force: force_overwrite.get(),
            };
            handler.dispatch(request);
        }
    };

    let dismiss = move |ev: MouseEvent| {
        ev.prevent_default();
        if let Some(handler) = on_dismiss {
            handler.dispatch(ev);
        }
    };

    let supported_versions = move || {
        capabilities
            .as_ref()
            .map(|caps| caps.get().supported_cuda_versions.clone())
            .unwrap_or_default()
    };

    let detected_cuda = move || {
        capabilities
            .as_ref()
            .and_then(|caps| caps.get().detected_cuda_version.clone())
    };

    let cuda_version = move || {
        detected_cuda()
            .or_else(|| supported_versions().first().cloned())
            .unwrap_or_else(|| "12.4".to_string())
    };

    let needs_source_warning = move || force_source() && !can_build();

    view! {
        {if is_visible() {
            view! {
                <div class="modal-overlay" on:click=cancel>
                    <div class="modal" on:click=|e| e.stop_propagation()>
                        <div class="modal-header">
                            <h2 class="text-xl font-bold">
                                {match current_backend.get().as_str() {
                                    "llama_cpp" => "Install llama.cpp",
                                    "ik_llama" => "Install ik_llama.cpp",
                                    _ => "Install Backend",
                                }}
                            </h2>
                            <button class="btn btn-sm btn-ghost" on:click=cancel>"×"</button>
                        </div>

                        <div class="modal-body space-y-4">
                            {if needs_source_warning() {
                                view! {
                                    <div class="alert alert-warning">
                                        <svg class="w-5 h-5 text-warning" fill="currentColor" viewBox="0 0 20 20">
                                            <path fill-rule="evenodd" d="M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z" clip-rule="evenodd"/>
                                        </svg>
                                        <span>"cmake/compiler not found — source build will fail"</span>
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}

                            {if !notices.get().is_empty() {
                                view! {
                                    <div class="space-y-2">
                                        {notices.get().iter().map(|notice| {
                                            view! {
                                                <div class="alert alert-info">
                                                    {notice.clone()}
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}

                            {/* GPU Type */}
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "GPU Acceleration"
                                </label>
                                <select
                                    value={gpu_type.get_untracked()}
                                    on:change=move |ev| {
                                        let val = event_target_value(&ev);
                                        let v = match val.as_str() {
                                            "cuda" => GpuTypeDto::Cuda { version: cuda_version() },
                                            "vulkan" => GpuTypeDto::Vulkan,
                                            "metal" => GpuTypeDto::Metal,
                                            "rocm" => GpuTypeDto::Rocm { version: "7.2".to_string() },
                                            "cpu" => GpuTypeDto::CpuOnly,
                                            _ => GpuTypeDto::CpuOnly,
                                        };
                                        set_gpu_type.set(v);
                                    }
                                    class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                                >
                                    <option value="cpu">"CPU Only"</option>
                                    <option value="cuda">"CUDA"</option>
                                    <option value="vulkan">"Vulkan"</option>
                                    <option value="metal">"Metal (macOS)"</option>
                                    <option value="rocm">"ROCm"</option>
                                </select>
                            </div>

                            {/* CUDA Version */}
                            {if matches!(gpu_type.get(), GpuTypeDto::Cuda { .. }) {
                                view! {
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "CUDA Version"
                                        </label>
                                        <select
                                            value={cuda_version()}
                                            on:change=move |ev| {
                                                let val = event_target_value(&ev);
                                                let current = gpu_type.get();
                                                if let GpuTypeDto::Cuda { .. } = current {
                                                    set_gpu_type.set(GpuTypeDto::Cuda { version: val });
                                                }
                                            }
                                            class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                                        >
                                            {supported_versions().into_iter().map(|v| {
                                                view! { <option value={v.clone()}>{v}</option> }
                                            }).collect::<Vec<_>>()}
                                        </select>
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}

                            {/* Version */}
                            {if !is_ik_llama() {
                                view! {
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Version"
                                        </label>
                                        <input
                                            type="text"
                                            value={version.get_untracked()}
                                            placeholder="latest"
                                            on:input=move |ev| {
                                                set_version.set(event_target_value(&ev));
                                            }
                                            class="block w-full rounded-md border-gray-300 shadow-sm focus:border-blue-500 focus:ring-blue-500 sm:text-sm p-2 border"
                                        />
                                    </div>
                                }
                                .into_any()
                            } else {
                                view! {
                                    <div class="alert alert-info">
                                        "ik_llama always builds from source"
                                    </div>
                                }
                                .into_any()
                            }}

                            {/* Build from source */}
                            <div>
                                <label class="flex items-center gap-2">
                                    <input
                                        type="checkbox"
                                        checked={build_from_source.get_untracked()}
                                        disabled={force_source()}
                                        on:change=move |ev| {
                                            let checked = event_target_checked(&ev);
                                            set_build_from_source.set(checked);
                                        }
                                        class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                                    />
                                    <span class="text-sm">"Build from source"</span>
                                </label>
                                {if force_source() {
                                    view! {
                                        <p class="text-sm text-gray-500 mt-1">
                                            "Forced: " {if is_ik_llama() {
                                                "ik_llama always builds from source"
                                            } else if is_linux() && is_cuda() {
                                                "No prebuilt CUDA binary for Linux"
                                            } else {
                                                ""
                                            }}
                                        </p>
                                    }
                                    .into_any()
                                } else {
                                    view! { <></> }.into_any()
                                }}
                            </div>

                            {/* Force overwrite */}
                            <div>
                                <label class="flex items-center gap-2">
                                    <input
                                        type="checkbox"
                                        checked={force_overwrite.get_untracked()}
                                        on:change=move |ev| {
                                            let checked = event_target_checked(&ev);
                                            set_force_overwrite.set(checked);
                                        }
                                        class="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                                    />
                                    <span class="text-sm">"Force overwrite existing installation"</span>
                                </label>
                            </div>
                        </div>

                        <div class="modal-footer flex gap-2">
                            {if let Some(_) = on_retry {
                                view! {
                                    <button
                                        class="btn btn-secondary"
                                        on:click=retry
                                        disabled={submit_disabled()}
                                    >
                                        "Retry"
                                    </button>
                                }
                                .into_any()
                            } else {
                                view! { <></> }.into_any()
                            }}
                            <button
                                class="btn btn-primary"
                                on:click=submit
                                disabled={submit_disabled()}
                            >
                                "Install"
                            </button>
                            <button class="btn btn-ghost" on:click=cancel>"Cancel"</button>
                        </div>
                    </div>
                </div>
            }
            .into_any()
        } else {
            view! { <></> }.into_any()
        }}
    }
}
