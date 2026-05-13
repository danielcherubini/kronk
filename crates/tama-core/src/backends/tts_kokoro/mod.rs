pub mod download;
pub mod paths;

use super::{
    get_backend_install_path, BackendInfo, BackendManager, BackendSource, BackendType, ProgressSink,
};
use anyhow::Context;

/// Install the Kokoro TTS backend: clone repo, create venv, install deps, download model.
pub async fn install_tts_kokoro(
    manager: BackendManager,
    progress: Box<dyn ProgressSink>,
) -> anyhow::Result<()> {
    let p = std::sync::Arc::from(progress);

    // Compute the versioned base directory for installation
    let backends_dir = crate::backends::backends_dir()?;
    let version = paths::KOKORO_FASTAPI_TAG.to_string();
    let base_dir =
        get_backend_install_path(&backends_dir, &BackendType::TtsKokoro, "cpu", &version);

    // Run the full Kokoro-FastAPI installation pipeline
    download::install_kokoro_fastapi(&base_dir, &p).await?;

    // Register in the backend registry — path points to base_dir (parent of
    // install_dir and venv). This allows safe_remove_installation to remove
    // the entire versioned directory including both venv and repo.
    let info = BackendInfo {
        name: "tts_kokoro".to_string(),
        backend_type: BackendType::TtsKokoro,
        version: version.clone(),
        path: paths::base_dir(&base_dir),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64),
        gpu_type: None,
        gpu_variant: "cpu".to_string(),
        source: Some(BackendSource::SourceCode {
            version,
            git_url: paths::KOKORO_FASTAPI_URL.to_string(),
            commit: None,
        }),
    };

    manager
        .add_installation(&info)
        .with_context(|| "Failed to register Kokoro backend")?;

    Ok(())
}

/// Verify the installed Kokoro backend has all required files.
///
/// Checks:
/// (a) {install_dir}/api/src/main.py exists
/// (b) .git directory exists (proves clone worked)
/// (c) venv python can import uvicorn and kokoro
/// (d) model file exists at model_dir/kokoro-v1_0.pth
pub fn verify_tts_kokoro(info: &BackendInfo) -> anyhow::Result<()> {
    // info.path is now the base_dir (backends/tts_kokoro/)
    let base = &info.path;

    // (a) Check api/src/main.py exists
    let main_py = base
        .join("kokoro-fastapi")
        .join("api")
        .join("src")
        .join("main.py");
    if !main_py.exists() {
        return Err(anyhow::anyhow!(
            "Kokoro-FastAPI main.py not found at: {}",
            main_py.display()
        ));
    }

    // (b) Check .git directory exists (proves clone worked)
    let git_dir = base.join("kokoro-fastapi").join(".git");
    if !git_dir.is_dir() {
        return Err(anyhow::anyhow!(
            ".git directory not found at {}; \
             installation may have been done manually, not via git clone",
            git_dir.display()
        ));
    }

    // (c) Check venv python can import uvicorn and kokoro
    let python_bin = paths::python_bin(base);
    if !python_bin.exists() {
        return Err(anyhow::anyhow!(
            "Python binary not found at {}; \
             virtualenv may not be properly set up",
            python_bin.display()
        ));
    }

    let import_result = std::process::Command::new(&python_bin)
        .args(["-c", "import uvicorn; import kokoro"])
        .output();

    match import_result {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!(
                "Python import check failed: {}\n\
                 Command: {:?}\n\
                 Stderr: {}",
                output.status,
                &["-c", "import uvicorn; import kokoro"],
                stderr
            ));
        }
        Err(e) => {
            return Err(anyhow::anyhow!(
                "Failed to run Python import check: {}\n\
                 Ensure python3 and pip are on PATH.",
                e
            ));
        }
    }

    // (d) Check model file exists
    let model_file = paths::model_file(base);
    if !model_file.exists() {
        return Err(anyhow::anyhow!(
            "Kokoro model file not found at: {}",
            model_file.display()
        ));
    }

    Ok(())
}
