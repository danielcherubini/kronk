//! Command handlers for the kronk CLI
//!
//! Each submodule handles a specific command group.

pub mod bench;
pub mod config;
pub mod logs;
pub mod profile;
pub mod run;
pub mod serve;
pub mod server;
pub mod service_cmd;
pub mod status;
#[cfg(feature = "web-ui")]
pub mod web;
