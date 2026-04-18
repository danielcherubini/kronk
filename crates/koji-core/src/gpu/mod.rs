pub mod detect;
pub mod system;
pub mod vram;

// Re-export all public items for backward compatibility
pub use detect::{
    detect_amdgpu_targets, detect_build_prerequisites, detect_cuda_version,
    parse_rocminfo_gfx_names, suggest_context_sizes, BuildPrerequisites, ContextSuggestion,
    GpuType, DEFAULT_CUDA_VERSION,
};
pub use system::{
    collect_system_metrics, collect_system_metrics_with, MetricSample, ModelStatus, SystemMetrics,
};
pub use vram::{query_vram, VramInfo};
