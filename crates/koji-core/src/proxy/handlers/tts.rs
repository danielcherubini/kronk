//! TTS (Text-to-Speech) API handlers.
//!
//! Implements OpenAI-compatible `/v1/audio/*` endpoints for speech synthesis.

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
}

fn default_response_format() -> String {
    "mp3".to_string()
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
    let tts_engine = state.tts_engine.read().await;
    let eng = match tts_engine.as_ref() {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "message": "TTS engine not installed. Install a TTS backend first.",
                        "type": "NotFoundError"
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
        speed: 1.0,
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
    let tts_engine = state.tts_engine.read().await;
    let eng = match tts_engine.as_ref() {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "message": "TTS engine not installed. Install a TTS backend first.",
                        "type": "NotFoundError"
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
        speed: 1.0,
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
}
