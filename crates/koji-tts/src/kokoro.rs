//! Kokoro TTS engine wrapper.
//!
//! Wraps the `kokoro-micro` crate for text-to-speech synthesis.

use std::path::Path;
use std::pin::Pin;

use anyhow::{anyhow, Result};
use futures_core::Stream;

use crate::config::{AudioChunk, TtsRequest, VoiceInfo};
use crate::TtsEngine;

/// A loaded Kokoro TTS engine.
#[derive(Debug, Clone)]
pub struct KokoroEngine {
    /// The underlying kokoro-micro model handle.
    /// Note: kokoro-micro's API may differ — adapt accordingly.
    #[allow(dead_code)]
    model: Option<KokoroModelHandle>,
    /// Available voices discovered from the voices directory.
    voices: Vec<VoiceInfo>,
}

/// Opaque handle to a loaded Kokoro model.
#[derive(Debug, Clone)]
pub struct KokoroModelHandle {
    /// The ONNX model path for reference.
    _model_path: std::path::PathBuf,
}

impl KokoroEngine {
    /// Create a new KokoroEngine from a models directory.
    ///
    /// Scans the `voices/` subdirectory to discover available voices.
    pub async fn new(model_path: &Path) -> Result<Self> {
        let voices_path = model_path.join("voices");
        let mut voices = Vec::new();

        if voices_path.is_dir() {
            let mut entries = tokio::fs::read_dir(&voices_path).await?;
            while let Ok(Some(entry)) = entries.next_entry().await {
                if let Some(ext) = entry.path().extension() {
                    if ext == "onnx" {
                        if let Some(file_stem) = entry.path().file_stem() {
                            if let Some(voice_id) = file_stem.to_str() {
                                voices.push(VoiceInfo {
                                    id: voice_id.to_string(),
                                    name: format!("Kokoro - {}", voice_id),
                                    language: "en".to_string(),
                                    gender: Some("female".to_string()), // Default assumption
                                });
                            }
                        }
                    }
                }
            }
        }

        // If no voices found in directory, use the known default list
        if voices.is_empty() {
            for id in KNOWN_VOICE_IDS {
                voices.push(VoiceInfo {
                    id: id.to_string(),
                    name: format!("Kokoro - {}", id),
                    language: "en".to_string(),
                    gender: Some("female".to_string()),
                });
            }
        }

        // Create the model handle
        let model = KokoroModelHandle {
            _model_path: model_path.join("kokoro-82m.onnx"),
        };

        Ok(Self {
            model: Some(model),
            voices,
        })
    }
}

/// Known voice IDs for Kokoro (fallback when directory scan fails).
pub const KNOWN_VOICE_IDS: &[&str] = &[
    "af_heart",
    "af_bella",
    "af_nicole",
    "af_nova",
    "af_river",
    "af_sarah",
    "af_sky",
    "am_adam",
    "am_michael",
    "bf_emma",
    "bf_isabella",
    "bm_george",
    "bm_lewis",
    "am_sarah",
    "am_santa",
    "af_jessica",
    "bm_daniel",
    "af_scout",
    "am_sky",
    "am_wren",
    "bf_sarah",
    "bm_sage",
    "af_robin",
    "am_sage",
    "bm_scout",
];

#[async_trait::async_trait]
impl TtsEngine for KokoroEngine {
    fn name(&self) -> &str {
        "kokoro"
    }

    fn voices(&self) -> Vec<VoiceInfo> {
        self.voices.clone()
    }

    async fn synthesize(&self, req: &TtsRequest) -> Result<Vec<u8>> {
        // NOTE: kokoro-micro API may differ. This is a placeholder that
        // needs to be adapted to the actual crate API.
        //
        // The real implementation will call kokoro-micro's synthesize function
        // with the text, voice ID, and speed parameters.

        if self.model.is_none() {
            return Err(anyhow!("Kokoro model not loaded"));
        }

        // Placeholder: return empty audio for now — actual synthesis
        // requires adapting to kokoro-micro's real API surface.
        // TODO: Integrate with actual kokoro-micro synthesize call
        let _ = req;
        Ok(vec![])
    }

    async fn synthesize_stream(
        &self,
        req: &TtsRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<AudioChunk>> + Send>>> {
        // For non-streaming engines like Kokoro, return the full result as one chunk.
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
