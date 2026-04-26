//! CLI argument parsing and command types
//!
//! This module contains all clap-derived types for the tama CLI.

use crate::commands::backend::BackendSubcommand;
use crate::commands::tts::TtsSubcommand;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "tama")]
#[command(version)]
#[command(about = "A local AI server with automatic backend management.")]
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
    /// Text-to-speech — synthesize speech, list voices
    Tts {
        #[command(subcommand)]
        command: TtsSubcommand,
    },
    /// Benchmark model inference performance
    Bench {
        /// Model config name to benchmark (required unless --all is used)
        name: Option<String>,
        /// Benchmark all enabled model configs sequentially
        #[arg(long)]
        all: bool,
        /// Prompt processing sizes, comma-separated (default: "512")
        #[arg(long, default_value = "512")]
        pp: String,
        /// Token generation lengths, comma-separated (default: "128")
        #[arg(long, default_value = "128")]
        tg: String,
        /// Number of measurement runs per test (default: 3)
        #[arg(long, default_value_t = 3)]
        runs: u32,
        /// Number of warmup runs before measuring (default: 1)
        #[arg(long, default_value_t = 1)]
        warmup: u32,
        /// Override context size (e.g. 4096, 8192)
        #[arg(long)]
        ctx: Option<u32>,
    },
    /// Start the tama server (OpenAI-compatible API on a single port)
    Serve {
        /// Host to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind to
        #[arg(long, default_value = "11434")]
        port: u16,
        /// Enable automatic unloading of idle models
        /// --auto-unload enables it, --auto-unload=false disables it, omit to use config value
        #[arg(long, num_args = 0..=1, default_missing_value = "true")]
        auto_unload: Option<bool>,
        /// Idle timeout in seconds (models unload after this many seconds of inactivity; requires --auto-unload)
        #[arg(long, default_value = "300")]
        idle_timeout: u64,
    },
    /// Update tama to the latest version from GitHub
    SelfUpdate {
        /// Only check for updates, don't install
        #[arg(long)]
        check: bool,
        /// Skip version comparison, always download latest
        #[arg(long)]
        force: bool,
    },
    /// View server logs
    Logs {
        /// Server name (defaults to "tama" proxy logs)
        name: Option<String>,
        /// Follow log output (like tail -f)
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show (default: 50)
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
    },
    /// Create a backup of configuration and database
    Backup {
        /// Output path for the backup archive (default: tama-backup-YYYY-MM-DD.tar.gz in current dir)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Show what would be backed up without creating the archive
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore from a backup archive
    Restore {
        /// Path to backup archive
        archive: PathBuf,
        /// Interactively select which models to restore
        #[arg(long)]
        select: bool,
        /// Show what would be restored without making changes
        #[arg(long)]
        dry_run: bool,
        /// Skip backend re-installation
        #[arg(long)]
        skip_backends: bool,
        /// Skip model re-downloading
        #[arg(long)]
        skip_models: bool,
    },
    /// Start the tama web control plane UI
    #[cfg(feature = "web-ui")]
    Web {
        /// Port to listen on (default: 11435)
        #[arg(long, default_value = "11435")]
        port: u16,
        /// Tama proxy base URL (default: http://127.0.0.1:11434)
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        proxy_url: String,
        /// Directory containing tama log files
        #[arg(long)]
        logs_dir: Option<std::path::PathBuf>,
        /// Path to tama config file
        #[arg(long)]
        config_path: Option<std::path::PathBuf>,
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
    Ls {
        /// Model identifier - extracted if it looks like a model card ref (contains `/`, no `.gguf`, not absolute path)
        #[arg(long)]
        model: Option<String>,
        /// Quantization level (e.g., "Q4_K_M")
        #[arg(long)]
        quant: Option<String>,
        /// Sampling profile name
        #[arg(long)]
        profile: Option<String>,
    },
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
        /// Config name to create (e.g. "gemma4-coding"). Prompted if omitted.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove an installed model
    Rm {
        /// Model ID in "company/modelname" format
        model: String,
    },
    /// Scan for untracked GGUF files and update model cards
    Scan,
    /// Remove orphaned GGUF files not referenced by any server config
    Prune {
        /// Show what would be deleted without actually deleting
        #[arg(long, short = 'n')]
        dry_run: bool,
        /// Skip confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Check for and download model updates from HuggingFace
    Update {
        /// Model ID to update (e.g. "bartowski/OmniCoder-8B-GGUF"). Checks all if omitted.
        model: Option<String>,
        /// Only check for updates, don't download
        #[arg(long, conflicts_with = "refresh")]
        check: bool,
        /// Refresh stored metadata without re-downloading (establishes baseline for future checks)
        #[arg(long, conflicts_with = "check")]
        refresh: bool,
        /// Skip confirmation prompt (for scripting/CI)
        #[arg(long, short = 'y')]
        yes: bool,
    },
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
    /// Verify downloaded GGUF files against their HuggingFace LFS SHA-256 hashes
    Verify {
        /// Model ID to verify (e.g. "bartowski/OmniCoder-8B-GGUF"). Verifies all if omitted.
        model: Option<String>,
    },
    /// Verify existing models and backfill missing LFS hashes from HuggingFace
    VerifyExisting {
        /// Model ID to verify (e.g. "bartowski/OmniCoder-8B-GGUF"). Verifies all if omitted.
        model: Option<String>,
        /// Show detailed progress for each file
        #[arg(long, default_value = "true")]
        verbose: bool,
    },
    /// Migrate model entries from tama.toml to the database
    Migrate,
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
        /// Profile name: coding, chat, analysis, creative
        profile: String,
    },
    /// Clear a server's sampling profile (remove sampling preset)
    Clear {
        /// Server name
        server: String,
    },
}

#[derive(Parser, Debug)]
pub enum ServiceCommands {
    /// Install tama as a system service (proxy mode)
    Install {
        /// Server name (omit to install the proxy; provide a name for legacy single-backend mode)
        name: Option<String>,
        /// Install as a system-wide service instead of a user service (requires root)
        #[arg(long)]
        system: bool,
    },
    /// Start the tama service
    Start {
        /// Server name (omit to start the proxy service)
        name: Option<String>,
        /// Manage the system-wide service instead of the user service
        #[arg(long)]
        system: bool,
    },
    /// Stop the tama service
    Stop {
        /// Server name (omit to stop the proxy service)
        name: Option<String>,
        /// Manage the system-wide service instead of the user service
        #[arg(long)]
        system: bool,
    },
    /// Restart the tama service (stop then start)
    Restart {
        /// Server name (omit to restart the proxy service)
        name: Option<String>,
        /// Manage the system-wide service instead of the user service
        #[arg(long)]
        system: bool,
    },
    /// Remove the tama service
    Remove {
        /// Server name (omit to remove the proxy service)
        name: Option<String>,
        /// Manage the system-wide service instead of the user service
        #[arg(long)]
        system: bool,
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
