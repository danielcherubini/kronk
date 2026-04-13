//! Backup and restore CLI commands.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;

/// Backup command arguments.
#[derive(clap::Parser, Debug)]
pub struct BackupArgs {
    /// Output path for the backup archive (default: koji-backup-YYYY-MM-DD.tar.gz in current dir)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Show what would be backed up without creating the archive
    #[arg(long)]
    pub dry_run: bool,
}

/// Restore command arguments.
#[derive(clap::Parser, Debug)]
pub struct RestoreArgs {
    /// Path to backup archive
    pub archive: PathBuf,

    /// Interactively select which models to restore
    #[arg(long)]
    pub select: bool,

    /// Show what would be restored without making changes
    #[arg(long)]
    pub dry_run: bool,

    /// Skip backend re-installation
    #[arg(long)]
    pub skip_backends: bool,

    /// Skip model re-downloading
    #[arg(long)]
    pub skip_models: bool,
}

/// Create a backup of the Koji configuration.
pub fn cmd_backup(
    config: &koji_core::config::Config,
    output: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    let config_dir = config
        .loaded_from
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Config has no loaded_from path"))?;

    let output_path = if let Some(path) = output {
        path
    } else {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d");
        PathBuf::from(format!("koji-backup-{}.tar.gz", timestamp))
    };

    if dry_run {
        println!(
            "Dry run - would create backup at: {}",
            output_path.display()
        );
        println!("\nFiles to be backed up:");
        println!("  - config.toml");
        if let Ok(entries) = std::fs::read_dir(config_dir.join("configs")) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "toml") {
                    println!("  - configs/{}", entry.file_name().to_string_lossy());
                }
            }
        }
        println!("  - koji.db");
        println!("\nNote: Model files and backend binaries are NOT included.");
        return Ok(());
    }

    let start = Instant::now();

    let manifest = koji_core::backup::create_backup(config_dir, &output_path)
        .context("Failed to create backup")?;

    let size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    println!("Backup created successfully: {}", output_path.display());
    println!("  Size: {:.2} MB", size as f64 / (1024.0 * 1024.0));
    println!("  Models: {}", manifest.models.len());
    println!("  Backends: {}", manifest.backends.len());
    println!("  Duration: {:.2}s", start.elapsed().as_secs_f64());

    Ok(())
}

/// Restore from a backup archive.
pub async fn cmd_restore(config: &mut koji_core::config::Config, args: RestoreArgs) -> Result<()> {
    let config_dir = config
        .loaded_from
        .as_ref()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow::anyhow!("Config has no loaded_from path"))?;

    if args.dry_run {
        println!("Dry run - would restore from: {}", args.archive.display());
        let manifest = koji_core::backup::extract_manifest(&args.archive)
            .context("Failed to read backup manifest")?;
        println!("\nBackup info:");
        println!("  Created: {}", manifest.created_at);
        println!("  Koji version: {}", manifest.koji_version);
        println!("\nWould restore:");
        println!("  - config.toml");
        println!("  - {} model cards", manifest.models.len());
        println!("  - koji.db");
        println!("\nWould install:");
        println!("  - {} backends", manifest.backends.len());
        println!("Would download:");
        println!("  - {} models", manifest.models.len());
        return Ok(());
    }

    // Extract backup to temp directory
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;

    let extract_result = koji_core::backup::extract_backup(&args.archive, temp_dir.path())
        .context("Failed to extract backup")?;

    println!("Backup extracted successfully");

    // Merge config
    let backup_config = toml::from_str::<koji_core::config::Config>(&std::fs::read_to_string(
        &extract_result.config_path,
    )?)
    .context("Failed to parse backup config")?;

    let merge_stats = koji_core::backup::merge_config(&mut *config, &backup_config);

    println!(
        "Config merged: {} new backends, {} new models",
        merge_stats.new_backends.len(),
        merge_stats.new_models.len()
    );

    // Persist merged config
    let config_content = toml::to_string_pretty(&config)
        .context("Failed to serialize merged config")?;
    std::fs::write(&extract_result.config_path, config_content)
        .context("Failed to write merged config")?;

    // Merge model cards
    let card_paths = koji_core::backup::merge_model_cards(
        &config_dir.join("configs"),
        &temp_dir.path().join("configs"),
    )
    .context("Failed to merge model cards")?;

    println!("Model cards: {} copied", card_paths.len());

    // Merge database
    let local_conn = koji_core::db::open(&config_dir)
        .context("Failed to open local database")?
        .conn;

    let db_stats = koji_core::backup::merge_database(&local_conn, &extract_result.db_path)
        .context("Failed to merge database")?;

    println!(
        "Database merged: {} new pulls, {} new files, {} new backends",
        db_stats.new_model_pulls, db_stats.new_model_files, db_stats.new_backend_installations,
    );

    // Install backends (if not skipped)
    if !args.skip_backends {
        println!("\nInstalling backends...");
        // TODO: Implement backend installation from DB records
        println!("Backend installation skipped (not yet implemented)");
    }

    // Download models (if not skipped)
    if !args.skip_models {
        println!("\nDownloading models...");
        // TODO: Implement model download from DB records
        println!("Model download skipped (not yet implemented)");
    }

    // Cleanup
    drop(temp_dir);

    println!("\nRestore complete!");

    Ok(())
}
