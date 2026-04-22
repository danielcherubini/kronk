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
use base64::Engine;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

/// Response for voice listing.
#[derive(Debug, Serialize)]
pub struct VoiceResponse {
    pub id: String,
    pub name: String,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
}

/// Ensure a TTS backend is loaded and return its server URL.
async fn ensure_tts_server(
    state: &ProxyState,
    model_name: &str,
) -> anyhow::Result<String> {
    // Check if already loaded
    if let Some(server) = state.get_tts_server(model_name).await {
        return Ok(format!("http://{}", server));
    }

    // Not loaded — try to load it
    let backend_name = match model_name.to_lowercase().as_str() {
        "kokoro" | "tts_kokoro" => "tts_kokoro",
        _ => "tts_kokoro", // default to kokoro
    };

    state.load_tts_backend(backend_name).await?;

    // After loading, get the server URL from models map
    let models = state.models.read().await;
    if let Some(state) = models.get(backend_name) {
        return Ok(state.backend_url().map(|u| u.to_string()).unwrap_or_default());
    }

    anyhow::bail!("TTS backend '{}' loaded but URL not found", backend_name);
}

/// GET /v1/audio/voices - List available voices.
pub async fn handle_audio_voices(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    // Try to lazy-load the default TTS backend (Kokoro) if not already loaded
    let _ = ensure_tts_server(&state, "kokoro").await;

    // Get the server URL and forward to the backend
    match state.get_tts_server("tts_kokoro").await {
        Some(server) => {
            let url = format!("http://{}/v1/audio/voices", server);
            match state.client.get(&url).send().await {
                Ok(response) => {
                    let body = response.text().await.unwrap_or_default();
                    Json(serde_json::from_str::<serde_json::Value>(&body).unwrap_or_else(|_| serde_json::json!({"data": []}))).into_response()
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
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "TTS backend not installed. Install a TTS backend first.",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response(),
    }
}

/// GET /v1/audio/models - List available audio models.
pub async fn handle_audio_models(State(_state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let models = vec![serde_json::json!({
        "id": "kokoro",
        "object": "model",
        "created": 0,
        "owned_by": "kokoro",
        "ready": true
    })];

    Json(serde_json::json!({"object": "list", "data": models})).into_response()
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
    let model_name = if req.model.to_lowercase() == "kokoro" || req.model.to_lowercase() == "tts_kokoro" {
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
                .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response())
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
    let model_name = if req.model.to_lowercase() == "kokoro" || req.model.to_lowercase() == "tts_kokoro" {
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
            use axum::response::sse::Event;
            use axum::response::{IntoResponse, Sse};

            let stream = response.bytes_stream().map(move |chunk_result| {
                match chunk_result {
                    Ok(chunk) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&chunk);
                        // Simple framing: each SSE event contains one audio chunk
                        Ok::<Event, anyhow::Error>(
                            Event::default().event("audio").data(encoded),
                        )
                    }
                    Err(e) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(e.to_string().as_bytes());
                        Ok(Event::default().event("error").data(encoded))
                    }
                }
            });

            Sse::new(stream).into_response()
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
    use crate::config::Config;
    use crate::proxy::ProxyState;
    use axum::{http::StatusCode, response::IntoResponse};

    fn create_test_state() -> ProxyState {
        let config = Config::default();
        ProxyState::new(config, None)
    }

    #[tokio::test]
    async fn test_audio_voices_returns_404_when_not_loaded() {
        let state = Arc::new(create_test_state());
        let response = handle_audio_voices(State(state)).await;
        let response: axum::http::Response<axum::body::Body> = response.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

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

    /// Test that audio_speech returns 404 when backend is not installed.
    #[tokio::test]
    async fn test_audio_speech_returns_404_when_not_installed() {
        let state = Arc::new(create_test_state());
        let req = AudioRequest {
            model: "kokoro".to_string(),
            input: "Hello world".to_string(),
            voice: None,
            response_format: "mp3".to_string(),
            stream: false,
            speed: 1.0,
        };
        let response = handle_audio_speech(State(state), Json(req)).await;
        // Returns NOT_FOUND because tts_kokoro is not installed in the test env
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
