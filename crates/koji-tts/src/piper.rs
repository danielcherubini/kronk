//! Piper TTS engine wrapper.
//!
//! Wraps the `piper-rs` crate for text-to-speech synthesis.

use std::path::Path;
use std::pin::Pin;

use anyhow::{anyhow, Result};
use futures_core::Stream;

use crate::config::{AudioChunk, TtsRequest, VoiceInfo};
use crate::TtsEngine;

/// A loaded Piper TTS engine.
#[derive(Debug, Clone)]
pub struct PiperEngine {
    /// The ONNX model path for reference.
    #[allow(dead_code)]
    model_path: std::path::PathBuf,
    /// Voice metadata.
    voice: VoiceInfo,
}

impl PiperEngine {
    /// Create a new PiperEngine from a models directory.
    pub async fn new(model_path: &Path) -> Result<Self> {
        let model_file = model_path.join("piper.onnx");

        if !model_file.exists() {
            return Err(anyhow!(
                "Piper model file not found at {}",
                model_file.display()
            ));
        }

        // Try to load voice config from JSON if available
        let config_file = model_path.join("piper.json");
        let voice_name = if config_file.exists() {
            // Could parse the JSON for the actual voice name
            "en_US-lessac-medium".to_string()
        } else {
            "unknown".to_string()
        };

        Ok(Self {
            model_path: model_path.to_path_buf(),
            voice: VoiceInfo {
                id: voice_name.clone(),
                name: format!("Piper - {}", voice_name),
                language: "en".to_string(),
                gender: Some("female".to_string()),
            },
        })
    }
}

#[async_trait::async_trait]
impl TtsEngine for PiperEngine {
    fn name(&self) -> &str {
        "piper"
    }

    fn voices(&self) -> Vec<VoiceInfo> {
        vec![self.voice.clone()]
    }

    async fn synthesize(&self, req: &TtsRequest) -> Result<Vec<u8>> {
        // NOTE: piper-rs API may differ. This is a placeholder that
        // needs to be adapted to the actual crate API.
        //
        // The real implementation will call piper-rs's synthesize function
        // with the text and voice settings.

        if !self.model_path.exists() {
            return Err(anyhow!("Piper model file not found"));
        }

        // Placeholder: return empty audio for now — actual synthesis
        // requires adapting to piper-rs's real API surface.
        // TODO: Integrate with actual piper-rs synthesize call
        let _ = req;
        Ok(vec![])
    }

    async fn synthesize_stream(
        &self,
        req: &TtsRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<AudioChunk>> + Send>>> {
        // For non-streaming engines like Piper, return the full result as one chunk.
        let audio = self.synthesize(req).await?;
        let stream = async_stream::stream! {
            yield Ok(AudioChunk {
                data: audio,
                is_final: true,
            });
        };
        Ok(Box::pin(stream))
    }
}
