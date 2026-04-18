pub mod build;
pub mod detect;
pub mod install;

// Re-export the main public entry point
pub use install::install_from_source;
