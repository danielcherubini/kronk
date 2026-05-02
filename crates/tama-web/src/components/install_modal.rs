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

    let supported_versions = RwSignal::new(capabilities.supported_cuda_versions.clone());
    let backend_type_submit = backend_type.clone();



    let display_name = match backend_type.as_str() {
        "llama_cpp" => "llama.cpp",
        "ik_llama" => "ik_llama.cpp",
        other => other,
    };
    let title = format!("Install {display_name}");

    view! {
        <div class="modal-backdrop modal-backdrop--open">
            <div class="modal" on:click=|e: leptos::ev::MouseEvent| e.stop_propagation()>
                <div class="modal-header">
                    <h2 class="modal-title">{title}</h2>
                    <button
                        type="button"
                        class="modal-close"
                        on:click=move |_| { if let Some(cb) = &on_cancel { cb.run(()); } }
                        aria-label="Close"
                    >"✕"</button>
                </div>
                <div class="modal-body">
                    {/* Build prerequisites warning */}
                    {if !can_build {
                        view! {
                            <div class="alert alert--warning">
                                <span class="alert__icon">"⚠"</span>
                                "Build prerequisites missing (git/cmake/compiler). Source builds will fail."
                            </div>
                        }.into_any()
                    } else {
                        view! { <span/> }.into_any()
                    }}

                    {/* GPU Type */}
                    <div class="form-group">
                        <label class="form-label">"GPU Acceleration"</label>
                        <select
                            on:change=move |e| gpu_kind.set(event_target_value(&e))
                            class="form-select"
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
                            let versions = supported_versions.get();
                            view! {
                                <div class="form-group">
                                    <label class="form-label">"CUDA Version"</label>
                                    <select
                                        on:change=move |e| cuda_version.set(event_target_value(&e))
                                        class="form-select"
                                    >
                                        {versions.iter().cloned().map(|v| {
                                            let v_for_selected = v.clone();
                                            view! {
                                                <option
                                                    value=v_for_selected.clone()
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
                            <div class="form-group">
                                <label class="form-label">"Version"</label>
                                <input
                                    type="text"
                                    placeholder="latest"
                                    prop:value=move || version.get()
                                    on:input=move |e| version.set(event_target_value(&e))
                                    class="form-input"
                                />
                                <p class="form-hint">
                                    "Use 'latest' or a specific tag like 'b8407'."
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="alert alert--info">
                                "ik_llama is built from the latest main branch commit."
                            </div>
                        }.into_any()
                    }}

                    {/* Build from source */}
                    <div class="form-group">
                        <div class="form-check">
                            <input
                                type="checkbox"
                                prop:checked=move || effective_build_from_source.get()
                                prop:disabled=move || force_source.get()
                                on:change=move |e| user_build_from_source.set(event_target_checked(&e))
                            />
                            <span class="form-check-label">"Build from source"</span>
                        </div>
                        {move || {
                            if force_source.get() {
                                let reason = if is_ik_llama {
                                    "ik_llama always builds from source"
                                } else {
                                    "No prebuilt CUDA binary for Linux — source build required"
                                };
                                view! {
                                    <p class="form-hint" style="margin-left: 1.5rem;">
                                        {format!("Forced: {reason}")}
                                    </p>
                                }.into_any()
                            } else {
                                view! { <span/> }.into_any()
                            }
                        }}
                    </div>

                    {/* Force overwrite */}
                    <div class="form-group">
                        <div class="form-check">
                            <input
                                type="checkbox"
                                prop:checked=move || force_overwrite.get()
                                on:change=move |e| force_overwrite.set(event_target_checked(&e))
                            />
                            <span class="form-check-label">"Force overwrite existing installation"</span>
                        </div>
                    </div>

                    {/* Actions */}
                    <div class="form-actions">
                        <button
                            type="button"
                            class="btn btn-secondary"
                            on:click=move |_| { if let Some(cb) = &on_cancel { cb.run(()); } }
                        >
                            "Cancel"
                        </button>
                        <button
                            type="button"
                            class="btn btn-primary"
                            on:click=move |_| {
                                let kind = gpu_kind.get();
                                let gpu_type = match kind.as_str() {
                                    "cuda" => GpuTypeDto::Cuda { version: cuda_version.get() },
                                    "vulkan" => GpuTypeDto::Vulkan,
                                    "metal" => GpuTypeDto::Metal,
                                    "rocm" => GpuTypeDto::Rocm { version: "7.2".to_string() },
                                    _ => GpuTypeDto::CpuOnly,
                                };
                                let request = InstallRequest {
                                    backend_type: backend_type_submit.clone(),
                                    version: None,
                                    gpu_type,
                                    build_from_source: effective_build_from_source.get(),
                                    force: force_overwrite.get(),
                                };
                                if let Some(cb) = &on_submit {
                                    cb.run(request);
                                }
                            }
                        >
                            "Install"
                        </button>
                    </div>
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
