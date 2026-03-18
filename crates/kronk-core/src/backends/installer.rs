// Stub - will be implemented in Task 3
pub async fn install_backend(_options: crate::backends::installer::InstallOptions) -> anyhow::Result<std::path::PathBuf> {
    unimplemented!()
}

#[derive(Debug, Clone)]
pub enum BackendSource {
    Prebuilt { version: String },
    SourceCode { version: String, git_url: String },
}

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub backend_type: crate::backends::registry::BackendType,
    pub source: BackendSource,
    pub target_dir: std::path::PathBuf,
    pub gpu_type: Option<crate::gpu::GpuType>,
}