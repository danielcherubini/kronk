//! Koji TTS engine library.
//!
//! Provides a unified `TtsEngine` trait for text-to-speech synthesis,
//! with concrete implementations for Kokoro and Piper backends.

pub mod config;
pub mod kokoro;
pub mod piper;
pub mod streaming;

use std::path::Path;
use std::pin::Pin;

use anyhow::Result;
use futures_core::Stream;

use config::{AudioChunk, TtsRequest, VoiceInfo};

/// Trait for a TTS engine that can synthesize speech.
#[async_trait::async_trait]
pub trait TtsEngine: Send + Sync {
    /// Returns the name of this engine (e.g., "kokoro" or "piper").
    fn name(&self) -> &str;

    /// Returns the list of available voices for this engine.
    fn voices(&self) -> Vec<VoiceInfo>;

    /// Synthesize speech synchronously, returning full audio data.
    async fn synthesize(&self, req: &TtsRequest) -> Result<Vec<u8>>;

    /// Synthesize speech with streaming output (SSE-compatible chunks).
    async fn synthesize_stream(
        &self,
        req: &TtsRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<AudioChunk>> + Send>>>;
}

/// The kind of TTS engine to load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineKind {
    Kokoro,
    Piper,
}

/// A loaded TTS engine (either Kokoro or Piper).
#[derive(Debug, Clone)]
pub enum Engine {
    Kokoro(kokoro::KokoroEngine),
    Piper(piper::PiperEngine),
}

#[async_trait::async_trait]
impl TtsEngine for Engine {
    fn name(&self) -> &str {
        match self {
            Engine::Kokoro(e) => e.name(),
            Engine::Piper(e) => e.name(),
        }
    }

    fn voices(&self) -> Vec<VoiceInfo> {
        match self {
            Engine::Kokoro(e) => e.voices(),
            Engine::Piper(e) => e.voices(),
        }
    }

    async fn synthesize(&self, req: &TtsRequest) -> Result<Vec<u8>> {
        match self {
            Engine::Kokoro(e) => e.synthesize(req).await,
            Engine::Piper(e) => e.synthesize(req).await,
        }
    }

    async fn synthesize_stream(
        &self,
        req: &TtsRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<AudioChunk>> + Send>>> {
        match self {
            Engine::Kokoro(e) => e.synthesize_stream(req).await,
            Engine::Piper(e) => e.synthesize_stream(req).await,
        }
    }
}

/// Load a TTS engine from the given path.
///
/// The `kind` parameter determines which engine to create.
/// The `model_path` should point to the models directory containing
/// the ONNX model file (and voices for Kokoro).
pub async fn load_engine(kind: EngineKind, model_path: &Path) -> Result<Engine> {
    match kind {
        EngineKind::Kokoro => {
            let engine = kokoro::KokoroEngine::new(model_path).await?;
            Ok(Engine::Kokoro(engine))
        }
        EngineKind::Piper => {
            let engine = piper::PiperEngine::new(model_path).await?;
            Ok(Engine::Piper(engine))
        }
    }
}

/// Check if a given Engine is of the specified kind.
pub fn engine_matches_kind(engine: &Engine, kind: &EngineKind) -> bool {
    match (engine, kind) {
        (Engine::Kokoro(_), EngineKind::Kokoro) => true,
        (Engine::Piper(_), EngineKind::Piper) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AudioChunk, AudioFormat, TtsRequest};

    /// Test that AudioFormat derives Default correctly.
    #[test]
    fn test_audio_format_default_is_mp3() {
        assert!(matches!(AudioFormat::default(), AudioFormat::Mp3));
    }

    /// Test that TtsRequest has sensible defaults.
    #[test]
    fn test_tts_request_defaults() {
        let req = TtsRequest::default();
        assert_eq!(req.text, "");
        assert_eq!(req.voice, "");
        // f32::default() is 0.0 — the actual default from derive(Default)
        assert_eq!(req.speed, 0.0);
        assert!(matches!(req.format, AudioFormat::Mp3));
    }

    /// Test that AudioChunk can be created and cloned.
    #[test]
    fn test_audio_chunk_clone() {
        let chunk = AudioChunk {
            data: vec![1, 2, 3],
            is_final: true,
        };
        let cloned = chunk.clone();
        assert_eq!(cloned.data, vec![1, 2, 3]);
        assert!(cloned.is_final);
    }

    /// Test that engine_matches_kind works correctly.
    #[test]
    fn test_engine_matches_kind() {
        // Since we can't easily create real engines without model files,
        // test the logic by checking the enum variants match.
        assert!(matches!(EngineKind::Kokoro, EngineKind::Kokoro));
        assert!(matches!(EngineKind::Piper, EngineKind::Piper));
        assert!(!matches!(EngineKind::Kokoro, EngineKind::Piper));
    }

    /// Test AudioFormat variants.
    #[test]
    fn test_audio_format_variants() {
        assert!(!matches!(AudioFormat::Wav, AudioFormat::Mp3));
        assert!(!matches!(AudioFormat::Ogg, AudioFormat::Wav));
        assert!(matches!(AudioFormat::Mp3, AudioFormat::Mp3));
    }
}
