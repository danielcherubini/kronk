mod download;
mod extract;
mod prebuilt;
mod source;
mod urls;

pub use extract::{extract_archive, find_backend_binary};
pub use prebuilt::prepare_target_dir;
pub use urls::get_prebuilt_url;

use anyhow::Result;
use std::path::PathBuf;

use super::registry::{BackendSource, BackendType};
use crate::gpu::GpuType;

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub backend_type: BackendType,
    pub source: BackendSource,
    pub target_dir: PathBuf,
    pub gpu_type: Option<GpuType>,
    /// When true, skip the target directory existence check.
    /// Used by the update path where the directory already exists.
    pub allow_overwrite: bool,
}

/// Main entry point for installing a backend.
///
/// Clones `source` from `options` before matching so that `options` fields
/// remain accessible inside each arm.
pub async fn install_backend(options: InstallOptions) -> Result<PathBuf> {
    let source = options.source.clone();
    match source {
        BackendSource::Prebuilt { version } => prebuilt::install_prebuilt(&options, &version).await,
        BackendSource::SourceCode {
            version,
            git_url,
            commit,
        } => source::install_from_source(&options, &version, &git_url, commit.as_deref()).await,
    }
}
