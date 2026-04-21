use anyhow::{Context, Result};
use std::sync::Arc;

use super::paths::*;
use crate::backends::ProgressSink;

/// Default Piper voice to download: en_US-lessac-medium
pub const DEFAULT_VOICE_ID: &str = "en_US-lessac-medium";

/// Download the default Piper model (en_US-lessac-medium) from HuggingFace.
///
/// Source: https://huggingface.co/rhasspy/piper-voices/tree/main/en/en_US/lessac/medium
pub async fn download_piper_model(progress: &Arc<dyn ProgressSink>) -> Result<()> {
    let base_dir = crate::backends::backends_dir()?;

    // Piper voices are organized by language/region/voice/quality on HF
    // The default voice is en_US-lessac-medium
    let voice_path: &str = "en/en_US/lessac/medium";

    progress.log(&format!("Downloading Piper voice: {}...", DEFAULT_VOICE_ID));

    // Download the ONNX model
    let onnx_url = format!(
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/{}/{}.onnx",
        voice_path, DEFAULT_VOICE_ID
    );
    let onnx_dest = model_file(&base_dir);

    // Piper config is named {voice_id}.onnx.json, not .json
    let json_url = format!(
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/{}/{}.onnx.json",
        voice_path, DEFAULT_VOICE_ID
    );

    if let Some(parent) = onnx_dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| "Failed to create piper directory")?;
    }

    crate::backends::installer::download_with_client(&onnx_url, &onnx_dest, Some(progress), None)
        .await
        .with_context(|| "Failed to download Piper ONNX model")?;

    progress.log(&format!("Piper model saved to: {}", onnx_dest.display()));

    // Download the JSON config (contains voice settings)
    let json_dest = config_file(&base_dir);

    crate::backends::installer::download_with_client(&json_url, &json_dest, Some(progress), None)
        .await
        .with_context(|| "Failed to download Piper config")?;

    progress.log(&format!(
        "Piper configuration saved to: {}",
        json_dest.display()
    ));
    Ok(())
}
