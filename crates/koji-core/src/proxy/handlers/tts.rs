//! TTS (Text-to-Speech) API handlers.
//!
//! Implements OpenAI-compatible `/v1/audio/*` endpoints for speech synthesis.

use crate::backends::BackendRegistry;
use crate::proxy::ProxyState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use base64::Engine;
use futures::StreamExt;
use koji_tts::TtsEngine;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Request body for speech synthesis.
#[derive(Debug, Deserialize)]
pub struct AudioRequest {
    /// Model/engine name (e.g., "kokoro", "piper").
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

/// GET /v1/audio/voices - List available voices.
pub async fn handle_audio_voices(State(state): State<Arc<ProxyState>>) -> impl IntoResponse {
    let tts_engine = state.tts_engine.read().await;
    if let Some(ref eng) = *tts_engine {
        let voices: Vec<VoiceResponse> = eng
            .voices()
            .into_iter()
            .map(|v| VoiceResponse {
                id: v.id,
                name: v.name,
                language: v.language,
                gender: v.gender,
            })
            .collect();
        Json(serde_json::json!({"data": voices})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": "TTS engine not installed. Install a TTS backend first.",
                    "type": "NotFoundError"
                }
            })),
        )
            .into_response()
    }
}

/// POST /v1/audio/speech - Synthesize speech (non-streaming).
pub async fn handle_audio_speech(
    State(state): State<Arc<ProxyState>>,
    Json(req): Json<AudioRequest>,
) -> Response {
    let eng = match load_or_get_engine(&state, &req.model).await {
        Ok(e) => e,
        Err(e) => {
            // Treat "not installed", config errors, and registry errors as 404
            // (TTS not set up). Any other error (model loading failure) returns 500.
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
                        "message": format!("Failed to load TTS engine: {}", e),
                        "type": "ServerError"
                    }
                })),
            )
                .into_response();
        }
    };

    let voice = req.voice.unwrap_or_default();

    let format = match req.response_format.to_lowercase().as_str() {
        "wav" => koji_tts::config::AudioFormat::Wav,
        "ogg" => koji_tts::config::AudioFormat::Ogg,
        _ => koji_tts::config::AudioFormat::Mp3,
    };

    let tts_req = koji_tts::config::TtsRequest {
        text: req.input,
        voice,
        speed: req.speed.clamp(0.5, 2.0),
        format,
    };

    match eng.synthesize(&tts_req).await {
        Ok(audio) => Response::builder()
            .status(StatusCode::OK)
            .header(
                "Content-Type",
                content_type_for_format(&req.response_format),
            )
            .body(axum::body::Body::from(audio))
            .unwrap_or_else(|_| {
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal error").into_response()
            }),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Synthesis failed: {}", e),
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
    let eng = match load_or_get_engine(&state, &req.model).await {
        Ok(e) => e,
        Err(e) => {
            // Treat "not installed", config errors, and registry errors as 404
            // (TTS not set up). Any other error (model loading failure) returns 500.
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
                        "message": format!("Failed to load TTS engine: {}", e),
                        "type": "ServerError"
                    }
                })),
            )
                .into_response();
        }
    };

    let voice = req.voice.unwrap_or_default();

    let format = match req.response_format.to_lowercase().as_str() {
        "wav" => koji_tts::config::AudioFormat::Wav,
        "ogg" => koji_tts::config::AudioFormat::Ogg,
        _ => koji_tts::config::AudioFormat::Mp3,
    };

    let tts_req = koji_tts::config::TtsRequest {
        text: req.input,
        voice,
        speed: req.speed.clamp(0.5, 2.0),
        format,
    };

    match eng.synthesize_stream(&tts_req).await {
        Ok(stream) => {
            use axum::response::sse::Event;
            use axum::response::{IntoResponse, Sse};
            let sse_stream = stream.map(|chunk_result| match chunk_result {
                Ok(chunk) => {
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&chunk.data);
                    if chunk.is_final {
                        Ok::<axum::response::sse::Event, anyhow::Error>(
                            Event::default().event("audio").data(encoded).event("end"),
                        )
                    } else {
                        Ok(Event::default().event("audio").data(encoded))
                    }
                }
                Err(e) => {
                    let encoded =
                        base64::engine::general_purpose::STANDARD.encode(e.to_string().as_bytes());
                    Ok(Event::default().event("error").data(encoded))
                }
            });

            Sse::new(sse_stream).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "message": format!("Streaming failed: {}", e),
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

/// Load or switch the TTS engine based on the requested model/engine name.
///
/// If the correct engine is already loaded, returns it without changes.
/// Otherwise, loads the new engine from the backend registry (replacing any existing TTS engine).
pub async fn load_or_get_engine(
    state: &ProxyState,
    engine_name: &str,
) -> anyhow::Result<koji_tts::Engine> {
    use anyhow::{anyhow, Context};

    // Determine which kind of engine to load
    let kind = match engine_name.to_lowercase().as_str() {
        "kokoro" | "tts_kokoro" => koji_tts::EngineKind::Kokoro,
        "piper" | "tts_piper" => koji_tts::EngineKind::Piper,
        _ => koji_tts::EngineKind::Kokoro, // default to kokoro
    };

    // Check if the correct engine is already loaded
    {
        let current = state.tts_engine.read().await;
        if let Some(ref eng) = *current {
            if koji_tts::engine_matches_kind(eng, &kind) {
                return Ok(eng.clone());
            }
        }
    }

    // Need to load/switch — find installed backend from registry
    let base_dir =
        crate::config::Config::base_dir().with_context(|| "Failed to get config directory")?;
    let db_path = base_dir.join("koji.db");
    let registry =
        BackendRegistry::open(&db_path).with_context(|| "Failed to open backend registry")?;

    let backend_name = match kind {
        koji_tts::EngineKind::Kokoro => "tts_kokoro",
        koji_tts::EngineKind::Piper => "tts_piper",
    };

    let backend = registry
        .get(backend_name)
        .with_context(|| format!("Failed to query backend '{}'", backend_name))?
        .ok_or_else(|| {
            anyhow!(
                "TTS backend '{}' not installed. Run: koji backend add tts_{}",
                backend_name,
                match kind {
                    koji_tts::EngineKind::Kokoro => "kokoro",
                    koji_tts::EngineKind::Piper => "piper",
                }
            )
        })?;

    // Load the engine from the installed model files
    let engine = koji_tts::load_engine(kind, &backend.path)
        .await
        .with_context(|| format!("Failed to load {} engine", backend_name))?;

    // Replace in state (replaces previous TTS engine if any — singleton behavior)
    {
        let mut current = state.tts_engine.write().await;
        *current = Some(engine.clone());
    }

    Ok(engine)
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

    /// Test that audio_speech returns 404 when no engine is loaded.
    #[tokio::test]
    async fn test_audio_speech_returns_404_when_not_loaded() {
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
