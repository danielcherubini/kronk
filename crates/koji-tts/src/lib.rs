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
