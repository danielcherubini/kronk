//! Install modal - configures install request for a backend.

use crate::components::backend_card::GpuTypeDto;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

// ── DTOs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CapabilitiesDto {
    pub os: String,
    pub arch: String,
    pub git_available: bool,
    pub cmake_available: bool,
    pub compiler_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_cuda_version: Option<String>,
    #[serde(default)]
    pub supported_cuda_versions: Vec<String>,
}

impl Default for CapabilitiesDto {
    fn default() -> Self {
        Self {
            os: String::new(),
            arch: String::new(),
            git_available: false,
            cmake_available: false,
            compiler_available: false,
            detected_cuda_version: None,
            supported_cuda_versions: vec![
                "11.1".to_string(),
                "12.4".to_string(),
                "13.1".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InstallRequest {
    pub backend_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub gpu_type: GpuTypeDto,
    pub build_from_source: bool,
    pub force: bool,
}

// ── Component ────────────────────────────────────────────────────────────────

/// InstallModal - configures and submits an install request.
#[component]
#[allow(dead_code)]
pub fn InstallModal(
    /// Backend type to install (e.g. "llama_cpp", "ik_llama")
    backend_type: String,
    /// System capabilities for defaults and validation
    capabilities: CapabilitiesDto,
    /// Called with the request payload when user clicks Install
    #[prop(optional)]
    on_submit: Option<Callback<InstallRequest>>,
    /// Called when user clicks Cancel or closes the modal
    #[prop(optional)]
    on_cancel: Option<Callback<()>>,
) -> impl IntoView {
    let is_ik_llama = backend_type == "ik_llama";
    let is_linux = capabilities.os == "linux";

    // Default GPU type: CUDA if detected, otherwise CPU
    let default_gpu = if let Some(v) = &capabilities.detected_cuda_version {
        GpuTypeDto::Cuda { version: v.clone() }
    } else {
        GpuTypeDto::CpuOnly
    };

    // Signals for form state
    let gpu_kind = RwSignal::new(match default_gpu {
        GpuTypeDto::Cuda { .. } => "cuda".to_string(),
        GpuTypeDto::Vulkan => "vulkan".to_string(),
        GpuTypeDto::Metal => "metal".to_string(),
        GpuTypeDto::Rocm { .. } => "rocm".to_string(),
        GpuTypeDto::CpuOnly => "cpu".to_string(),
        GpuTypeDto::Custom => "cpu".to_string(),
    });

    let cuda_version = RwSignal::new(
        capabilities
            .detected_cuda_version
            .clone()
            .or_else(|| capabilities.supported_cuda_versions.first().cloned())
            .unwrap_or_else(|| "12.4".to_string()),
    );

    let version = RwSignal::new(String::from("latest"));
    let force_overwrite = RwSignal::new(false);

    // Build-from-source: forced for ik_llama (any OS) and linux+cuda
    let user_build_from_source = RwSignal::new(false);
    let backend_type_for_force = backend_type.clone();
    let force_source = Memo::new(move |_| {
        let is_ik = backend_type_for_force == "ik_llama";
        let is_cuda = gpu_kind.get() == "cuda";
        is_ik || (is_linux && is_cuda)
    });
    let effective_build_from_source =
        Memo::new(move |_| force_source.get() || user_build_from_source.get());

    // Prereq check
    let can_build = capabilities.git_available
        && capabilities.cmake_available
        && capabilities.compiler_available;
    let submit_disabled = Memo::new(move |_| effective_build_from_source.get() && !can_build);

    let supported_versions = capabilities.supported_cuda_versions.clone();

    let backend_type_submit = backend_type.clone();
    let on_submit_handler = move |_| {
        let kind = gpu_kind.get();
        let gpu_type = match kind.as_str() {
            "cuda" => GpuTypeDto::Cuda {
                version: cuda_version.get(),
            },
            "vulkan" => GpuTypeDto::Vulkan,
            "metal" => GpuTypeDto::Metal,
            "rocm" => GpuTypeDto::Rocm {
                version: "7.2".to_string(),
            },
            _ => GpuTypeDto::CpuOnly,
        };
        let v = version.get();
        let request = InstallRequest {
            backend_type: backend_type_submit.clone(),
            version: if v.is_empty() || v == "latest" {
                None
            } else {
                Some(v)
            },
            gpu_type,
            build_from_source: effective_build_from_source.get(),
            force: force_overwrite.get(),
        };
        if let Some(cb) = on_submit {
            cb.run(request);
        }
    };

    let on_cancel_handler = move |_| {
        if let Some(cb) = on_cancel {
            cb.run(());
        }
    };
    let on_cancel_overlay = on_cancel_handler;

    let display_name = match backend_type.as_str() {
        "llama_cpp" => "llama.cpp",
        "ik_llama" => "ik_llama.cpp",
        other => other,
    };
    let title = format!("Install {display_name}");

    view! {
        <div
            class="modal-overlay"
            style="position:fixed;inset:0;background:rgba(0,0,0,0.5);display:flex;align-items:center;justify-content:center;z-index:1000;"
            on:click=on_cancel_overlay
        >
            <div
                class="modal"
                style="background:var(--bg,white);padding:1.5rem;border-radius:8px;max-width:500px;width:90%;max-height:90vh;overflow-y:auto;"
                on:click=|e: leptos::ev::MouseEvent| e.stop_propagation()
            >
                <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:1rem;">
                    <h2 style="margin:0;font-size:1.25rem;font-weight:600;">{title}</h2>
                    <button
                        type="button"
                        class="btn btn-sm"
                        style="background:none;border:none;font-size:1.5rem;cursor:pointer;"
                        on:click=on_cancel_handler
                    >
                        "×"
                    </button>
                </div>

                {if !can_build {
                    view! {
                        <div style="background:#fef3c7;border:1px solid #f59e0b;padding:0.75rem;border-radius:4px;margin-bottom:1rem;font-size:0.875rem;">
                            "⚠ Build prerequisites missing (git/cmake/compiler). Source builds will fail."
                        </div>
                    }.into_any()
                } else {
                    view! { <span/> }.into_any()
                }}

                <div style="display:flex;flex-direction:column;gap:1rem;">
                    {/* GPU Type */}
                    <div>
                        <label style="display:block;font-weight:500;margin-bottom:0.25rem;font-size:0.875rem;">
                            "GPU Acceleration"
                        </label>
                        <select
                            on:change=move |e| gpu_kind.set(event_target_value(&e))
                            style="width:100%;padding:0.5rem;border:1px solid var(--border,#ccc);border-radius:4px;"
                        >
                            <option value="cpu" selected=move || gpu_kind.get() == "cpu">"CPU Only"</option>
                            <option value="cuda" selected=move || gpu_kind.get() == "cuda">"CUDA (NVIDIA)"</option>
                            <option value="vulkan" selected=move || gpu_kind.get() == "vulkan">"Vulkan"</option>
                            <option value="metal" selected=move || gpu_kind.get() == "metal">"Metal (macOS)"</option>
                            <option value="rocm" selected=move || gpu_kind.get() == "rocm">"ROCm (AMD)"</option>
                        </select>
                    </div>

                    {/* CUDA version */}
                    {move || {
                        if gpu_kind.get() == "cuda" {
                            let versions = supported_versions.clone();
                            view! {
                                <div>
                                    <label style="display:block;font-weight:500;margin-bottom:0.25rem;font-size:0.875rem;">
                                        "CUDA Version"
                                    </label>
                                    <select
                                        on:change=move |e| cuda_version.set(event_target_value(&e))
                                        style="width:100%;padding:0.5rem;border:1px solid var(--border,#ccc);border-radius:4px;"
                                    >
                                        {versions.into_iter().map(|v| {
                                            let v_for_selected = v.clone();
                                            let v_for_value = v.clone();
                                            view! {
                                                <option
                                                    value=v_for_value
                                                    selected=move || cuda_version.get() == v_for_selected
                                                >
                                                    {v}
                                                </option>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </div>
                            }.into_any()
                        } else {
                            view! { <span/> }.into_any()
                        }
                    }}

                    {/* Version */}
                    {if !is_ik_llama {
                        view! {
                            <div>
                                <label style="display:block;font-weight:500;margin-bottom:0.25rem;font-size:0.875rem;">
                                    "Version"
                                </label>
                                <input
                                    type="text"
                                    placeholder="latest"
                                    prop:value=move || version.get()
                                    on:input=move |e| version.set(event_target_value(&e))
                                    style="width:100%;padding:0.5rem;border:1px solid var(--border,#ccc);border-radius:4px;"
                                />
                                <p style="font-size:0.75rem;color:var(--muted,#666);margin-top:0.25rem;">
                                    "Use 'latest' or a specific tag like 'b8407'."
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div style="background:#dbeafe;border:1px solid #3b82f6;padding:0.75rem;border-radius:4px;font-size:0.875rem;">
                                "ik_llama is built from the latest main branch commit."
                            </div>
                        }.into_any()
                    }}

                    {/* Build from source */}
                    <div>
                        <label style="display:flex;align-items:center;gap:0.5rem;font-size:0.875rem;">
                            <input
                                type="checkbox"
                                prop:checked=move || effective_build_from_source.get()
                                prop:disabled=move || force_source.get()
                                on:change=move |e| user_build_from_source.set(event_target_checked(&e))
                            />
                            <span>"Build from source"</span>
                        </label>
                        {move || {
                            if force_source.get() {
                                let reason = if is_ik_llama {
                                    "ik_llama always builds from source"
                                } else {
                                    "No prebuilt CUDA binary for Linux — source build required"
                                };
                                view! {
                                    <p style="font-size:0.75rem;color:var(--muted,#666);margin-top:0.25rem;margin-left:1.5rem;">
                                        {format!("Forced: {reason}")}
                                    </p>
                                }.into_any()
                            } else {
                                view! { <span/> }.into_any()
                            }
                        }}
                    </div>

                    {/* Force overwrite */}
                    <div>
                        <label style="display:flex;align-items:center;gap:0.5rem;font-size:0.875rem;">
                            <input
                                type="checkbox"
                                prop:checked=move || force_overwrite.get()
                                on:change=move |e| force_overwrite.set(event_target_checked(&e))
                            />
                            <span>"Force overwrite existing installation"</span>
                        </label>
                    </div>
                </div>

                <div style="display:flex;gap:0.5rem;margin-top:1.5rem;justify-content:flex-end;">
                    <button
                        type="button"
                        class="btn btn-secondary"
                        on:click=on_cancel_handler
                    >
                        "Cancel"
                    </button>
                    <button
                        type="button"
                        class="btn btn-primary"
                        prop:disabled=move || submit_disabled.get()
                        on:click=on_submit_handler
                    >
                        "Install"
                    </button>
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_install_request_serialization() {
        let req = InstallRequest {
            backend_type: "llama_cpp".to_string(),
            version: Some("b8407".to_string()),
            gpu_type: GpuTypeDto::Cuda {
                version: "12.4".to_string(),
            },
            build_from_source: false,
            force: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("llama_cpp"));
        assert!(json.contains("b8407"));
        assert!(json.contains("\"kind\":\"cuda\""));
    }

    #[test]
    fn test_capabilities_default() {
        let caps = CapabilitiesDto::default();
        assert_eq!(caps.supported_cuda_versions.len(), 3);
    }

    #[test]
    fn test_install_request_serialization_ik_llama() {
        let req = InstallRequest {
            backend_type: "ik_llama".to_string(),
            version: None,
            gpu_type: GpuTypeDto::CpuOnly,
            build_from_source: true,
            force: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("ik_llama"));
        assert!(json.contains("cpu_only"));
        assert!(json.contains("build_from_source"));
    }

    #[test]
    fn test_install_request_serialization_vulkan() {
        let req = InstallRequest {
            backend_type: "llama_cpp".to_string(),
            version: Some("latest".to_string()),
            gpu_type: GpuTypeDto::Vulkan,
            build_from_source: false,
            force: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("llama_cpp"));
        assert!(json.contains("vulkan"));
        assert!(json.contains("force"));
    }

    #[test]
    fn test_install_request_serialization_rocm() {
        let req = InstallRequest {
            backend_type: "llama_cpp".to_string(),
            version: Some("7.2".to_string()),
            gpu_type: GpuTypeDto::Rocm {
                version: "7.2".to_string(),
            },
            build_from_source: false,
            force: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("rocm"));
    }

    #[test]
    fn test_install_request_serialization_custom() {
        let req = InstallRequest {
            backend_type: "custom".to_string(),
            version: None,
            gpu_type: GpuTypeDto::CpuOnly,
            build_from_source: false,
            force: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("custom"));
    }

    #[test]
    fn test_install_request_roundtrip() {
        let original = InstallRequest {
            backend_type: "llama_cpp".to_string(),
            version: Some("b8407".to_string()),
            gpu_type: GpuTypeDto::Cuda {
                version: "12.4".to_string(),
            },
            build_from_source: false,
            force: false,
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: InstallRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.backend_type, "llama_cpp");
        assert_eq!(deserialized.version, Some("b8407".to_string()));
        assert!(!deserialized.build_from_source);
    }
}
