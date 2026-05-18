#[allow(unused_imports)]
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
#[allow(unused_imports)]
use std::sync::Arc;

#[allow(unused_imports)]
use super::resolve_model_id;
#[allow(unused_imports)]
use crate::api::{load_config_from_state, trigger_proxy_reload};
#[allow(unused_imports)]
use crate::server::AppState;

/// Maximum lengths for ModelBody fields.
const MAX_BACKEND: usize = 256;
const MAX_MODEL: usize = 256;
const MAX_QUANT: usize = 128;
const MAX_MMPROJ: usize = 128;
const MAX_API_NAME: usize = 128;
const MAX_DISPLAY_NAME: usize = 256;
const MAX_CACHE_TYPE: usize = 32;

/// Body for create/update model.
#[derive(serde::Deserialize)]
pub struct ModelBody {
    pub backend: String,
    #[serde(default)]
    pub gpu_variant: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub quant: Option<String>,
    #[serde(default)]
    pub mmproj: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub sampling: Option<tama_core::profiles::SamplingParams>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub context_length: Option<u32>,
    #[serde(default)]
    pub num_parallel: Option<u32>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_name: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub gpu_layers: Option<u32>,
    #[serde(default)]
    pub quants: Option<std::collections::BTreeMap<String, tama_core::config::QuantEntry>>,
    #[serde(default)]
    pub modalities: Option<tama_core::config::ModelModalities>,
    #[serde(default)]
    pub kv_unified: Option<bool>,
    #[serde(default)]
    pub cache_type_k: Option<String>,
    #[serde(default)]
    pub cache_type_v: Option<String>,
}

fn apply_model_body(
    body: ModelBody,
    existing: Option<tama_core::config::ModelConfig>,
) -> tama_core::config::ModelConfig {
    let base = existing.unwrap_or_else(|| tama_core::config::ModelConfig {
        gpu_variant: None,
        backend: String::new(),
        args: vec![],
        sampling: None,
        model: None,
        quant: None,
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        kv_unified: true,
        cache_type_k: None,
        cache_type_v: None,
        hf_format: None,
        hf_base_model: None,
        hf_pipeline_tag: None,
        hf_total_params: None,
        hf_active_params: None,
        hf_architecture_type: None,
        hf_context_length: None,
        hf_num_layers: None,
        hf_last_modified: None,
        db_id: None,
        spec_decoding: Default::default(),
    });

    // Handle sampling from body
    let sampling = body.sampling;

    tama_core::config::ModelConfig {
        backend: body.backend,
        gpu_variant: body.gpu_variant.or(base.gpu_variant),
        model: body.model.or(base.model),
        quant: body.quant.or(base.quant),
        mmproj: body.mmproj.or(base.mmproj),
        args: body.args,
        sampling,
        enabled: body.enabled.unwrap_or(base.enabled),
        context_length: body.context_length,
        num_parallel: body.num_parallel.or(base.num_parallel),
        port: body.port.or(base.port),
        health_check: base.health_check,
        profile: None,
        api_name: body.api_name.or(base.api_name),
        gpu_layers: body.gpu_layers.or(base.gpu_layers),
        modalities: body.modalities.or(base.modalities),
        display_name: body.display_name.or(base.display_name),
        // Preserve server-side `size_bytes` on update: the UI exposes the field
        // read-only and callers must not be able to rewrite it via the API. The
        // authoritative value comes from the download pipeline
        // (`std::fs::metadata` after pull + the HF blob metadata that later
        // populates `model_files.size_bytes` during verify/refresh). If no
        // prior entry exists, accept the client's value to avoid regressing
        // freshly-created entries that don't yet have a stored size.
        quants: body
            .quants
            .unwrap_or_else(|| base.quants.clone())
            .into_iter()
            .map(|(k, v)| {
                let preserved_size = base
                    .quants
                    .get(&k)
                    .and_then(|existing| existing.size_bytes)
                    .or(v.size_bytes);
                (
                    k,
                    tama_core::config::QuantEntry {
                        file: v.file,
                        kind: v.kind,
                        size_bytes: preserved_size,
                        context_length: v.context_length,
                    },
                )
            })
            .collect(),
        kv_unified: body.kv_unified.unwrap_or(base.kv_unified),
        cache_type_k: body
            .cache_type_k
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "__custom"),
        cache_type_v: body
            .cache_type_v
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && s != "__custom"),
        hf_format: base.hf_format,
        hf_base_model: base.hf_base_model,
        hf_pipeline_tag: base.hf_pipeline_tag,
        hf_total_params: base.hf_total_params,
        hf_active_params: base.hf_active_params,
        hf_architecture_type: base.hf_architecture_type,
        hf_context_length: base.hf_context_length,
        hf_num_layers: base.hf_num_layers,
        hf_last_modified: base.hf_last_modified,
        db_id: base.db_id,
        spec_decoding: base.spec_decoding,
    }
}

// ── Validation helpers ──────────────────────────────────────────────────────

/// Validate that a string is a valid repo_id: non-empty, only alphanumeric, dots, underscores, hyphens, slashes.
fn is_valid_repo_id(input: &str) -> bool {
    if input.is_empty() {
        return false;
    }
    for ch in input.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' | '/' => continue,
            _ => return false,
        }
    }
    true
}

/// Validate ModelBody field lengths. Returns an error message string if invalid.
fn validate_model_body(body: &ModelBody) -> Result<(), String> {
    if body.backend.is_empty() {
        return Err("backend cannot be empty".to_string());
    }
    if body.backend.len() > MAX_BACKEND {
        return Err(format!("backend must be at most {MAX_BACKEND} characters"));
    }
    if let Some(ref model) = body.model {
        if model.is_empty() {
            return Err("model cannot be empty".to_string());
        }
        if model.len() > MAX_MODEL {
            return Err(format!("model must be at most {MAX_MODEL} characters"));
        }
    }
    if let Some(ref quant) = body.quant {
        if !quant.is_empty() && quant.len() > MAX_QUANT {
            return Err(format!("quant must be at most {MAX_QUANT} characters"));
        }
    }
    if let Some(ref mmproj) = body.mmproj {
        if !mmproj.is_empty() && mmproj.len() > MAX_MMPROJ {
            return Err(format!("mmproj must be at most {MAX_MMPROJ} characters"));
        }
    }
    if let Some(ref api_name) = body.api_name {
        if !api_name.is_empty() && api_name.len() > MAX_API_NAME {
            return Err(format!(
                "api_name must be at most {MAX_API_NAME} characters"
            ));
        }
    }
    if let Some(ref display_name) = body.display_name {
        if !display_name.is_empty() && display_name.len() > MAX_DISPLAY_NAME {
            return Err(format!(
                "display_name must be at most {MAX_DISPLAY_NAME} characters"
            ));
        }
    }
    if let Some(ref cache_type_k) = body.cache_type_k {
        let trimmed = cache_type_k.trim();
        if trimmed == "__custom" {
            return Err("cache_type_k cannot be the sentinel value __custom".to_string());
        }
        if !trimmed.is_empty() && trimmed.len() > MAX_CACHE_TYPE {
            return Err(format!(
                "cache_type_k must be at most {MAX_CACHE_TYPE} characters"
            ));
        }
    }
    if let Some(ref cache_type_v) = body.cache_type_v {
        let trimmed = cache_type_v.trim();
        if trimmed == "__custom" {
            return Err("cache_type_v cannot be the sentinel value __custom".to_string());
        }
        if !trimmed.is_empty() && trimmed.len() > MAX_CACHE_TYPE {
            return Err(format!(
                "cache_type_v must be at most {MAX_CACHE_TYPE} characters"
            ));
        }
    }
    Ok(())
}

// ── Sub-modules ─────────────────────────────────────────────────────────────

pub mod create;
pub mod delete;
pub mod rename;
pub mod update;

// ── Re-exports ──────────────────────────────────────────────────────────────

pub use create::create_model;
pub use delete::{delete_model, delete_quant};
pub use rename::rename_model;
pub use update::update_model;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tama_core::config::{ModelConfig, QuantEntry, QuantKind};

    fn body_with_quants(quants: BTreeMap<String, QuantEntry>) -> ModelBody {
        ModelBody {
            backend: "llama".to_string(),
            gpu_variant: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: Some(true),
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: Some(quants),
            modalities: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        }
    }

    fn existing_with_size(name: &str, file: &str, size: Option<u64>) -> ModelConfig {
        let mut quants = BTreeMap::new();
        quants.insert(
            name.to_string(),
            QuantEntry {
                file: file.to_string(),
                kind: QuantKind::Model,
                size_bytes: size,
                context_length: Some(4096),
            },
        );
        ModelConfig {
            backend: "llama".into(),
            gpu_variant: None,
            args: vec![],
            sampling: None,
            model: Some("org/repo".into()),
            quant: Some("Q4_K_M".into()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            num_parallel: None,
            profile: None,
            api_name: None,
            gpu_layers: None,
            quants,
            modalities: None,
            display_name: None,
            kv_unified: false,
            cache_type_k: None,
            cache_type_v: None,
            hf_format: None,
            hf_base_model: None,
            hf_pipeline_tag: None,
            hf_total_params: None,
            hf_active_params: None,
            hf_architecture_type: None,
            hf_context_length: None,
            hf_num_layers: None,
            hf_last_modified: None,
            db_id: None,
            spec_decoding: Default::default(),
        }
    }

    /// When an existing entry has a stored `size_bytes`, a PUT that tries to
    /// change it must be silently ignored — the server-side value wins.
    #[test]
    fn apply_model_body_preserves_existing_size_bytes() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(1_234_567));

        let mut attacker_quants = BTreeMap::new();
        attacker_quants.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(42), // malicious / stale
                context_length: Some(8192),
            },
        );

        let result = apply_model_body(body_with_quants(attacker_quants), Some(existing));
        let q = result.quants.get("Q4_K_M").unwrap();
        assert_eq!(
            q.size_bytes,
            Some(1_234_567),
            "existing size_bytes must be preserved against client override"
        );
        assert_eq!(q.context_length, Some(8192));
    }

    /// When an existing entry has no stored size, we still accept the client
    /// value to avoid regressing fresh creates that haven't been verified yet.
    #[test]
    fn apply_model_body_accepts_client_size_when_none_stored() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", None);

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(9_999),
                context_length: Some(4096),
            },
        );

        let result = apply_model_body(body_with_quants(incoming), Some(existing));
        assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(9_999));
    }

    /// A brand-new model (no existing config) still honours whatever size the
    /// client supplies, so create flows aren't broken.
    #[test]
    fn apply_model_body_accepts_client_size_for_new_model() {
        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(5_000),
                context_length: None,
            },
        );

        let result = apply_model_body(body_with_quants(incoming), None);
        assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(5_000));
    }

    /// A new quant key (not in the existing config) on an existing model still
    /// accepts the client value — preservation is per-key.
    #[test]
    fn apply_model_body_accepts_client_size_for_new_quant_key() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(1_000));

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(7),
                context_length: None,
            },
        );
        incoming.insert(
            "Q8_0".to_string(),
            QuantEntry {
                file: "Model-Q8_0.gguf".to_string(),
                kind: QuantKind::Model,
                size_bytes: Some(2_000),
                context_length: None,
            },
        );

        let result = apply_model_body(body_with_quants(incoming), Some(existing));
        assert_eq!(result.quants.get("Q4_K_M").unwrap().size_bytes, Some(1_000));
        assert_eq!(result.quants.get("Q8_0").unwrap().size_bytes, Some(2_000));
    }

    // ── apply_model_body additional tests ─────────────────────────────────

    #[test]
    fn test_apply_model_body_preserves_existing_size() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", Some(10_000));

        let mut incoming = BTreeMap::new();
        incoming.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "Model-Q4_K_M-new.gguf".to_string(), // different file
                kind: QuantKind::Model,
                size_bytes: Some(5_000), // client sends smaller size
                context_length: None,
            },
        );

        let result = apply_model_body(body_with_quants(incoming), Some(existing));
        // Existing size_bytes should be preserved (server-side authoritative)
        assert_eq!(
            result.quants.get("Q4_K_M").unwrap().size_bytes,
            Some(10_000)
        );
    }

    #[test]
    fn test_apply_model_body_enabled_override() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: Some(false),
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert!(!result.enabled);
    }

    #[test]
    fn test_apply_model_body_enabled_default() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None, // Not specified
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        // Default enabled is true
        assert!(result.enabled);
    }

    #[test]
    fn test_apply_model_body_with_api_name() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: Some("my-api-name".to_string()),
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.api_name, Some("my-api-name".to_string()));
    }

    #[test]
    fn test_apply_model_body_with_gpu_layers() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: Some(32),
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.gpu_layers, Some(32));
    }

    #[test]
    fn test_apply_model_body_with_display_name() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: Some("My Model".to_string()),
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.display_name, Some("My Model".to_string()));
    }

    #[test]
    fn test_apply_model_body_context_length() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: Some(8192),
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.context_length, Some(8192));
    }

    /// Verify that num_parallel flows from body through to ModelConfig.
    #[test]
    fn test_apply_model_body_num_parallel_passthrough() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            num_parallel: Some(4),
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.num_parallel, Some(4));
    }

    #[test]
    fn test_apply_model_body_num_parallel_default() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            num_parallel: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.num_parallel, None);
    }

    #[test]
    fn test_apply_model_body_empty_quants() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: Some(BTreeMap::new()), // empty map
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert!(result.quants.is_empty());
    }

    /// When an existing model has `kv_unified: false` and the body omits the
    /// field, the existing value must be preserved (not overwritten to true).
    #[test]
    fn test_apply_model_body_kv_unified_passthrough() {
        let existing = existing_with_size("Q4_K_M", "Model-Q4_K_M.gguf", None);
        assert!(!existing.kv_unified, "helper must create kv_unified=false");

        let body = ModelBody {
            backend: "llama".to_string(),
            gpu_variant: None,
            model: Some("org/repo".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None, // omitted — should preserve existing
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, Some(existing));
        assert!(
            !result.kv_unified,
            "existing kv_unified=false must be preserved when body omits the field"
        );
    }

    /// When creating a new model (no existing config) and the body omits
    /// `kv_unified`, the result must default to `true`.
    #[test]
    fn test_apply_model_body_kv_unified_default_true_for_new() {
        let body = ModelBody {
            backend: "llama".to_string(),
            gpu_variant: None,
            model: Some("org/repo".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None, // omitted — should default to true
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert!(
            result.kv_unified,
            "new model must default kv_unified to true when body omits the field"
        );
    }

    /// Verify that cache_type_k and cache_type_v flow from body through to ModelConfig.
    #[test]
    fn test_apply_model_body_cache_type_passthrough() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: Some("q4_0".to_string()),
            cache_type_v: Some("q8_0".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.cache_type_k, Some("q4_0".to_string()));
        assert_eq!(result.cache_type_v, Some("q8_0".to_string()));
    }

    /// cache_type_k that exceeds MAX_CACHE_TYPE must be rejected.
    #[test]
    fn test_validate_cache_type_k_too_long() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None,
            cache_type_k: Some("a".repeat(MAX_CACHE_TYPE + 1)),
            cache_type_v: None,
        };
        let result = validate_model_body(&body);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cache_type_k"));
    }

    /// cache_type_v that exceeds MAX_CACHE_TYPE must be rejected.
    #[test]
    fn test_validate_cache_type_v_too_long() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: Some("a".repeat(MAX_CACHE_TYPE + 1)),
        };
        let result = validate_model_body(&body);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cache_type_v"));
    }

    /// cache_type_k/v at exactly MAX_CACHE_TYPE must pass.
    #[test]
    fn test_validate_cache_type_at_limit() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            display_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            kv_unified: None,
            cache_type_k: Some("a".repeat(MAX_CACHE_TYPE)),
            cache_type_v: Some("b".repeat(MAX_CACHE_TYPE)),
        };
        assert!(validate_model_body(&body).is_ok());
    }

    /// When cache_type_k/v are omitted in the body, they should be None.
    #[test]
    fn test_apply_model_body_cache_type_defaults_none() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: None,
            cache_type_v: None,
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.cache_type_k, None);
        assert_eq!(result.cache_type_v, None);
    }

    /// Whitespace-only cache_type_k/v must be normalized to None.
    #[test]
    fn test_apply_model_body_cache_type_whitespace_only_becomes_none() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: Some("   ".to_string()),
            cache_type_v: Some("\t\n".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(
            result.cache_type_k, None,
            "whitespace-only cache_type_k must become None"
        );
        assert_eq!(
            result.cache_type_v, None,
            "whitespace-only cache_type_v must become None"
        );
    }

    /// cache_type_k/v with leading/trailing whitespace must be trimmed.
    #[test]
    fn test_apply_model_body_cache_type_trims_whitespace() {
        let body = ModelBody {
            backend: "llama-cpp".to_string(),
            gpu_variant: None,
            model: Some("model.gguf".to_string()),
            quant: None,
            mmproj: None,
            args: vec![],
            sampling: None,
            enabled: None,
            context_length: None,
            num_parallel: None,
            port: None,
            api_name: None,
            gpu_layers: None,
            quants: None,
            modalities: None,
            display_name: None,
            kv_unified: None,
            cache_type_k: Some("  q4_0  ".to_string()),
            cache_type_v: Some(" q8_0 ".to_string()),
        };

        let result = apply_model_body(body, None);
        assert_eq!(result.cache_type_k, Some("q4_0".to_string()));
        assert_eq!(result.cache_type_v, Some("q8_0".to_string()));
    }
}
