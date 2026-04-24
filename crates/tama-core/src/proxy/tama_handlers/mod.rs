pub mod backend_logs;
mod models;
mod pull;
mod system;
mod types;

#[cfg(test)]
mod tests;

pub use backend_logs::handle_backend_log_sse;
pub use models::{
    capitalize_first, generate_display_name, handle_opencode_list_models, handle_tama_get_model,
    handle_tama_list_models, handle_tama_load_model, handle_tama_unload_model,
};
pub use pull::{
    enqueue_download, handle_pull_job_stream, handle_tama_get_pull_job, handle_tama_pull_model,
    start_download_from_queue,
};
pub use system::{
    handle_hf_list_quants, handle_system_metrics_history, handle_system_metrics_stream,
    handle_tama_system_health, handle_tama_system_restart,
};
pub use types::{
    max_concurrent_pulls, ModelResponse, PullRequest, PullResponse, QuantDownloadSpec, QuantEntry,
};
