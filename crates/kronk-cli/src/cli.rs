//! CLI argument parsing and command types
//!
//! This module contains all clap-derived types for the kronk CLI.

use crate::commands::backend::BackendSubcommand;
use anyhow::Context;
use clap::Parser;

/// Flags extracted from command line arguments that are specific to kronk.
/// Remaining args are passed through to the backend unchanged.
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
    /// Arguments not recognized as kronk flags (passed to backend)
    pub remaining_args: Vec<String>,
}

/// Extract kronk-specific flags from command line arguments.
///
/// Parses arguments looking for: `--model`, `--profile`, `--quant`, `--port`, `--ctx`
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
pub fn extract_kronk_flags(args: Vec<String>) -> anyhow::Result<ExtractedFlags> {
    let mut model: Option<String> = None;
    let mut quant: Option<String> = None;
    let mut profile: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut context_length: Option<u32> = None;
    let mut remaining_args = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];

        // Support --flag=value syntax: split on first '=' and treat as two args
        let (key, eq_value) = if let Some(eq_pos) = arg.find('=') {
            let (k, v) = arg.split_at(eq_pos);
            (k, Some(v[1..].to_string())) // skip the '='
        } else {
            (arg.as_str(), None)
        };

        match key {
            "--model" | "-m" => {
                let model_value = if let Some(ref v) = eq_value {
                    i += 1;
                    v.clone()
                } else {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--model/-m flag requires a value");
                    }
                    i += 2;
                    args[i - 1].clone()
                };
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
                    if eq_value.is_none() {
                        remaining_args.push(model_value);
                    }
                }
            }
            "--profile" => {
                let val = if let Some(ref v) = eq_value {
                    i += 1;
                    v.clone()
                } else {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--profile flag requires a value");
                    }
                    i += 2;
                    args[i - 1].clone()
                };
                profile = Some(val);
            }
            "--quant" => {
                let val = if let Some(ref v) = eq_value {
                    i += 1;
                    v.clone()
                } else {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--quant flag requires a value");
                    }
                    i += 2;
                    args[i - 1].clone()
                };
                quant = Some(val);
            }
            "--port" => {
                let val = if let Some(ref v) = eq_value {
                    i += 1;
                    v.clone()
                } else {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--port flag requires a valid u16 value");
                    }
                    i += 2;
                    args[i - 1].clone()
                };
                let port_val = val
                    .parse::<u16>()
                    .context("--port requires a valid u16 value")?;
                port = Some(port_val);
            }
            "--ctx" => {
                let val = if let Some(ref v) = eq_value {
                    i += 1;
                    v.clone()
                } else {
                    if i + 1 >= args.len() {
                        anyhow::bail!("--ctx flag requires a value");
                    }
                    i += 2;
                    args[i - 1].clone()
                };
                let ctx_val = val
                    .parse::<u32>()
                    .context("--ctx requires a valid u32 value")?;
                context_length = Some(ctx_val);
            }
            _ => {
                remaining_args.push(arg.clone());
                i += 1;
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

#[derive(Parser, Debug)]
#[command(name = "kronk")]
#[command(version)]
#[command(about = "Oh yeah, it's all coming together. -- Local AI Server")]
pub struct Args {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Parser, Debug)]
pub enum Commands {
    /// Run a single server in the foreground (for debugging)
    Run {
        /// Server name (required)
        name: String,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
    },
    /// Manage Windows services
    Service {
        #[command(subcommand)]
        command: ServiceCommands,
    },
    /// Internal: called by Windows SCM (do not use directly)
    #[command(hide = true)]
    ServiceRun {
        /// Run a single server backend (legacy mode)
        #[arg(short, long)]
        server: Option<String>,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
        /// Run the proxy server instead of a single backend
        #[arg(long)]
        proxy: bool,
    },
    /// Add a new server from a raw command line
    #[command(hide = true)]
    Add {
        /// Server name
        name: String,
        /// The full command: binary path followed by all arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Update an existing server with a new command line
    #[command(hide = true)]
    Update {
        /// Server name
        name: String,
        /// The full command: binary path followed by all arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Manage servers — list, add, edit, remove
    #[command(hide = true)]
    Server {
        #[command(subcommand)]
        command: ServerCommands,
    },
    /// Show status of all servers
    Status,
    /// Manage sampling profiles — presets for inference params
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },
    /// View or edit configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Manage models — pull, list, create servers
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
    /// Manage backends — install, update, list, remove
    Backend {
        #[command(subcommand)]
        command: BackendSubcommand,
    },
    /// Start kronk server (OpenAI-compatible API on a single port)
    Serve {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(long, default_value = "11434")]
        port: u16,
        /// Idle timeout in seconds (models unload after this many seconds of inactivity)
        #[arg(long, default_value = "300")]
        idle_timeout: u64,
    },
    /// OpenAI-compliant proxy for local AI models (deprecated: use `kronk serve`)
    #[command(hide = true)]
    Proxy {
        /// Proxy settings
        #[command(subcommand)]
        command: ProxyCommands,
    },
    /// View server logs
    Logs {
        /// Server name (defaults to "kronk" proxy logs)
        name: Option<String>,
        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
}

#[derive(Parser, Debug)]
pub enum ProxyCommands {
    /// Start the proxy server
    Start {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(long, default_value = "11434")]
        port: u16,
        /// Idle timeout in seconds (models unload after this many seconds of inactivity)
        #[arg(long, default_value = "300")]
        idle_timeout: u64,
    },
}

#[derive(Parser, Debug)]
pub enum ModelCommands {
    /// Pull a model from HuggingFace
    Pull {
        /// HuggingFace repo ID, e.g. "bartowski/OmniCoder-8B-GGUF"
        repo: String,
    },
    /// List installed models
    Ls,
    /// Enable a model (will be loaded on demand by the proxy)
    Enable {
        /// Model config name
        name: String,
    },
    /// Disable a model (will not be loaded by the proxy)
    Disable {
        /// Model config name
        name: String,
    },
    /// Create a model config from an installed model
    Create {
        /// Config name to create
        name: String,
        /// Model ID in "company/modelname" format
        #[arg(long)]
        model: String,
        /// Quant to use (e.g. "Q4_K_M"). Interactive picker if omitted.
        #[arg(long)]
        quant: Option<String>,
        /// Sampling profile: coding, chat, analysis, creative
        #[arg(long)]
        profile: Option<String>,
        /// Backend to use. Interactive picker if omitted and multiple exist.
        #[arg(long)]
        backend: Option<String>,
    },
    /// Remove an installed model
    Rm {
        /// Model ID in "company/modelname" format
        model: String,
    },
    /// Scan for untracked GGUF files and update model cards
    Scan,
    /// Search HuggingFace for GGUF models
    Search {
        /// Search query (e.g. "llama", "coding", "mistral 7b")
        query: String,
        /// Sort by: downloads, likes, modified (default: downloads)
        #[arg(long, default_value = "downloads")]
        sort: String,
        /// Maximum number of results (default: 20)
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,
        /// Immediately pull a selected result
        #[arg(long)]
        pull: bool,
    },
}

#[derive(Parser, Debug)]
pub enum ServerCommands {
    /// List all servers with status
    Ls,
    /// Add a new server from a raw command line
    Add {
        /// Server name
        name: String,
        /// Backend command and arguments (e.g. llama-server -m model.gguf)
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Edit an existing server's command line
    Edit {
        /// Server name
        name: String,
        /// New backend command and arguments
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Remove a server
    Rm {
        /// Server name to remove
        name: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

#[derive(Parser, Debug)]
pub enum ProfileCommands {
    /// List all available profiles and their sampling params
    List,
    /// Set a server's sampling profile
    Set {
        /// Server name
        server: String,
        /// Profile name: coding, chat, analysis, creative, or a custom name
        profile: String,
    },
    /// Clear a server's sampling profile (remove sampling preset)
    Clear {
        /// Server name
        server: String,
    },
    /// Create a custom profile with specific sampling params
    Add {
        /// Custom profile name
        name: String,
        #[arg(long)]
        temp: Option<f64>,
        #[arg(long)]
        top_k: Option<u32>,
        #[arg(long)]
        top_p: Option<f64>,
        #[arg(long)]
        min_p: Option<f64>,
        #[arg(long)]
        presence_penalty: Option<f64>,
        #[arg(long)]
        frequency_penalty: Option<f64>,
        #[arg(long)]
        repeat_penalty: Option<f64>,
    },
    /// Remove a custom profile
    Remove {
        /// Custom profile name
        name: String,
    },
}

#[derive(Parser, Debug)]
pub enum ServiceCommands {
    /// Install kronk as a system service (proxy mode)
    Install {
        /// Server name (omit to install the proxy; provide a name for legacy single-backend mode)
        name: Option<String>,
    },
    /// Start the kronk service
    Start {
        /// Server name (omit to start the proxy service)
        name: Option<String>,
    },
    /// Stop the kronk service
    Stop {
        /// Server name (omit to stop the proxy service)
        name: Option<String>,
    },
    /// Remove the kronk service
    Remove {
        /// Server name (omit to remove the proxy service)
        name: Option<String>,
    },
}

#[derive(Parser, Debug)]
pub enum ConfigCommands {
    /// Print the current configuration
    Show,
    /// Open config file in editor
    Edit,
    /// Show the config file path
    Path,
}
