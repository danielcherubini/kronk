//! TTS configuration management via SQLite.

use anyhow::Result;
use clap::Subcommand;
use koji_core::config::Config;
use koji_core::db::queries::*;

#[derive(Debug, Subcommand)]
pub enum TtsConfigCmd {
    /// Set TTS configuration for an engine
    Set {
        /// TTS engine name (kokoro or piper)
        #[arg(long)]
        engine: String,

        /// Default voice ID
        #[arg(long)]
        voice: Option<String>,

        /// Speech speed (0.5 to 2.0)
        #[arg(long, default_value_t = 1.0)]
        speed: f32,

        /// Output format (mp3, wav, ogg)
        #[arg(long, default_value = "mp3")]
        format: String,
    },

    /// Show current TTS configuration
    Show {
        /// TTS engine to show (omit for all engines)
        #[arg(long)]
        engine: Option<String>,
    },
}

pub fn run(cmd: TtsConfigCmd) -> Result<()> {
    match cmd {
        TtsConfigCmd::Set {
            engine,
            voice,
            speed,
            format,
        } => cmd_set(&engine, voice.as_deref(), speed, &format),
        TtsConfigCmd::Show { engine } => cmd_show(engine.as_deref()),
    }
}

fn cmd_set(engine: &str, voice: Option<&str>, speed: f32, format: &str) -> Result<()> {
    if !(0.5..=2.0).contains(&speed) {
        anyhow::bail!("Speed must be between 0.5 and 2.0");
    }

    let valid_formats = ["mp3", "wav", "ogg"];
    if !valid_formats.contains(&format.to_lowercase().as_str()) {
        anyhow::bail!("Invalid format '{}'. Supported: mp3, wav, ogg", format);
    }

    let conn = Config::open_db();

    // Upsert the config
    let record = TtsConfigRecord {
        id: 0, // Will be set by INSERT OR REPLACE
        engine: engine.to_string(),
        default_voice: voice.map(String::from),
        speed,
        format: format.to_lowercase(),
        enabled: true,
        created_at: String::new(),
        updated_at: String::new(),
    };

    let id = upsert_tts_config(&conn, &record)?;

    println!("TTS configuration for '{}' saved (id: {}).", engine, id);
    Ok(())
}

fn cmd_show(engine: Option<&str>) -> Result<()> {
    let conn = Config::open_db();

    if let Some(name) = engine {
        // Show specific engine config
        let config = get_tts_config(&conn, name)?;
        match config {
            Some(c) => print_config(&c),
            None => println!(
                "No configuration found for '{}'. Use `koji tts config set --engine {}` to configure.",
                name, name
            ),
        }
    } else {
        // Show all engine configs
        let configs = get_all_tts_configs(&conn)?;
        if configs.is_empty() {
            println!("No TTS configurations found.");
            return Ok(());
        }
        for c in &configs {
            print_config(c);
            println!();
        }
    }

    Ok(())
}

fn print_config(c: &TtsConfigRecord) {
    println!("Engine:     {}", c.engine);
    println!("Voice:      {:?}", c.default_voice);
    println!("Speed:      {}", c.speed);
    println!("Format:     {}", c.format);
    println!("Enabled:    {}", c.enabled);
    println!("Updated:    {}", c.updated_at);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speed_validation() {
        assert!((0.5..=2.0).contains(&1.0));
        assert!(!(0.5..=2.0).contains(&0.4));
    }
}
