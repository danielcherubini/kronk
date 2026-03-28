//! Windows platform support
//!
//! Service installation, lifecycle management, firewall rules, and permissions.

mod firewall;
mod install;
mod permissions;
mod service;

// Re-export all public functions so callers don't need to change.
// e.g. `kronk_core::platform::windows::start_service` still works.
pub use firewall::{add_firewall_rule, remove_firewall_rule};
pub use install::{install_proxy_service, install_service};
pub use service::{query_service, remove_service, start_service, stop_service};
