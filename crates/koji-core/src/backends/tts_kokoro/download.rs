use anyhow::{Context, Result};
use std::sync::Arc;

use super::paths::*;
use crate::backends::ProgressSink;

/// List of available Kokoro voice IDs (26 voices from hexgrad/Kokoro-82M).
pub const VOICE_IDS: &[&str] = &[
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

/// Download the Kokoro 82M PyTorch model from HuggingFace.
/// The kokoro-micro Rust library handles ONNX conversion at runtime.
pub async fn download_kokoro_model(progress: &Arc<dyn ProgressSink>) -> Result<()> {
    let url = "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/kokoro-v1_0.pth";
    let base_dir = crate::backends::backends_dir()?;
    let dest = model_file(&base_dir);

    // Create parent directories
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| "Failed to create model directory")?;
    }

    progress.log("Downloading Kokoro 82M model from HuggingFace...");
    crate::backends::installer::download_with_client(url, &dest, Some(progress), None)
        .await
        .with_context(|| "Failed to download Kokoro model")?;

    progress.log(&format!("Kokoro model saved to: {}", dest.display()));
    Ok(())
}

/// Download all available Kokoro voice files from HuggingFace.
pub async fn download_kokoro_voices(progress: &Arc<dyn ProgressSink>) -> Result<()> {
    let base_dir = crate::backends::backends_dir()?;
    let voices_path = voices_dir(&base_dir);
    std::fs::create_dir_all(&voices_path).with_context(|| "Failed to create voices directory")?;

    // Voice files are .pt (PyTorch) format — kokoro-micro handles them at runtime.
    for voice_id in VOICE_IDS {
        let url = format!(
            "https://huggingface.co/hexgrad/Kokoro-82M/resolve/main/voices/{}.pt",
            voice_id
        );
        let dest = voice_file(&base_dir, voice_id);

        progress.log(&format!("Downloading voice: {}...", voice_id));
        crate::backends::installer::download_with_client(&url, &dest, Some(progress), None)
            .await
            .with_context(|| format!("Failed to download voice: {}", voice_id))?;
    }

    progress.log(&format!(
        "All {} Kokoro voices downloaded.",
        VOICE_IDS.len()
    ));
    Ok(())
}

/// Download both the model and all voices.
pub async fn download_all(progress: &Arc<dyn ProgressSink>) -> Result<()> {
    download_kokoro_model(progress).await?;
    download_kokoro_voices(progress).await?;
    Ok(())
}
