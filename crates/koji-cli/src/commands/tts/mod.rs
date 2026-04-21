//! TTS (Text-to-Speech) CLI commands.
//!
//! Provides subcommands for synthesizing speech and managing TTS configuration.

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::io::Write;
use std::path::PathBuf;

pub mod config;

use crate::commands::tts::config::TtsConfigCmd;

#[derive(Debug, Args)]
pub struct TtsArgs {
    #[command(subcommand)]
    pub command: TtsSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum TtsSubcommand {
    /// Synthesize speech from text
    Say {
        /// TTS engine to use (kokoro or piper)
        #[arg(long, default_value = "kokoro")]
        engine: String,

        /// Voice ID to use
        #[arg(long)]
        voice: Option<String>,

        /// Speech speed (0.5 = half speed, 2.0 = double speed)
        #[arg(long, default_value_t = 1.0)]
        speed: f32,

        /// Output audio format (mp3, wav, ogg)
        #[arg(long, default_value = "mp3")]
        format: String,

        /// Output file path (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Text to synthesize (read from stdin if not provided and no --output)
        text: Option<String>,
    },

    /// List available voices for an engine
    Voices {
        /// TTS engine to query (kokoro or piper)
        #[arg(long)]
        engine: String,
    },

    /// Manage TTS configuration
    Config {
        #[command(subcommand)]
        command: TtsConfigCmd,
    },
}

pub async fn run(cmd: TtsArgs) -> Result<()> {
    match cmd.command {
        TtsSubcommand::Say {
            engine,
            voice,
            speed,
            format,
            output,
            text,
        } => cmd_say(&engine, voice.as_deref(), speed, &format, output, text).await,
        TtsSubcommand::Voices { engine } => cmd_voices(&engine).await,
        TtsSubcommand::Config { command } => config::run(command),
    }
}

async fn cmd_say(
    engine: &str,
    voice: Option<&str>,
    speed: f32,
    format: &str,
    output: Option<PathBuf>,
    text: Option<String>,
) -> Result<()> {
    // Get the text to synthesize
    let text = match text {
        Some(t) => t,
        None => {
            if output.is_some() {
                anyhow::bail!("Text is required when no output file is specified. Provide --output or pass text as argument.");
            }
            // Read from stdin
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)?;
            input.trim().to_string()
        }
    };

    if text.is_empty() {
        anyhow::bail!("Text to synthesize cannot be empty");
    }

    // Validate speed
    if !(0.5..=2.0).contains(&speed) {
        anyhow::bail!("Speed must be between 0.5 and 2.0");
    }

    // Call the proxy server's TTS endpoint
    let client = reqwest::Client::new();
    let base_url = "http://127.0.0.1:11434";

    let url = format!("{}/v1/audio/speech", base_url);

    let body = serde_json::json!({
        "model": engine,
        "input": text,
        "voice": voice,
        "response_format": format,
        "stream": false,
    });

    println!("Synthesizing speech with {} engine...", engine);

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Failed to connect to proxy server at {}", base_url))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!("TTS request failed ({}): {}", status, error_text);
    }

    let audio_data = response.bytes().await?;

    if let Some(path) = &output {
        std::fs::write(path, &audio_data)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        println!("Audio saved to: {}", path.display());
    } else {
        // Write to stdout
        std::io::stdout().write_all(&audio_data)?;
        println!(); // newline after audio data
    }

    Ok(())
}

async fn cmd_voices(engine: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let url = "http://127.0.0.1:11434/v1/audio/voices";

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| "Failed to connect to proxy server")?;

    if !response.status().is_success() {
        anyhow::bail!("TTS voices endpoint returned error: {}", response.status());
    }

    let json: serde_json::Value = response.json().await?;

    if let Some(voices) = json.get("data").and_then(|v| v.as_array()) {
        println!("Available voices for {} engine:\n", engine);
        for voice in voices {
            let id = voice.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let name = voice.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let lang = voice
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            println!("  {} - {} ({})", id, name, lang);
        }
    } else {
        println!("No voices available for {} engine.", engine);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_speed_validation_range() {
        // Speed must be between 0.5 and 2.0
        assert!((0.5..=2.0).contains(&0.5));
        assert!((0.5..=2.0).contains(&1.0));
        assert!((0.5..=2.0).contains(&2.0));
        assert!(!(0.5..=2.0).contains(&0.4));
        assert!(!(0.5..=2.0).contains(&2.1));
    }
}
