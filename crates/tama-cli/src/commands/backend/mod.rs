pub mod parse;

mod install;
mod list;
mod remove;
mod switch;
mod update;

use anyhow::Result;
use clap::{Args, Subcommand};
use tama_core::config::Config;

#[derive(Debug, Args)]
pub struct BackendArgs {
    #[command(subcommand)]
    pub command: BackendSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum BackendSubcommand {
    /// Install a new backend (LLM or TTS)
    Install {
        /// Backend type: llama_cpp, ik_llama, or tts_kokoro
        #[arg(value_name = "TYPE")]
        backend_type: String,

        /// Version to install (e.g., b8407). Defaults to latest.
        #[arg(short, long)]
        version: Option<String>,

        /// Force build from source instead of downloading pre-built binary
        #[arg(long)]
        build: bool,

        /// Pin to a specific git commit hash (implies --build).
        /// Example: --commit 61fad8b0940af2bfda9c2708b899c1fe16f9455b
        #[arg(long)]
        commit: Option<String>,

        /// Custom name for this backend installation
        #[arg(short, long)]
        name: Option<String>,

        /// GPU acceleration type (cpu, cuda, cuda:12, rocm, rocm:6, vulkan, metal)
        #[arg(long)]
        gpu: Option<String>,

        /// Overwrite existing backend installation
        #[arg(short, long)]
        force: bool,
    },

    /// Update an installed backend to the latest version
    Update {
        /// Name of the backend to update
        name: String,

        /// Force reinstall even if already up to date
        #[arg(short, long)]
        force: bool,
    },

    /// List installed backends
    #[command(alias = "ls")]
    List,

    /// Remove an installed backend
    #[command(alias = "rm")]
    Remove {
        /// Name of the backend to remove
        name: String,
        /// GPU variant to remove (cpu, cuda, vulkan, rocm, metal). Omit to remove all variants.
        #[arg(long)]
        gpu: Option<String>,
    },

    /// Check for updates to all installed backends
    CheckUpdates,

    /// List all versions of a backend (not just the active one)
    #[command(alias = "versions")]
    AllVersions {
        /// Name of the backend (omit to list all backends with all their versions)
        #[arg(long)]
        name: Option<String>,
    },

    /// Activate a specific version of a backend
    Switch {
        /// Name of the backend
        name: String,
        /// Version to activate
        version: String,
        /// GPU variant (cpu, cuda, vulkan, rocm, metal). Auto-inferred if only one variant exists.
        #[arg(long)]
        gpu: Option<String>,
    },

    /// Remove a single version (not all versions)
    RemoveVersion {
        /// Name of the backend
        name: String,
        /// Version to remove
        version: String,
        /// GPU variant (cpu, cuda, vulkan, rocm, metal). Auto-inferred if only one variant exists.
        #[arg(long)]
        gpu: Option<String>,
    },
}

pub async fn run(config: &Config, cmd: BackendArgs) -> Result<()> {
    match cmd.command {
        BackendSubcommand::Install {
            backend_type,
            version,
            build,
            commit,
            name,
            gpu,
            force,
        } => {
            install::cmd_install(
                config,
                &backend_type,
                version,
                build,
                commit,
                name,
                gpu,
                force,
            )
            .await
        }
        BackendSubcommand::Update { name, force } => update::cmd_update(config, &name, force).await,
        BackendSubcommand::List => list::cmd_list(config).await,
        BackendSubcommand::Remove { name, gpu } => {
            remove::cmd_remove(config, &name, gpu.as_deref()).await
        }
        BackendSubcommand::CheckUpdates => list::cmd_check_updates(config).await,
        BackendSubcommand::AllVersions { name } => {
            list::cmd_all_versions(config, name.as_deref()).await
        }
        BackendSubcommand::Switch { name, version, gpu } => {
            switch::cmd_switch(config, &name, &version, gpu.as_deref()).await
        }
        BackendSubcommand::RemoveVersion { name, version, gpu } => {
            remove::cmd_remove_version(config, &name, &version, gpu.as_deref()).await
        }
    }
}
