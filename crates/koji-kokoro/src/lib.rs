//! kokoro-micro: A minimal, embeddable TTS engine using the Kokoro model
//!
//! This crate provides a simple API for text-to-speech synthesis using the
//! Kokoro 82M parameter model. Perfect for embedding in other applications!
//!
//! # Example
//! ```no_run
//! use kokoro_micro::TtsEngine;
//!
//! #[tokio::main]
//! async fn main() {
//!     // Initialize with auto-download of model if needed
//!     let mut tts = TtsEngine::new().await.unwrap();
//!
//!     // Generate speech with synthesize_with_options
//!     let audio = tts.synthesize_with_options("Hello world!", None, 1.0, 1.0, Some("en")).unwrap();
//!
//!     // Save to file
//!     tts.save_wav("output.wav", &audio).unwrap();
//! }
//! ```

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use espeak_rs::text_to_phonemes;
use ndarray::{ArrayBase, IxDyn, OwnedRepr};
use ndarray_npy::NpzReader;
use ort::{
    ep,
    session::{builder::GraphOptimizationLevel, Session, SessionInputValue, SessionInputs},
    value::{Tensor, Value},
};

// Debug logging macro - only prints when KOKORO_DEBUG=1
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if std::env::var("KOKORO_DEBUG").map(|v| v == "1").unwrap_or(false) {
            eprintln!($($arg)*);
        }
    };
}

// Constants - Model files stored in GitHub LFS
const MODEL_URL: &str = "https://github.com/8b-is/kokoro-tiny/raw/main/models/0.onnx";
const VOICES_URL: &str = "https://github.com/8b-is/kokoro-tiny/raw/main/models/0.bin";
const SAMPLE_RATE: u32 = 24000; // Kokoro model sample rate
const DEFAULT_VOICE: &str = "af_sky";
const DEFAULT_SPEED: f32 = 1.0; // User-facing normal speed (maps to model 0.65)
const DEFAULT_LANG: &str = "en";
const SPEED_SCALE: f32 = 0.65; // Model speed = user speed * this scale factor
const LONG_TEXT_THRESHOLD: usize = 120;
const MAX_CHARS_PER_CHUNK: usize = 180;
const CHUNK_CROSSFADE_MS: usize = 45;
const MIN_ENGINE_SPEED: f32 = 0.35;
const MAX_ENGINE_SPEED: f32 = 2.2;
const PAD_TOKEN: char = '$'; // Padding token for beginning/end of phonemes

// Fallback audio message - "Excuse me, I lost my voice. Give me time to get it back."
// This is a pre-generated minimal WAV file that can play while downloading
const FALLBACK_MESSAGE: &[u8] = include_bytes!("../assets/fallback.wav");

// Get cache directory for shared model storage - keeping it minimal like Hue wants!
// Always uses $HOME/.cache/k on all platforms (Windows, macOS, Linux)
fn get_cache_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| {
            debug_log!("⚠️  Could not determine HOME directory, using current directory");
            ".".to_string()
        });

    Path::new(&home).join(".cache").join("k")
}

/// Main TTS engine struct
pub struct TtsEngine {
    session: Option<Arc<Mutex<Session>>>,
    voices: HashMap<String, Vec<f32>>,
    vocab: HashMap<char, i64>,
    fallback_mode: bool,
}

/// Global ort environment initializer — called once before any sessions.
static ORT_INIT: OnceLock<bool> = OnceLock::new();

fn init_ort_environment() -> bool {
    *ORT_INIT.get_or_init(|| {
        // Try ROCm first, fall back to CPU if unavailable
        let rocm = ep::ROCm::default();
        debug_log!("🎮 Initializing ort with ROCm GPU execution provider");
        match ort::init()
            .with_name("kokoro-tts")
            .with_execution_providers([rocm.build()])
            .commit()
        {
            true => {
                debug_log!("✅ ROCm initialized successfully");
                true
            }
            false => {
                debug_log!("⚠️  ROCm init failed, falling back to CPU");
                false
            }
        }
    })
}

impl TtsEngine {
    /// Create a new TTS engine, downloading model files if necessary
    ///
    /// Uses `$HOME/.cache/k/` for shared model storage on all platforms:
    /// - Linux/macOS: `$HOME/.cache/k/` (e.g., `/home/user/.cache/k/`)
    /// - Windows: `%USERPROFILE%/.cache/k/` (e.g., `C:\Users\Username\.cache\k\`)
    pub async fn new() -> Result<Self, String> {
        let cache_dir = get_cache_dir();
        let model_path = cache_dir.join("0.onnx");
        let voices_path = cache_dir.join("0.bin");

        Self::with_paths(
            model_path.to_str().unwrap_or("0.onnx"),
            voices_path.to_str().unwrap_or("0.bin"),
        )
        .await
    }

    /// Create a new TTS engine with custom model paths
    pub async fn with_paths(model_path: &str, voices_path: &str) -> Result<Self, String> {
        // Ensure cache directory exists
        if let Some(parent) = Path::new(model_path).parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create cache directory: {}", e))?;
        }

        // Check if we need to download
        let need_download = !Path::new(model_path).exists() || !Path::new(voices_path).exists();

        if need_download {
            debug_log!("🎤 First time setup - downloading voice model...");
            debug_log!("   (This only happens once, files will be cached in ~/.cache/k)");

            // Try to download the files
            let download_success = {
                let mut success = true;

                // Download model if needed
                if !Path::new(model_path).exists() {
                    debug_log!("   📥 Downloading model (310MB)...");
                    if let Err(e) = download_file(MODEL_URL, model_path).await {
                        debug_log!("   ❌ Failed to download model: {}", e);
                        success = false;
                    }
                }

                // Download voices if needed
                if success && !Path::new(voices_path).exists() {
                    debug_log!("   📥 Downloading voices (27MB)...");
                    if let Err(e) = download_file(VOICES_URL, voices_path).await {
                        debug_log!("   ❌ Failed to download voices: {}", e);
                        success = false;
                    }
                }

                if success {
                    debug_log!("   ✅ Voice model downloaded successfully!");
                }

                success
            };

            // If download failed, return fallback engine
            if !download_success {
                debug_log!("\n⚠️  Using fallback mode. The model files are not available at:");
                debug_log!("   - {}", MODEL_URL);
                debug_log!("   - {}", VOICES_URL);
                debug_log!("\n💡 Please manually download the model files to ~/.cache/k/");

                return Ok(Self {
                    session: None,
                    voices: HashMap::new(),
                    vocab: build_vocab(),
                    fallback_mode: true,
                });
            }
        }

        // Initialize ort environment (once globally) with ROCm GPU if available
        init_ort_environment();

        // Load ONNX model
        let model_bytes =
            std::fs::read(model_path).map_err(|e| format!("Failed to read model file: {}", e))?;

        let session = Session::builder()
            .map_err(|e| format!("Failed to create session builder: {}", e))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| format!("Failed to set optimization level: {}", e))?
            .commit_from_memory(&model_bytes)
            .map_err(|e| format!("Failed to load model: {}", e))?;

        // Load voices
        let voices = load_voices(voices_path)?;

        Ok(Self {
            session: Some(Arc::new(Mutex::new(session))),
            voices,
            vocab: build_vocab(),
            fallback_mode: false,
        })
    }

    /// List all available voices
    pub fn voices(&self) -> Vec<String> {
        if self.fallback_mode {
            vec!["fallback".to_string()]
        } else {
            self.voices.keys().cloned().collect()
        }
    }

    /// Synthesize text to speech with full options
    /// Speed: 0.5 = half speed (slower), 1.0 = normal, 2.0 = double speed (faster)
    /// Gain: 0.5 = quieter, 1.0 = normal, 2.0 = twice as loud (with soft clipping)
    pub fn synthesize_with_options(
        &mut self,
        text: &str,
        voice: Option<&str>,
        speed: f32,
        gain: f32,
        lang: Option<&str>,
    ) -> Result<Vec<f32>, String> {
        // If in fallback mode, return the excuse message audio
        if self.fallback_mode {
            debug_log!("🎤 Playing fallback message while downloading voice model...");
            return wav_to_f32(FALLBACK_MESSAGE);
        }

        let session = self
            .session
            .as_ref()
            .ok_or_else(|| "TTS engine not initialized".to_string())?;

        // Map user-facing speed to model speed (user 1.0 = model 0.65)
        let model_speed = speed * SPEED_SCALE;
        let clamped_speed = model_speed.clamp(MIN_ENGINE_SPEED, MAX_ENGINE_SPEED);
        let voice = voice.unwrap_or(DEFAULT_VOICE);

        // Parse voice style (e.g., "af_sky.8+af_bella.2" for mixing)
        let style = self.parse_voice_style(voice)?;

        // Short form: synthesize in one pass for predictable cadence
        if !needs_chunking(text) {
            let mut audio = self.synthesize_segment(session, &style, text, clamped_speed, lang)?;
            if gain != 1.0 {
                audio = amplify_audio(&audio, gain);
            }
            return Ok(audio);
        }

        // Long-form synthesis path - chunk the text while preserving pacing
        let prepared_chunks: Vec<String> = split_text_for_tts(text, MAX_CHARS_PER_CHUNK)
            .into_iter()
            .filter(|chunk| !chunk.trim().is_empty())
            .collect();

        if prepared_chunks.is_empty() {
            return Err("No text provided for synthesis".to_string());
        }

        let chunk_count = prepared_chunks.len();
        debug_log!(
            "📚 Long-form synthesis enabled: {} chars -> {} chunk(s) (≤ {} chars each)",
            text.chars().count(),
            chunk_count,
            MAX_CHARS_PER_CHUNK
        );

        let overlap = chunk_crossfade_samples();
        let mut combined_audio = Vec::new();

        for (idx, chunk) in prepared_chunks.iter().enumerate() {
            debug_log!(
                "   → Chunk {}/{} ({} chars)",
                idx + 1,
                chunk_count,
                chunk.chars().count()
            );

            let chunk_audio =
                self.synthesize_segment(session, &style, chunk, clamped_speed, lang)?;
            append_with_crossfade(&mut combined_audio, &chunk_audio, overlap);
        }

        if combined_audio.is_empty() {
            return Err("Failed to synthesize combined audio".to_string());
        }

        let mut final_audio = combined_audio;
        if gain != 1.0 {
            final_audio = amplify_audio(&final_audio, gain);
        }

        Ok(final_audio)
    }

    fn synthesize_segment(
        &self,
        session: &Arc<Mutex<Session>>,
        style: &[f32],
        text: &str,
        speed: f32,
        lang: Option<&str>,
    ) -> Result<Vec<f32>, String> {
        // Convert text to phonemes
        let phonemes = text_to_phonemes(text, lang.unwrap_or(DEFAULT_LANG), None, true, false)
            .map_err(|e| format!("Failed to convert text to phonemes: {}", e))?;

        // Join phonemes with spaces and add padding tokens at beginning and end
        // Spaces between phonemes create natural pauses for commas and periods
        // Padding tokens are crucial to prevent word dropping at beginning and end
        let mut phonemes_text = phonemes.join(" ");
        // Add multiple padding tokens for better buffering
        phonemes_text.insert_str(0, "$$$");
        phonemes_text.push_str("$$$");

        // Debug output only for long text
        if text.len() > 50 {
            debug_log!("   Text length: {} chars", text.len());
            debug_log!("   Phonemes array: {} entries", phonemes.len());
            debug_log!("   Phoneme text length: {} chars", phonemes_text.len());
        }

        let tokens = self.tokenize(phonemes_text);

        // Run inference with user-specified speed directly
        self.run_inference(session, tokens, style.to_vec(), speed)
    }

    /// Save audio as WAV file
    pub fn save_wav(&self, path: &str, audio: &[f32]) -> Result<(), String> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = hound::WavWriter::create(path, spec)
            .map_err(|e| format!("Failed to create WAV file: {}", e))?;

        for &sample in audio {
            let sample_i16 = (sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
            writer
                .write_sample(sample_i16)
                .map_err(|e| format!("Failed to write sample: {}", e))?;
        }

        writer
            .finalize()
            .map_err(|e| format!("Failed to finalize WAV: {}", e))?;
        Ok(())
    }

    // Private helper methods

    fn parse_voice_style(&self, voice_str: &str) -> Result<Vec<f32>, String> {
        if self.fallback_mode {
            // Return a dummy style vector for fallback mode
            return Ok(vec![0.0; 256]);
        }

        let mut result = vec![0.0; 256];
        let parts: Vec<&str> = voice_str.split('+').collect();

        for part in parts {
            let (voice_name, weight) = if part.contains('.') {
                let pieces: Vec<&str> = part.split('.').collect();
                if pieces.len() != 2 {
                    return Err(format!("Invalid voice format: {}", part));
                }
                let weight = pieces[1]
                    .parse::<f32>()
                    .map_err(|_| format!("Invalid weight: {}", pieces[1]))?;
                (pieces[0], weight / 10.0)
            } else {
                (part, 1.0)
            };

            let voice_style = self
                .voices
                .get(voice_name)
                .ok_or_else(|| format!("Voice not found: {}", voice_name))?;

            for (i, val) in voice_style.iter().enumerate() {
                if i < result.len() {
                    result[i] += val * weight;
                }
            }
        }

        Ok(result)
    }

    fn tokenize(&self, text: String) -> Vec<i64> {
        text.chars()
            .map(|c| *self.vocab.get(&c).unwrap_or(&0))
            .collect()
    }

    fn run_inference(
        &self,
        session: &Arc<Mutex<Session>>,
        tokens: Vec<i64>,
        style: Vec<f32>,
        speed: f32,
    ) -> Result<Vec<f32>, String> {
        let mut session = session
            .lock()
            .map_err(|e| format!("Failed to lock session: {}", e))?;

        let token_count = tokens.len(); // Save count before moving

        // Prepare tokens tensor
        let tokens_array = ndarray::Array2::from_shape_vec((1, tokens.len()), tokens)
            .map_err(|e| format!("Failed to create tokens array: {}", e))?;
        let tokens_tensor = Tensor::from_array(tokens_array)
            .map_err(|e| format!("Failed to create tokens tensor: {}", e))?;

        // Prepare style tensor
        let style_array = ndarray::Array2::from_shape_vec((1, style.len()), style)
            .map_err(|e| format!("Failed to create style array: {}", e))?;
        let style_tensor = Tensor::from_array(style_array)
            .map_err(|e| format!("Failed to create style tensor: {}", e))?;

        // Prepare speed tensor
        let speed_array = ndarray::Array1::from_vec(vec![speed]);
        let speed_tensor = Tensor::from_array(speed_array)
            .map_err(|e| format!("Failed to create speed tensor: {}", e))?;

        // Create inputs
        use std::borrow::Cow;
        let inputs = SessionInputs::from(vec![
            (
                Cow::Borrowed("tokens"),
                SessionInputValue::Owned(Value::from(tokens_tensor)),
            ),
            (
                Cow::Borrowed("style"),
                SessionInputValue::Owned(Value::from(style_tensor)),
            ),
            (
                Cow::Borrowed("speed"),
                SessionInputValue::Owned(Value::from(speed_tensor)),
            ),
        ]);

        // Run inference
        let outputs = session
            .run(inputs)
            .map_err(|e| format!("Failed to run inference: {}", e))?;

        // Extract audio
        let (shape, data) = outputs["audio"]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract audio tensor: {}", e))?;

        // Debug output shape for longer text
        let data_vec = data.to_vec();
        if token_count > 100 {
            debug_log!(
                "   Output audio shape: {:?}, samples: {}",
                shape,
                data_vec.len()
            );
        }

        Ok(data_vec)
    }
}

// Helper functions

// Build proper vocabulary for tokenization (matching original Kokoros)
fn build_vocab() -> HashMap<char, i64> {
    let pad = "$";
    let punctuation = r#";:,.!?¡¿—…"«»"" "#;
    let letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let letters_ipa = "ɑɐɒæɓʙβɔɕçɗɖðʤəɘɚɛɜɝɞɟʄɡɠɢʛɦɧħɥʜɨɪʝɭɬɫɮʟɱɯɰŋɳɲɴøɵɸθœɶʘɹɺɾɻʀʁɽʂʃʈʧʉʊʋⱱʌɣɤʍχʎʏʑʐʒʔʡʕʢǀǁǂǃˈˌːˑʼʴʰʱʲʷˠˤ˞↓↑→↗↘'̩'ᵻ";

    let symbols: String = [pad, punctuation, letters, letters_ipa].concat();

    symbols
        .chars()
        .enumerate()
        .map(|(idx, c)| (c, idx as i64))
        .collect()
}

// Load voices from binary file
fn load_voices(path: &str) -> Result<HashMap<String, Vec<f32>>, String> {
    let mut file = File::open(path).map_err(|e| format!("Failed to open voices file: {}", e))?;

    let mut reader =
        NpzReader::new(&mut file).map_err(|e| format!("Failed to create NPZ reader: {}", e))?;

    let mut voices = HashMap::new();

    for name in reader
        .names()
        .map_err(|e| format!("Failed to read NPZ names: {:?}", e))?
    {
        let array: ArrayBase<OwnedRepr<f32>, IxDyn> = reader
            .by_name(&name)
            .map_err(|e| format!("Failed to read NPZ array {}: {:?}", name, e))?;
        let data: Vec<f32> = array.iter().cloned().collect();

        // Clean up the name (remove .npy extension if present)
        let clean_name = name.trim_end_matches(".npy");
        voices.insert(clean_name.to_string(), data);
    }

    Ok(voices)
}

// Download file from URL
async fn download_file(url: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let response = reqwest::get(url).await?;
    let bytes = response.bytes().await?;

    let mut file = File::create(path)?;
    file.write_all(&bytes)?;

    Ok(())
}

// Convert WAV bytes to f32 samples
fn wav_to_f32(wav_bytes: &[u8]) -> Result<Vec<f32>, String> {
    let cursor = Cursor::new(wav_bytes);
    let mut reader =
        hound::WavReader::new(cursor).map_err(|e| format!("Failed to read WAV: {}", e))?;

    let samples: Result<Vec<f32>, _> = reader
        .samples::<i16>()
        .map(|s| s.map(|sample| sample as f32 / 32768.0))
        .collect();

    samples.map_err(|e| format!("Failed to read samples: {}", e))
}

fn needs_chunking(text: &str) -> bool {
    text.chars().count() > LONG_TEXT_THRESHOLD || text.lines().count() > 3
}

fn chunk_crossfade_samples() -> usize {
    ((SAMPLE_RATE as usize) * CHUNK_CROSSFADE_MS) / 1000
}

fn append_with_crossfade(buffer: &mut Vec<f32>, next: &[f32], overlap_samples: usize) {
    if next.is_empty() {
        return;
    }

    if buffer.is_empty() || overlap_samples == 0 {
        buffer.extend_from_slice(next);
        return;
    }

    let overlap = overlap_samples.min(buffer.len()).min(next.len());
    if overlap == 0 {
        buffer.extend_from_slice(next);
        return;
    }

    let start = buffer.len() - overlap;
    for i in 0..overlap {
        let fade_in = i as f32 / overlap as f32;
        let fade_out = 1.0 - fade_in;
        buffer[start + i] = buffer[start + i] * fade_out + next[i] * fade_in;
    }

    buffer.extend_from_slice(&next[overlap..]);
}

// Split text into chunks for better synthesis
// Kokoro model handles shorter text better without dropping words
fn split_text_for_tts(text: &str, max_chars: usize) -> Vec<String> {
    // First try to split by sentences
    let sentences: Vec<&str> = text
        .split_terminator(&['.', '!', '?'][..])
        .filter(|s| !s.trim().is_empty())
        .collect();

    let mut chunks = Vec::new();
    let mut current_chunk = String::new();

    for sentence in sentences {
        // Add back the punctuation if it was there
        let full_sentence = if text.contains(&format!("{}.", sentence.trim())) {
            format!("{}.", sentence.trim())
        } else if text.contains(&format!("{}!", sentence.trim())) {
            format!("{}!", sentence.trim())
        } else if text.contains(&format!("{}?", sentence.trim())) {
            format!("{}?", sentence.trim())
        } else {
            sentence.trim().to_string()
        };

        // If this sentence alone is too long, split it by commas or words
        if full_sentence.len() > max_chars {
            // Try splitting by commas first
            let parts: Vec<&str> = full_sentence.split(',').collect();
            if parts.len() > 1 {
                for part in parts {
                    if part.trim().len() > max_chars {
                        // Still too long, split by words
                        chunks.extend(split_by_words(part, max_chars));
                    } else if !part.trim().is_empty() {
                        chunks.push(part.trim().to_string());
                    }
                }
            } else {
                // No commas, split by words
                chunks.extend(split_by_words(&full_sentence, max_chars));
            }
        }
        // If adding this sentence would make chunk too long, save current and start new
        else if !current_chunk.is_empty()
            && current_chunk.len() + full_sentence.len() + 1 > max_chars
        {
            chunks.push(current_chunk.trim().to_string());
            current_chunk = full_sentence;
        }
        // Add to current chunk
        else {
            if !current_chunk.is_empty() {
                current_chunk.push(' ');
            }
            current_chunk.push_str(&full_sentence);
        }
    }

    // Don't forget the last chunk
    if !current_chunk.is_empty() {
        chunks.push(current_chunk.trim().to_string());
    }

    // If no chunks were created (text had no sentence endings), split by words
    if chunks.is_empty() && !text.trim().is_empty() {
        chunks = split_by_words(text, max_chars);
    }

    chunks
}

// Split text by words when sentences are too long
fn split_by_words(text: &str, max_chars: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut chunks = Vec::new();
    let mut current = String::new();

    for word in words {
        if current.len() + word.len() + 1 > max_chars && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current = word.to_string();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        chunks.push(current.trim().to_string());
    }

    chunks
}

// Amplify audio - allows some clipping for maximum loudness
fn amplify_audio(audio: &[f32], gain: f32) -> Vec<f32> {
    audio
        .iter()
        .map(|&sample| {
            let amplified = sample * gain;

            // Simple hard clipping at the limits
            // This allows maximum volume even if it distorts a bit
            amplified.clamp(-1.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_extends_buffer() {
        let mut buffer = vec![1.0, 1.0, 1.0];
        let next = vec![0.0, 0.0, 0.0];
        append_with_crossfade(&mut buffer, &next, 2);
        // Result should be len 4 (3 + 3 - overlap)
        assert_eq!(buffer.len(), 4);
        // Last sample should come from next chunk
        assert!((buffer.last().copied().unwrap() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn detects_need_for_chunking() {
        let short = "hello world";
        assert!(!needs_chunking(short));

        let long = "This sentence is intentionally quite a bit longer than the \
                    short sample so that it exceeds the chunking threshold we set.";
        assert!(needs_chunking(long));
    }
}
