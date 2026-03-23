pub fn migrate_model_cards_to_configs_d(config: &crate::config::Config) -> anyhow::Result<()> {
    let configs_dir = config.configs_dir()?;
    let models_dir = config.models_dir()?;
    if !models_dir.exists() {
        return Ok(());
    }
    let mut migrated = false;
    for company_entry in std::fs::read_dir(&models_dir)? {
        let company_entry = company_entry?;
        if !company_entry.path().is_dir() {
            continue;
        }
        let company = company_entry.file_name().to_string_lossy().to_string();
        for model_entry in std::fs::read_dir(company_entry.path())? {
            let model_entry = model_entry?;
            let old_card = model_entry.path().join("model.toml");
            if old_card.exists() {
                let model_name = model_entry.file_name().to_string_lossy().to_string();
                let new_filename = format!("{}--{}.toml", company, model_name);
                let new_path = configs_dir.join(&new_filename);
                // Skip if already migrated - but clean up legacy file instead of leaving it behind
                if new_path.exists() {
                    let old_path = &old_card;
                    if let Err(e) = std::fs::remove_file(old_path) {
                        if e.kind() == std::io::ErrorKind::NotFound {
                            // Already removed, continue
                        } else {
                            tracing::warn!(
                                "Failed to remove legacy model.toml {}: {}",
                                old_path.display(),
                                e
                            );
                        }
                    }
                    continue;
                }
                std::fs::create_dir_all(&configs_dir)?;
                std::fs::copy(&old_card, &new_path)?;
                if let Err(e) = std::fs::remove_file(&old_card) {
                    tracing::warn!(
                        "Failed to remove old model card {}: {}",
                        old_card.display(),
                        e
                    );
                }
                migrated = true;
            }
        }
    }
    if migrated {
        tracing::info!("Migrated model cards to {}", configs_dir.display());
    }
    Ok(())
}
