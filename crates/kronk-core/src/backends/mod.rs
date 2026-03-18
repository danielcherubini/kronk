pub mod installer;
pub mod registry;
pub mod updater;

pub use installer::{install_backend, BackendSource, InstallOptions};
pub use registry::{BackendInfo, BackendRegistry, BackendType};
pub use updater::{check_latest_version, check_updates, update_backend, UpdateCheck};