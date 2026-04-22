//! TTS (Text-to-Speech) API handlers.
//!
//! Implements OpenAI-compatible `/v1/audio/*` endpoints for speech synthesis.
//! The TTS backend runs as a subprocess (Kokoro-FastAPI uvicorn server).

use crate::proxy::ProxyState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use serde::Deserialize;
use std::sync::Arc;

/// Get the backend URL for a TTS backend from the models map.
///
/// Returns `Ok(Some(url))` if the backend is loaded and has a URL,
/// `Ok(None)` if the backend exists but has no URL (starting state)
/// or is not yet in the map.
async fn get_backend_url(state: &ProxyState, backend_name: &str) -> anyhow::Result<Option<String>> {
    let models = state.models.read().await;
    Ok(models
        .get(backend_name)
        .and_then(|ms| ms.backend_url())
        .map(|u| u.to_string()))
}

/// Request body for speech synthesis.
#[derive(Debug, Deserialize)]
pub struct AudioRequest {
    /// Model/engine name (e.g., "kokoro", "tts_kokoro").
    pub model: String,
    /// Text to synthesize.
    pub input: String,
    /// Voice ID to use.
    #[serde(default)]
    pub voice: Option<String>,
    /// Output format: "mp3", "wav", or "ogg". Defaults to "mp3".
    #[serde(default = "default_response_format")]
    pub response_format: String,
    /// Whether to stream the output.
    #[serde(default)]
    pub stream: bool,
    /// Speech speed (0.5 = half speed, 2.0 = double speed). Defaults to 1.0.
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_response_format() -> String {
    "mp3".to_string()
}

fn default_speed() -> f32 {
    1.0
}

/// Ensure a TTS backend is loaded and return its server URL.
async fn ensure_tts_server(state: &ProxyState, model_name: &str) -> anyhow::Result<String> {
    // Resolve backend name
    let backend_name = match model_name.to_lowercase().as_str() {
        "kokoro" | "tts_kokoro" => "tts_kokoro",
        _ => "tts_kokoro", // default to kokoro
    };

    // Check if already loaded and get the actual URL from ModelState
    if let Some(url) = get_backend_url(state, backend_name).await? {
        return Ok(url);
    }

    // Not loaded — try to load it
    state.load_tts_backend(backend_name).await?;

    // After loading, get the server URL from models map
    get_backend_url(state, backend_name)
        .await?
        .ok_or_else(|| anyhow::anyhow!("TTS backend '{}' loaded but URL not set", backend_name))
}

/// GET /v1/audio/voices - List available voices.
pub async fn handle_audio_voices(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Try to lazy-load the default TTS backend (Kokoro) if not already loaded,
    // and get its actual URL from ModelState
    let server_url = match ensure_tts_server(&state, "kokoro").await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": err_msg,
                            "type": "NotFoundError"
                        }
                    })),
                )
                    .into_response();
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to load TTS backend: {}", e),
                        "type": "ServerError"
                    }
                })),
            )
                .into_response();
        }
    };

    let url = format!("{}/v1/audio/voices", server_url);
    match state.client.get(&url).send().await {
        Ok(response) => {
            let body = match response.text().await {
                Ok(text) => text,
                Err(e) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("Failed to read backend response: {}", e),
                                "type": "ServerError"
                            }
                        })),
                    )
                        .into_response();
                }
            };

            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(parsed) => Json(parsed).into_response(),
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Backend returned invalid JSON: {}", e),
                            "type": "ServerError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to reach TTS backend: {}", e),
                    "type": "ServerError"
                }
            })),
        )
            .into_response(),
    }
}

/// GET /v1/audio/models - List available audio models.
pub async fn handle_audio_models(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Try to lazy-load the default TTS backend (Kokoro) if not already loaded
    let server_url = match ensure_tts_server(&state, "kokoro").await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                // No backend installed — return static list.
                // NOTE: This only lists kokoro. When other TTS engines are supported,
                // this should be expanded or removed in favor of the backend's model list.
                let models = vec![serde_json::json!({
                    "id": "kokoro",
                    "object": "model",
                    "created": 0,
                    "owned_by": "kokoro",
                    "ready": false
                })];
                return Json(serde_json::json!({"object": "list", "data": models})).into_response();
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to load TTS backend: {}", e),
                        "type": "ServerError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Forward to the backend's /v1/audio/models endpoint
    let url = format!("{}/v1/audio/models", server_url);
    match state.client.get(&url).send().await {
        Ok(response) => {
            let body = match response.text().await {
                Ok(text) => text,
                Err(e) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(serde_json::json!({
                            "error": {
                                "message": format!("Failed to read backend response: {}", e),
                                "type": "ServerError"
                            }
                        })),
                    )
                        .into_response();
                }
            };

            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(parsed) => Json(parsed).into_response(),
                Err(e) => (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({
                        "error": {
                            "message": format!("Backend returned invalid JSON: {}", e),
                            "type": "ServerError"
                        }
                    })),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to reach TTS backend: {}", e),
                    "type": "ServerError"
                }
            })),
        )
            .into_response(),
    }
}

/// POST /v1/audio/speech - Synthesize speech (non-streaming).
pub async fn handle_audio_speech(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<AudioRequest>,
) -> Response {
    let server_url = match ensure_tts_server(&state, &req.model).await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": err_msg,
                            "type": "NotFoundError"
                        }
                    })),
                )
                    .into_response();
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to load TTS backend: {}", e),
                        "type": "ServerError"
                    }
                })),
            )
                .into_response();
        }
    };

    // Build the request body for Kokoro-FastAPI (OpenAI-compatible format)
    let voice = req.voice.unwrap_or_default();
    let model_name =
        if req.model.to_lowercase() == "kokoro" || req.model.to_lowercase() == "tts_kokoro" {
            "kokoro"
        } else {
            &req.model
        };

    let speech_req = serde_json::json!({
        "model": model_name,
        "input": req.input,
        "voice": voice,
        "response_format": req.response_format.to_lowercase(),
        "speed": req.speed.clamp(0.5, 2.0),
    });

    let url = format!("{}/v1/audio/speech", server_url);
    match state.client.post(&url).json(&speech_req).send().await {
        Ok(response) => {
            let status = response.status();
            let content_type = content_type_for_format(&req.response_format);
            let bytes = response.bytes().await.unwrap_or_default();
            Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
                })
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to reach TTS backend: {}", e),
                    "type": "ServerError"
                }
            })),
        )
            .into_response(),
    }
}

/// POST /v1/audio/speech/stream - Synthesize speech (streaming via SSE).
pub async fn handle_audio_stream(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<AudioRequest>,
) -> Response {
    let server_url = match ensure_tts_server(&state, &req.model).await {
        Ok(url) => url,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("not installed")
                || err_msg.contains("config directory")
                || err_msg.contains("backend registry")
            {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {
                            "message": err_msg,
                            "type": "NotFoundError"
                        }
                    })),
                )
                    .into_response();
            }
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("Failed to load TTS backend: {}", e),
                        "type": "ServerError"
                    }
                })),
            )
                .into_response();
        }
    };

    let voice = req.voice.unwrap_or_default();
    let model_name =
        if req.model.to_lowercase() == "kokoro" || req.model.to_lowercase() == "tts_kokoro" {
            "kokoro"
        } else {
            &req.model
        };

    let speech_req = serde_json::json!({
        "model": model_name,
        "input": req.input,
        "voice": voice,
        "response_format": req.response_format.to_lowercase(),
        "speed": req.speed.clamp(0.5, 2.0),
        "stream": true,
    });

    let url = format!("{}/v1/audio/speech", server_url);
    match state.client.post(&url).json(&speech_req).send().await {
        Ok(response) => {
            let status = response.status();
            let content_type = content_type_for_format(&req.response_format);
            // Forward raw binary audio stream as-is (no base64 encoding)
            let body = axum::body::Body::from_stream(response.bytes_stream());
            Response::builder()
                .status(status)
                .header("Content-Type", content_type)
                .body(body)
                .unwrap_or_else(|_| {
                    (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
                })
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Failed to reach TTS backend: {}", e),
                    "type": "ServerError"
                }
            })),
        )
            .into_response(),
    }
}

fn content_type_for_format(format: &str) -> &'static str {
    match format.to_lowercase().as_str() {
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        _ => "audio/mpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that content_type_for_format returns correct MIME types.
    #[test]
    fn test_content_type_for_format_mp3() {
        assert_eq!(content_type_for_format("mp3"), "audio/mpeg");
    }

    #[test]
    fn test_content_type_for_format_wav() {
        assert_eq!(content_type_for_format("wav"), "audio/wav");
    }

    #[test]
    fn test_content_type_for_format_ogg() {
        assert_eq!(content_type_for_format("ogg"), "audio/ogg");
    }

    /// Test that content_type_for_format handles edge cases.
    #[test]
    fn test_content_type_edge_cases() {
        // Case insensitive
        assert_eq!(content_type_for_format("MP3"), "audio/mpeg");
        assert_eq!(content_type_for_format("WAV"), "audio/wav");
        assert_eq!(content_type_for_format("OGG"), "audio/ogg");
    }

    /// Test that default_response_format returns mp3.
    #[test]
    fn test_default_response_format() {
        assert_eq!(default_response_format(), "mp3");
    }

    /// Test that default_speed returns 1.0.
    #[test]
    fn test_default_speed() {
        assert_eq!(default_speed(), 1.0);
    }
}
