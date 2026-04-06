//! CLI flag extraction utilities
//!
//! This module provides helper functions for extracting koji-specific flags
//! from command line arguments.

use anyhow::{Context, Result};

/// Extracted flags from command line arguments
#[derive(Debug, Clone)]
pub struct ExtractedFlags {
    /// Model identifier - extracted if it looks like a model card ref (contains `/`, no `.gguf`, not absolute path)
    pub model: Option<String>,
    /// Quantization level (e.g., "Q4_K_M")
    pub quant: Option<String>,
    /// Sampling profile name
    pub profile: Option<String>,
    /// Port to bind to
    pub port: Option<u16>,
    /// Context length override
    pub context_length: Option<u32>,
    /// Arguments not recognized as koji flags (passed to backend)
    pub remaining_args: Vec<String>,
}

/// Extract koji-specific flags from command line arguments.
///
/// Parses arguments looking for: `--model`, `--profile`, `--quant`, `--port`, `--ctx`
/// Supports both `--flag value` and `--flag=value` syntaxes.
///
/// # Model detection
/// A model argument is extracted if it looks like a model card reference:
/// - Contains `/` (e.g., "unsloth/Qwen3.5-0.8B")
/// - Does NOT contain `.gguf`
/// - Is NOT an absolute filesystem path
///
/// Otherwise, it's left in `remaining_args` for the backend.
///
/// # Flags consumed
/// Each recognized flag consumes both the flag AND its value from the argument list.
///
/// # Errors
/// Returns an error if a flag is present without a following value.
///
/// # Quant without model
/// If `--quant` is provided without `--model`, it's still extracted (no error).
/// The call site handles the warning about quant without model.
pub fn extract_koji_flags(args: Vec<String>) -> Result<ExtractedFlags> {
    let mut model: Option<String> = None;
    let mut quant: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut context_length: Option<u32> = None;
    let mut remaining_args = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        // Handle --flag=value syntax
        if let Some((flag, value)) = arg.split_once('=') {
            match flag {
                "--model" | "-m" => {
                    // Check if it looks like a model card ref
                    let is_model_ref = value.contains('/')
                        && !value.contains(".gguf")
                        && !value.starts_with(std::path::MAIN_SEPARATOR)
                        && !value.starts_with('/');
                    if is_model_ref {
                        model = Some(value.to_string());
                    } else {
                        // Not a model ref, leave in remaining_args
                        remaining_args.push(arg.clone());
                    }
                    i += 1;
                }
                "--profile" => {
                    profile = Some(value.to_string());
                    i += 1;
                }
                "--quant" => {
                    quant = Some(value.to_string());
                    i += 1;
                }
                "--port" => {
                    if let Ok(port_val) = value.parse::<u16>() {
                        port = Some(port_val);
                    } else {
                        remaining_args.push(arg.clone());
                    }
                    i += 1;
                }
                "--ctx" => {
                    if let Ok(ctx_val) = value.parse::<u32>() {
                        context_length = Some(ctx_val);
                    } else {
                        remaining_args.push(arg.clone());
                    }
                    i += 1;
                }
                _ => {
                    // Unknown flag=value, leave as-is
                    remaining_args.push(arg.clone());
                }
            }
        } else {
            // Traditional --flag value syntax
            match arg.as_str() {
                "--model" | "-m" => {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--model/-m flag requires a value");
                    }
                    let model_value = args[i + 1].clone();
                    // Check if it looks like a model card ref
                    let is_model_ref = model_value.contains('/')
                        && !model_value.contains(".gguf")
                        && !model_value.starts_with(std::path::MAIN_SEPARATOR)
                        && !model_value.starts_with('/');
                    if is_model_ref {
                        model = Some(model_value);
                    } else {
                        // Not a model ref, leave in remaining_args
                        remaining_args.push(arg.clone());
                        remaining_args.push(model_value);
                    }
                    i += 2;
                }
                "--profile" => {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--profile flag requires a value");
                    }
                    profile = Some(args[i + 1].clone());
                    i += 2;
                }
                "--quant" => {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--quant flag requires a value");
                    }
                    quant = Some(args[i + 1].clone());
                    i += 2;
                }
                "--port" => {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--port flag requires a valid u16 value");
                    }
                    let port_val = args[i + 1]
                        .parse::<u16>()
                        .context("--port requires a valid u16 value")?;
                    port = Some(port_val);
                    i += 2;
                }
                "--ctx" => {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--ctx flag requires a value");
                    }
                    let ctx_val = args[i + 1]
                        .parse::<u32>()
                        .context("--ctx requires a valid u32 value")?;
                    context_length = Some(ctx_val);
                    i += 2;
                }
                _ => {
                    remaining_args.push(arg.clone());
                    i += 1;
                }
            }
        }
    }

    Ok(ExtractedFlags {
        model,
        quant,
        profile,
        port,
        context_length,
        remaining_args,
    })
}
