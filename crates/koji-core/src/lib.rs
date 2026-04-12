//! Koji Core Library
//!
//! Core functionality for Koji including model card management, process supervision,
//! and platform abstractions.
//!
//! ## Model Card Configuration
//!
//! Koji uses model cards to store quantization info, context settings, and sampling presets
//! for each model. Model cards are stored in `~/.config/koji/configs/<company>--<model>.toml`
//! and are automatically discovered when models are installed.

pub mod backends;
pub mod bench;
pub mod config;
pub mod db;
pub mod gpu;
pub mod logging;
pub mod models;
pub mod platform;
pub mod process;
pub mod profiles;
pub mod proxy;
pub mod self_update;

#[cfg(test)]
mod tests {
    mod mmproj_detection_test;
}
