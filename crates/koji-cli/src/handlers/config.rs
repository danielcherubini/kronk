//! Config command handler
//!
//! Handles `koji config show/edit/path` commands.

use anyhow::Result;
use koji_core::config::Config;

/// View or edit configuration
pub fn cmd_config(config: &Config, command: crate::cli::ConfigCommands) -> Result<()> {
    match command {
        crate::cli::ConfigCommands::Show => {
            let toml_str = toml::to_string_pretty(config)?;
            println!("{}", toml_str);
        }
        crate::cli::ConfigCommands::Edit => {
            let path = Config::config_path()?;
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "notepad".to_string());
            std::process::Command::new(&editor)
                .arg(&path)
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to open editor '{}': {}", editor, e))?;
        }
        crate::cli::ConfigCommands::Path => {
            let path = Config::config_path()?;
            println!("{}", path.display());
        }
    }
    Ok(())
}
