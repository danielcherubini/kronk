use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use super::types::ModelResponse;
use crate::proxy::ProxyState;

/// Capitalize the first character of a string, preserve the rest unchanged.
pub fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Generate a pretty display name from an HF repo name.
/// e.g., "unsloth/Qwen3.5-35B-A3B-GGUF" -> "Unsloth: Qwen3.5 35B A3B"
/// Strips common file suffixes like "GGUF".
pub fn generate_display_name(hf_repo: &str) -> String {
    let parts: Vec<&str> = hf_repo.split('/').collect();
    let (org, model_name) = if parts.len() >= 2 {
        (parts[0], parts[1])
    } else {
        (hf_repo, hf_repo)
    };

    let model_name_processed = model_name
        .replace(['-', '_'], " ")
        .split_whitespace()
        .filter(|word| !word.eq_ignore_ascii_case("GGUF"))
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ");

    format!("{}: {}", capitalize_first(org), model_name_processed)
}

/// Resolve an incoming `:id` path param to the internal config_key.
///
/// Accepts three forms (in priority order):
/// 1. Integer db_id — looked up against `config.db_id` in the in-memory map.
/// 2. Repo id with a slash (e.g. `Unsloth/Foo-GGUF`) — normalized to the
///    lowercased double-dash config_key (e.g. `unsloth--foo-gguf`).
/// 3. Anything else — returned unchanged, on the assumption it is already a
///    config_key, api_name, or model field that downstream lookups will handle.
///
/// Steps 1 and 2 both honour the case-insensitive repo_id contract established
/// by the `COLLATE NOCASE` migration on `model_configs.repo_id`: the in-memory
/// HashMap is keyed by the lowercased repo_id, so a repo id in any case
/// resolves to the same bucket.
async fn resolve_model_id(state: &ProxyState, raw: &str) -> String {
    if let Ok(id) = raw.parse::<i64>() {
        let configs = state.model_configs.read().await;
        if let Some((key, _)) = configs.iter().find(|(_, c)| c.db_id == Some(id)) {
            return key.clone();
        }
    }
    if raw.contains('/') {
        return raw.to_lowercase().replace('/', "--");
    }
    raw.to_string()
}

/// Handle listing all configured models (Koji management API).
pub async fn handle_koji_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let models = state.build_status_response().await;
    let models_obj = models.get("models").and_then(|v| v.as_object());

    let result: Vec<serde_json::Value> = models_obj
        .into_iter()
        .flat_map(|models_obj| {
            models_obj.iter().filter_map(|(_key, model)| {
                model
                    .as_object()
                    .and_then(|model| serde_json::to_value(model).ok())
            })
        })
        .collect();

    Json(serde_json::json!({
        "models": result
    }))
}

/// Handle getting a single model's state (Koji management API).
pub async fn handle_koji_get_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    // Check if already loaded (by server name or model name)
    let model_state = state.get_model_state(&model_id).await;

    if let Some(ms) = model_state {
        let owned_by = ms.backend();
        let created = match ms.load_time() {
            Some(load_time) => load_time
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::ZERO)
                .as_secs(),
            None => 0,
        };
        return Json(serde_json::json!({
            "id": model_id,
            "object": "model",
            "created": created,
            "owned_by": owned_by,
            "ready": ms.is_ready()
        }))
        .into_response();
    }

    // Check if it's a configured (but not loaded) model
    let model_configs = state.model_configs.read().await;
    let config = state.config.read().await;
    let servers = config.resolve_servers_for_model(&model_configs, &model_id);
    if let Some((config_name, server_cfg, _)) = servers.first() {
        if server_cfg.enabled {
            return Json(serde_json::json!({
                "id": config_name,
                "object": "model",
                "created": 0,
                "owned_by": server_cfg.backend,
                "ready": false
            }))
            .into_response();
        }
    }

    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": {
                "message": "Model not found",
                "type": "NotFoundError"
            }
        })),
    )
        .into_response()
}

/// Handle loading a model (Koji management API).
pub async fn handle_koji_load_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    match state.load_model(&model_id, None).await {
        Ok(server_name) => {
            let model_state = state.get_model_state(&server_name).await;
            let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
            Json(ModelResponse {
                id: model_id,
                loaded,
            })
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to load model: {}", e),
                    "type": "LoadModelError"
                }
            })),
        )
            .into_response(),
    }
}

/// Handle unloading a model (Koji management API).
pub async fn handle_koji_unload_model(
    state: State<Arc<ProxyState>>,
    Path(model_id): Path<String>,
) -> Response {
    let model_id = resolve_model_id(&state, &model_id).await;
    // Get the server name for this model
    let server_name = state.get_available_server_for_model(&model_id).await;

    match server_name {
        Some(server_name) => {
            // Unload the model
            match state.unload_model(&server_name).await {
                Ok(_) => {
                    let model_state = state.get_model_state(&model_id).await;
                    let loaded = model_state.as_ref().is_some_and(|ms| ms.is_ready());
                    Json(ModelResponse {
                        id: model_id,
                        loaded,
                    })
                    .into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Failed to unload model: {}", e),
                            "type": "UnloadModelError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "Model not configured or not loaded",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}

/// Handle listing all enabled models for OpenCode plugin discovery.
/// Returns rich metadata including context limits, modalities, and capabilities.
pub async fn handle_opencode_list_models(state: State<Arc<ProxyState>>) -> Json<serde_json::Value> {
    let model_configs = state.model_configs.read().await;
    let _config = state.config.read().await;

    let mut models: Vec<serde_json::Value> = Vec::new();

    for (id, cfg) in model_configs.iter().filter(|(_, cfg)| cfg.enabled) {
        // Use model field first, fall back to api_name — either is sufficient for HF repo identification.
        let Some(hf_repo) = cfg.model.clone().or(cfg.api_name.clone()) else {
            continue;
        };

        let context_length = if let Some(ctx) = cfg.context_length {
            Some(ctx)
        } else {
            let card = state.get_model_card(id).await;
            card.and_then(|c| {
                let quant_key = cfg.quant.as_deref().unwrap_or_default();
                c.quants
                    .get(quant_key)
                    .and_then(|q| q.context_length)
                    .or(c.model.default_context_length)
            })
        };
        let modalities = cfg.modalities.as_ref().map(|m| {
            serde_json::json!({
                "input": m.input,
                "output": m.output
            })
        });

        // Output limit: 1/8 of context window, floored at 16K and capped at 32K.
        let output_limit = context_length.map(|ctx| (ctx / 8).clamp(16384, 32768));

        // API id is the lowercased HF repo name (includes org prefix).
        // e.g., "unsloth/Qwen3.5-35B-A3B-GGUF" -> "unsloth/qwen3.5-35b-a3b-gguf"
        let api_id = hf_repo.to_lowercase();

        // Generate a pretty display name with org prefix.
        // e.g., "unsloth/Qwen3.5-35B-A3B-GGUF" -> "Unsloth: Qwen3.5 35B A3B"
        // e.g., "mudler/Qwen3.5-35B-A3B-APEX-GGUF" -> "Mudler: Qwen3.5 35B A3B APEX"
        // Strips common file suffixes like "GGUF" since they add no meaning.
        let parts: Vec<&str> = hf_repo.split('/').collect();
        let (org, model_name) = if parts.len() >= 2 {
            (parts[0], parts[1])
        } else {
            (hf_repo.as_str(), hf_repo.as_str())
        };

        let model_name_processed = model_name
            .replace(['-', '_'], " ")
            .split_whitespace()
            .filter(|word| !word.eq_ignore_ascii_case("GGUF"))
            .map(capitalize_first)
            .collect::<Vec<_>>()
            .join(" ");

        let pretty_name = format!("{}: {}", capitalize_first(org), model_name_processed);

        let mut model_json = serde_json::json!({
            "id": api_id,
            "name": pretty_name,
            "model": cfg.model,
            "backend": cfg.backend,
            "context_length": context_length,
            "limit": {
                "context": context_length,
                "output": output_limit,
            },
            "quant": cfg.quant,
            "gpu_layers": cfg.gpu_layers,
        });

        // Only include modalities if explicitly configured.
        if let Some(m) = modalities {
            model_json["modalities"] = m;
        }

        models.push(model_json);
    }

    Json(serde_json::json!({
        "models": models
    }))
}
