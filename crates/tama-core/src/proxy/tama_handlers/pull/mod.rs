use std::sync::Arc;

use anyhow::Result;

use crate::proxy::ProxyState;

pub mod download;
pub mod handlers;

#[cfg(test)]
pub(crate) use download::_setup_model_after_pull_with_config;
pub use download::start_download_from_queue;
pub use handlers::{handle_pull_job_stream, handle_tama_get_pull_job, handle_tama_pull_model};

/// Enqueue a download in the database queue.
///
/// Creates a `download_queue` DB row with status='queued' and returns immediately.
/// Does NOT start the download — the queue processor picks it up and starts it.
/// If `download_queue` is None (no DB configured), this is a no-op.
pub fn enqueue_download(
    state: &Arc<ProxyState>,
    job_id: String,
    repo_id: String,
    filename: &str,
    display_name: Option<&str>,
    quant: Option<&str>,
    context_length: Option<u32>,
) -> Result<(), anyhow::Error> {
    if let Some(ref svc) = state.download_queue {
        svc.enqueue(
            &job_id,
            &repo_id,
            filename,
            display_name,
            "model",
            quant,
            context_length,
        )?;
    }
    Ok(())
}
