use crate::config::Config;
use std::path::PathBuf;

pub fn migrate_model_cards_to_configs_d(config: &crate::config::Config) -> anyhow::Result<()> {
    let configs_dir = config.configs_dir()?;
    let models_dir = config
        .general
        .models_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models.d"));
    if !models_dir.exists() {
        return Ok(());
    }
    let mut migrated = false;
    for company_entry in std::fs::read_dir(models_dir)? {
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

pub fn migrate_profiles_to_model_cards(config: &mut Config) -> anyhow::Result<()> {
    let configs_dir = config.configs_dir()?;
    let profiles_dir = config
        .loaded_from
        .as_ref()
        .map(|p: &std::path::PathBuf| p.join("profiles.d"));
    let models_dir = &config
        .general
        .models_dir
        .clone()
        .unwrap_or_else(|| "models.d".to_string());

    // Collect profiles from profiles.d/
    let mut profiles = Vec::new();
    if let Some(dir) = &profiles_dir {
        let profiles_dir: &std::path::Path = dir.as_ref();
        if profiles_dir.exists() {
            for entry in std::fs::read_dir(profiles_dir)? {
                let entry = entry?;
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str());
                if ext != Some("toml") {
                    continue;
                }
                let name = path.file_stem().unwrap().to_string_lossy().to_string();

                // Parse as ProfileDef - collect all profiles (no built-in skipping)
                if let Ok(profile_def) =
                    toml::from_str::<crate::profiles::ProfileDef>(&std::fs::read_to_string(&path)?)
                {
                    profiles.push((name.clone(), profile_def.sampling));
                }
            }
        }
    }

    // Collect profiles from config.custom_profiles (always custom)
    if let Some(custom_profiles) = &config.custom_profiles {
        for (name, sampling) in custom_profiles {
            profiles.push((name.clone(), sampling.clone()));
        }
    }

    // Load all model cards from configs.d/
    let model_cards = std::fs::read_dir(&configs_dir)?;
    let mut processed_cards = 0;

    for card_entry in model_cards {
        let card_entry = card_entry?;
        let card_path = card_entry.path();

        // Load existing card
        let mut card = crate::models::card::ModelCard::load(&card_path)?;

        // For each collected profile, insert if key doesn't exist
        for (profile_name, profile) in &profiles {
            let key = format!("sampling.{}", profile_name);
            if !card.sampling.contains_key(profile_name) {
                card.sampling.insert(profile_name.clone(), profile.clone());
            }
        }

        // Save each modified card
        if card != crate::models::card::ModelCard::load(&card_path)? {
            crate::models::card::save(&card, &card_path)?;
            processed_cards += 1;
        }
    }

    // Remove processed .toml files from profiles.d/ and rmdir if empty
    if let Some(dir) = profiles_dir {
        let profiles_dir: &std::path::Path = dir.as_ref();
        if profiles_dir.exists() {
            let mut remaining = 0;
            for entry in std::fs::read_dir(profiles_dir)? {
                let entry = entry?;
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str());
                if ext == Some("toml") {
                    remaining += 1;
                }
            }
            if remaining == 0 {
                std::fs::remove_dir_all(profiles_dir)?;
            }
        }
    }

    // Set custom_profiles = None and save config
    config.custom_profiles = None;
    config.save()?;

    Ok(())
}
