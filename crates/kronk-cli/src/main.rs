//! Kronk CLI
//!
//! This is the entry point for the kronk CLI application.
//! It delegates to the library crate for all functionality.

fn main() {
    // Thin wrapper that calls into the library crate
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    if let Err(e) = rt.block_on(kronk::main()) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
