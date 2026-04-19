//! Typed query functions for the koji SQLite database.
//!
//! All functions take a `&Connection` — the caller owns the connection.
//! All functions are synchronous (no async).

mod active_model_queries;
mod backend_queries;
mod download_queue_queries;
mod metrics_queries;
mod model_config_queries;
mod model_queries;
mod types;
mod update_check_queries;

pub use active_model_queries::*;
pub use backend_queries::*;
pub use download_queue_queries::*;
pub use metrics_queries::*;
pub use model_config_queries::*;
pub use model_queries::*;
pub use types::*;
pub use update_check_queries::*;

#[cfg(test)]
mod tests;
