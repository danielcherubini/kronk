use anyhow::Result;
use std::collections::HashSet;
use tama_core::config::Config;
use tama_core::models::{ModelCard, ModelManager};

pub(super) fn cmd_prune(config: &Config, dry_run: bool, yes: bool) -> Result<()> {
    // Helper to format bytes
    fn format_bytes(b: u64) -> String {
        const KIB: f64 = 1024.0;
        const MIB: f64 = KIB * 1024.0;
        const GIB: f64 = MIB * 1024.0;
        let bf = b as f64;
        if bf >= GIB {
            format!("{:.2} GiB", bf / GIB)
        } else if bf >= MIB {
            format!("{:.1} MiB", bf / MIB)
        } else if bf >= KIB {
            format!("{:.1} KiB", bf / KIB)
        } else {
            format!("{} B", b)
        }
    }

    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;

    // Build set of referenced files: (repo_id, filename)
    let mut referenced_files: HashSet<(String, String)> = HashSet::new();
    let db_dir = tama_core::config::Config::config_dir()?;
    let mgr = ModelManager::open(&db_dir)?;
    let model_configs = tama_core::db::load_model_configs(mgr.conn())?;

    for model_config in model_configs.values() {
        if let Some(ref repo_id) = model_config.model {
            for quant_entry in model_config.quants.values() {
                referenced_files.insert((repo_id.clone(), quant_entry.file.clone()));
            }
        }
    }

    // Scan for orphaned GGUF files recursively
    // Tuple: (repo_id, filename, file_path, size)
    let mut orphaned_files: Vec<(String, String, std::path::PathBuf, u64)> = Vec::new();

    if models_dir.exists() {
        // Recursively walk models_dir to find all GGUF files
        fn scan_for_ggufs(
            dir: &std::path::Path,
            base_dir: &std::path::Path,
            referenced_files: &HashSet<(String, String)>,
            orphaned_files: &mut Vec<(String, String, std::path::PathBuf, u64)>,
        ) -> std::io::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_file() {
                    if path.extension().is_none_or(|e| e != "gguf") {
                        continue;
                    }

                    // Compute repo_id from relative path
                    let relative_path = path
                        .strip_prefix(base_dir)
                        .map_err(|_| std::io::Error::other("Failed to compute relative path"))?;
                    let repo_id = relative_path
                        .parent()
                        .and_then(|p| p.to_str())
                        .unwrap_or("")
                        .replace(std::path::MAIN_SEPARATOR, "/");

                    let filename = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let file_key = (repo_id.clone(), filename.clone());

                    if !referenced_files.contains(&file_key) {
                        if let Ok(metadata) = std::fs::metadata(&path) {
                            orphaned_files.push((repo_id, filename, path, metadata.len()));
                        }
                    }
                } else if path.is_dir() {
                    scan_for_ggufs(&path, base_dir, referenced_files, orphaned_files)?;
                }
            }
            Ok(())
        }

        scan_for_ggufs(
            &models_dir,
            &models_dir,
            &referenced_files,
            &mut orphaned_files,
        )?;
    }

    if orphaned_files.is_empty() {
        println!("No orphaned GGUF files found.");
        return Ok(());
    }

    // Display what would be deleted
    println!("Found {} orphaned GGUF file(s):", orphaned_files.len());
    let mut total_size = 0u64;
    for (repo_id, filename, _, size) in &orphaned_files {
        println!("  {} ({}) [{}]", filename, format_bytes(*size), repo_id);
        total_size += size;
    }
    println!();
    println!("Total size: {}", format_bytes(total_size));

    if dry_run {
        println!();
        println!("Dry run complete. No files were deleted.");
        return Ok(());
    }

    // Prompt for confirmation
    if !yes {
        let confirm = inquire::Confirm::new("Delete these files?")
            .with_default(false)
            .prompt()?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Delete files and track which ones succeeded for DB cleanup
    let mut deleted_count = 0;
    let mut actually_deleted: Vec<(String, String)> = Vec::new();

    for (repo_id, filename, file_path, _) in &orphaned_files {
        if let Err(e) = std::fs::remove_file(file_path) {
            tracing::warn!("Failed to delete {}: {}", file_path.display(), e);
        } else {
            deleted_count += 1;
            actually_deleted.push((repo_id.clone(), filename.clone()));
        }
    }

    // Clean up empty directories
    for (_, _, file_path, _) in &orphaned_files {
        if let Some(parent) = file_path.parent() {
            if parent
                .read_dir()
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
            {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }

    // Clean up orphaned model cards
    let mut orphaned_cards = Vec::new();
    if configs_dir.exists() {
        for entry in std::fs::read_dir(&configs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "toml") {
                continue;
            }
            // Try to load card and check if model dir still exists
            if let Ok(card) = ModelCard::load(&path) {
                if !card.model.source.is_empty() {
                    let model_dir = tama_core::models::repo_path(&models_dir, &card.model.source);
                    if !model_dir.exists()
                        || model_dir
                            .read_dir()
                            .map(|mut d| d.next().is_none())
                            .unwrap_or(true)
                    {
                        orphaned_cards.push(path.clone());
                    }
                }
            }
        }
    }

    for card_path in &orphaned_cards {
        let _ = std::fs::remove_file(card_path);
    }

    // Clean up DB records for actually deleted files
    for (repo_id, filename) in &actually_deleted {
        if let Some(record) = mgr.get_config_by_repo_id(repo_id)? {
            let _ = mgr.delete_file(record.id, filename);
        }
    }

    println!();
    println!("Deleted {} file(s).", deleted_count);
    if !orphaned_cards.is_empty() {
        println!("Deleted {} orphaned model card(s).", orphaned_cards.len());
    }

    Ok(())
}
