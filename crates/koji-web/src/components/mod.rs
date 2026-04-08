pub mod backend_card;
pub mod config_nav;
pub mod form_validation;
pub mod general_section;
pub mod install_modal;
pub mod job_log_panel;
pub mod modal;
pub mod nav;
pub mod pull_quant_wizard;
pub mod sampling_templates_section;
pub mod sparkline;
pub mod supervisor_section;

// New components for backend UI
#[cfg(feature = "ssr")]
pub use backend_card::BackendCard;
#[cfg(feature = "ssr")]
pub use install_modal::InstallModal;
#[cfg(feature = "ssr")]
pub use job_log_panel::JobLogPanel;
