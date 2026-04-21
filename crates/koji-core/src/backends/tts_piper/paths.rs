use std::path::{Path, PathBuf};

/// Return the base directory for tts_piper models: `<backends_dir>/tts_piper`.
pub fn models_dir(base: &Path) -> PathBuf {
    base.join("tts_piper")
}

/// Return the path to the Piper ONNX model file.
pub fn model_file(base: &Path) -> PathBuf {
    models_dir(base).join("en_US-lessac-medium.onnx")
}

/// Return the path to the Piper JSON config file (contains voice settings).
pub fn config_file(base: &Path) -> PathBuf {
    models_dir(base).join("en_US-lessac-medium.onnx.json")
}
