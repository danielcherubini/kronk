//! GPU/VRAM utilities for benchmarking.
//!
//! Re-exports VRAM querying from `koji_core` for use in benchmark handlers.

pub use koji_core::gpu::{query_vram, VramInfo};
