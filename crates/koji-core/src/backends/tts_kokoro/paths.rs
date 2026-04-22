use std::path::{Path, PathBuf};

/// Kokoro model version used for versioned directory naming.
pub const MODEL_VERSION: &str = "kokoro-v1_0";

/// The pinned git tag for the Kokoro-FastAPI repository.
pub const KOKORO_FASTAPI_TAG: &str = "v0.2.4";

/// The GitHub URL for the Kokoro-FastAPI repository.
pub const KOKORO_FASTAPI_URL: &str = "https://github.com/remsky/Kokoro-FastAPI.git";

/// Return the base directory for tts_kokoro: `<backends_dir>/tts_kokoro`.
pub fn base_dir(base: &Path) -> PathBuf {
    base.join("tts_kokoro")
}

/// Return the git clone target directory: `<backends_dir>/tts_kokoro/kokoro-fastapi/`.
pub fn install_dir(base: &Path) -> PathBuf {
    base_dir(base).join("kokoro-fastapi")
}

/// Return the Python virtualenv directory: `<backends_dir>/tts_kokoro/venv/`.
pub fn venv_dir(base: &Path) -> PathBuf {
    base_dir(base).join("venv")
}

/// Return the Python binary inside the venv: `<venv_dir>/bin/python`.
pub fn python_bin(base: &Path) -> PathBuf {
    venv_dir(base).join("bin").join("python")
}

/// Return the model directory where download_model.py places files.
/// The script downloads to `<repo>/api/api/src/models/v1_0/` relative to repo root.
pub fn model_dir(base: &Path) -> PathBuf {
    install_dir(base)
        .join("api")
        .join("api")
        .join("src")
        .join("models")
        .join("v1_0")
}

/// Return the voice packs directory.
pub fn voices_dir(base: &Path) -> PathBuf {
    install_dir(base)
        .join("api")
        .join("api")
        .join("src")
        .join("voices")
        .join("v1_0")
}

/// Return the path to the Kokoro PyTorch model file.
pub fn model_file(base: &Path) -> PathBuf {
    model_dir(base).join("kokoro-v1_0.pth")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_base() -> PathBuf {
        PathBuf::from("/tmp/test_backends")
    }

    #[test]
    fn test_base_dir() {
        let base = test_base();
        assert_eq!(
            base_dir(&base),
            PathBuf::from("/tmp/test_backends/tts_kokoro")
        );
    }

    #[test]
    fn test_install_dir() {
        let base = test_base();
        assert_eq!(
            install_dir(&base),
            PathBuf::from("/tmp/test_backends/tts_kokoro/kokoro-fastapi")
        );
    }

    #[test]
    fn test_venv_dir() {
        let base = test_base();
        assert_eq!(
            venv_dir(&base),
            PathBuf::from("/tmp/test_backends/tts_kokoro/venv")
        );
    }

    #[test]
    fn test_python_bin() {
        let base = test_base();
        assert_eq!(
            python_bin(&base),
            PathBuf::from("/tmp/test_backends/tts_kokoro/venv/bin/python")
        );
    }

    #[test]
    fn test_model_dir() {
        let base = test_base();
        assert_eq!(
            model_dir(&base),
            PathBuf::from("/tmp/test_backends/tts_kokoro/kokoro-fastapi/api/src/models/v1_0")
        );
    }

    #[test]
    fn test_model_file() {
        let base = test_base();
        assert_eq!(
            model_file(&base),
            PathBuf::from(
                "/tmp/test_backends/tts_kokoro/kokoro-fastapi/api/src/models/v1_0/kokoro-v1_0.pth"
            )
        );
    }

    #[test]
    fn test_model_file_is_inside_model_dir() {
        let base = test_base();
        assert!(model_file(&base).starts_with(model_dir(&base)));
    }

    #[test]
    fn test_install_dir_contains_venv_dir() {
        // install_dir and venv_dir are siblings under base_dir
        let base = test_base();
        let install = install_dir(&base);
        let venv = venv_dir(&base);
        assert!(install.starts_with(base_dir(&base)));
        assert!(venv.starts_with(base_dir(&base)));
        assert_ne!(install, venv);
    }

    #[test]
    fn test_python_bin_is_inside_venv() {
        let base = test_base();
        assert!(python_bin(&base).starts_with(venv_dir(&base)));
    }

    #[test]
    fn test_model_dir_is_inside_install_dir() {
        let base = test_base();
        assert!(model_dir(&base).starts_with(install_dir(&base)));
    }
}
