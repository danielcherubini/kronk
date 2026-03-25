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
                let name = match path.file_stem() {
                    Some(stem) => stem.to_string_lossy().to_string(),
                    None => continue,
                };

                // Parse as a temporary struct to extract sampling params
                #[derive(serde::Deserialize)]
                struct TempProfileDef {
                    sampling: crate::profiles::SamplingParams,
                }

                if let Ok(profile_def) =
                    toml::from_str::<TempProfileDef>(&std::fs::read_to_string(&path)?)
                {
                    profiles.push((name.clone(), profile_def.sampling));
                } else {
                    // Skip malformed profile files with a warning
                    match toml::from_str::<TempProfileDef>(&std::fs::read_to_string(&path)?) {
                        Ok(_) => unreachable!(),
                        Err(e) => {
                            tracing::warn!(
                                "Skipping malformed profile file {}: {}",
                                path.display(),
                                e
                            );
                        }
                    }
                }
            }
        }
    }

    // Load all model cards from configs.d/
    // Ensure configs_dir exists before attempting to read
    if !configs_dir.exists() {
        std::fs::create_dir_all(&configs_dir)?;
    }

    let model_cards = std::fs::read_dir(&configs_dir)?;
    let mut _processed_cards = 0; // Marked as unused, will be removed if no longer needed

    for card_entry in model_cards {
        let card_entry = card_entry?;
        let card_path = card_entry.path();

        // Ensure it's a file with .toml extension
        if !card_path.is_file() || card_path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        // Load existing card
        let mut card = crate::models::card::ModelCard::load(&card_path)?;

        // For each collected profile, insert if key doesn't exist
        for (profile_name, profile) in &profiles {
            // Check if profile_name already exists in card.sampling
            if !card.sampling.contains_key(profile_name) {
                card.sampling.insert(profile_name.clone(), profile.clone());
            }
        }

        // Save each modified card
        // Check if card was actually modified before saving
        let original_card = crate::models::card::ModelCard::load(&card_path)?;
        if card != original_card {
            crate::models::card::save(&card, &card_path)?;
            _processed_cards += 1;
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
                    // Only count actual TOML files that were part of the old profiles
                    // Ensure these files are indeed empty before removal
                    let name = path.file_stem().and_then(|s| s.to_str());
                    if let Some(name) = name {
                        if profiles.iter().any(|(p_name, _)| p_name == name) {
                            // Check if this profile was migrated. If so, remove the file.
                            std::fs::remove_file(&path)?;
                        } else {
                            remaining += 1; // Keep if not migrated, or is some other TOML
                        }
                    }
                }
            }
            // Only remove the directory if it's completely empty of TOML files
            if remaining == 0 {
                if let Ok(entries) = std::fs::read_dir(profiles_dir) {
                    if entries.count() == 0 {
                        std::fs::remove_dir_all(profiles_dir)?;
                    }
                }
            }
        }
    }

    config.save()?;

    Ok(())
}
