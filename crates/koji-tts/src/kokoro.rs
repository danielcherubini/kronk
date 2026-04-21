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
    /// Available voices discovered from the voices directory.
    voices: Vec<VoiceInfo>,
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
                    if ext == "pt" || ext == "onnx" {
                        if let Some(file_stem) = entry.path().file_stem() {
                            if let Some(voice_id) = file_stem.to_str() {
                                voices.push(VoiceInfo {
                                    id: voice_id.to_string(),
                                    name: format!("Kokoro - {}", voice_id),
                                    language: "en".to_string(),
                                    gender: Some("female".to_string()),
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

        Ok(Self { voices })
    }

    /// Encode f32 audio samples as 16-bit PCM WAV bytes.
    fn encode_wav(&self, samples: &[f32]) -> Vec<u8> {
        let sample_rate: u32 = 24000;
        let num_channels: u16 = 1;
        let bits_per_sample: u16 = 16;

        let sample_data_len = samples.len() * 2; // 16-bit = 2 bytes per sample
        let total_size = 44 + sample_data_len; // WAV header is 44 bytes

        let mut wav = Vec::with_capacity(total_size);

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((total_size - 8) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt chunk (16 bytes)
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size (PCM = 16)
        wav.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat (1 = PCM)
        wav.extend_from_slice(&num_channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        let byte_rate = sample_rate * num_channels as u32 * bits_per_sample as u32 / 8;
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        let block_align = num_channels * bits_per_sample / 8;
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());

        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(sample_data_len as u32).to_le_bytes());

        // Convert f32 [-1.0, 1.0] to i16 PCM
        for &sample in samples {
            let val = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
            wav.extend_from_slice(&val.to_le_bytes());
        }

        wav
    }
}

/// Known voice IDs for Kokoro (fallback when directory scan fails).
pub const KNOWN_VOICE_IDS: &[&str] = &[
    // Female American
    "af_alloy",
    "af_aoede",
    "af_bella",
    "af_heart",
    "af_jessica",
    "af_kore",
    "af_nicole",
    "af_nova",
    "af_river",
    "af_sarah",
    "af_sky",
    // Male American
    "am_adam",
    "am_echo",
    "am_eric",
    "am_fenrir",
    "am_liam",
    "am_michael",
    "am_onyx",
    "am_puck",
    "am_santa",
    // Female British
    "bf_alice",
    "bf_emma",
    "bf_isabella",
    "bf_lily",
    // Male British
    "bm_daniel",
    "bm_fable",
    "bm_george",
    "bm_lewis",
    // Extra voices
    "ef_dora",
    "em_alex",
    "em_santa",
    "ff_siwis",
    "hf_alpha",
    "hf_beta",
    "hm_omega",
    "hm_psi",
    "if_sara",
    "im_nicola",
    "jf_alpha",
    "jf_gongitsune",
    "jf_nezumi",
    "jf_tebukuro",
    "jm_kumo",
    "pf_dora",
    "pm_alex",
    "pm_santa",
    // Japanese
    "zf_xiaobei",
    "zf_xiaoni",
    "zf_xiaoxiao",
    "zf_xiaoyi",
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
        // Initialize kokoro-micro engine (downloads model on first run if needed)
        let mut tts = kokoro_micro::TtsEngine::new()
            .await
            .map_err(|e| anyhow!("Failed to initialize Kokoro engine: {}", e))?;

        // Synthesize audio samples (returns Vec<f32> at 24kHz)
        let voice: Option<&str> = if req.voice.is_empty() {
            None
        } else {
            Some(&req.voice)
        };
        let samples = tts
            .synthesize_with_options(&req.text, voice, req.speed, 1.0, Some("en"))
            .map_err(|e| anyhow!("Kokoro synthesis failed: {}", e))?;

        if samples.is_empty() {
            return Err(anyhow!("Kokoro returned empty audio"));
        }

        // Convert f32 samples to WAV bytes (16-bit PCM, 24kHz mono)
        Ok(self.encode_wav(&samples))
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
