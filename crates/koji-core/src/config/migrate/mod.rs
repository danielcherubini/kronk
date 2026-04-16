use crate::config::Config;
use crate::models::card::ModelCard;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Strip stale `--mmproj <path>` entries from `args` in every model config.
///
/// These were written by the broken v1.15.0 frontend code that munged the
/// `args` field directly. The new path is `ModelConfig.mmproj` + automatic
/// `--mmproj` injection in `build_full_args`.
///
/// As a best-effort recovery, if a stale `--mmproj` argument is found and
/// `model_config.mmproj` is currently `None`, the function tries to find a quant entry in `model_config.quants` whose `file` matches the basename of
/// the stripped path. If found, that entry's key is set as the active
/// `mmproj`. This preserves the user's intent across the migration.
///
/// Returns `true` if any model config was modified.
pub fn cleanup_stale_mmproj_args(
    model_configs: &mut HashMap<String, crate::config::types::ModelConfig>,
) -> bool {
    let mut changed = false;

    for (model_config_id, model_config) in model_configs.iter_mut() {
        let mut i = 0;
        while i < model_config.args.len() {
            // Match three forms:
            //   1. "--mmproj <path>"             (single grouped token)
            //   2. "--mmproj=<path>"             (inline equals)
            //   3. "--mmproj" then "<path>"      (two separate tokens)
            let arg = &model_config.args[i];
            tracing::debug!(
                "Checking model '{}' arg[{}]: {:?}",
                model_config_id,
                i,
                &model_config.args[i]
            );

            let stripped_path: Option<String> = if let Some(rest) = arg.strip_prefix("--mmproj ") {
                Some(rest.to_string())
            } else if let Some(rest) = arg.strip_prefix("--mmproj=") {
                Some(rest.to_string())
            } else if arg == "--mmproj" && i + 1 < model_config.args.len() {
                Some(model_config.args[i + 1].clone())
            } else {
                None
            };

            let Some(path) = stripped_path else {
                i += 1;
                continue;
            };

            // Best-effort: recover the user's mmproj selection.
            if model_config.mmproj.is_none() {
                // Strip surrounding quotes that the migration may have left in.
                let path_clean = path.trim_matches(|c: char| c == '"' || c == '\'');
                let filename = path_clean
                    .replace('\\', "/")
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !filename.is_empty() {
                    tracing::debug!(
                        "Extracted filename '{}' from --mmproj path for model '{}'",
                        filename,
                        model_config_id
                    );
                    if let Some((key, q)) =
                        model_config.quants.iter().find(|(_, q)| q.file == filename)
                    {
                        model_config.mmproj = Some(key.clone());
                        tracing::info!(
                            "Recovered mmproj selection '{}' (file={:?}) from stale --mmproj arg",
                            key,
                            q.file
                        );
                    } else {
                        tracing::warn!(
                            "Could not find mmproj entry with file '{}' in model '{}' quants map",
                            filename,
                            model_config_id
                        );
                    }
                }
            }

            // Remove the stale token(s). Two-token form removes both;
            // grouped/inline-equals forms remove just the one.
            if arg == "--mmproj" {
                model_config.args.remove(i); // remove flag
                if i < model_config.args.len() {
                    model_config.args.remove(i); // remove value
                }
            } else {
                model_config.args.remove(i);
            }
            changed = true;
            // Don't increment i — the next entry has shifted into this slot.
        }
    }

    changed
}

#[allow(dead_code)]
pub fn migrate_model_cards_to_configs(config: &crate::config::Config) -> anyhow::Result<()> {
    let configs_dir = config.configs_dir()?;
    let models_dir = config
        .general
        .models_dir
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("models"));
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
                let new_filename = format!(
                    "{}--{}.toml",
                    company,
                    model_entry.file_name().to_string_lossy()
                );
                let new_path = configs_dir.join(&new_filename);
                if new_path.exists() {
                    let old_path = &old_card;
                    if let Err(e) = std::fs::remove_file(old_path) {
                        if e.kind() != std::io::ErrorKind::NotFound {
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

#[allow(dead_code)]
pub fn migrate_profiles_to_model_cards(config: &mut Config) -> anyhow::Result<()> {
    let configs_dir = config.configs_dir()?;
    let profiles_dir = config
        .loaded_from
        .as_ref()
        .map(|p: &std::path::PathBuf| p.join("profiles"));

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

                #[derive(serde::Deserialize)]
                struct TempProfileDef {
                    sampling: crate::profiles::SamplingParams,
                }

                let contents = std::fs::read_to_string(&path)?;
                match toml::from_str::<TempProfileDef>(&contents) {
                    Ok(profile_def) => profiles.push((name.clone(), profile_def.sampling)),
                    Err(e) => {
                        tracing::warn!("Skipping malformed profile file {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    if !configs_dir.exists() {
        std::fs::create_dir_all(&configs_dir)?;
    }

    let model_cards = std::fs::read_dir(&configs_dir)?;
    let mut _processed_cards = 0;

    for card_entry in model_cards {
        let card_entry = card_entry?;
        let card_path = card_entry.path();

        if !card_path.is_file() || card_path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        let mut card = crate::models::card::ModelCard::load(&card_path)?;

        for (profile_name, profile) in &profiles {
            if !card.sampling.contains_key(profile_name) {
                card.sampling.insert(profile_name.clone(), profile.clone());
            }
        }

        let original_card = crate::models::card::ModelCard::load(&card_path)?;
        if card != original_card {
            crate::models::card::save(&card, &card_path)?;
            _processed_cards += 1;
        }
    }

    if let Some(dir) = profiles_dir {
        let profiles_dir: &std::path::Path = dir.as_ref();
        if profiles_dir.exists() {
            let mut remaining = 0;
            for entry in std::fs::read_dir(profiles_dir)? {
                let entry = entry?;
                let path = entry.path();
                let ext = path.extension().and_then(|e| e.to_str());
                if ext == Some("toml") {
                    let name = path.file_stem().and_then(|s| s.to_str());
                    if let Some(name) = name {
                        if profiles.iter().any(|(p_name, _)| p_name == name) {
                            std::fs::remove_file(&path)?;
                        } else {
                            remaining += 1;
                        }
                    }
                }
            }
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

pub fn rename_legacy_directories(config_dir: &std::path::Path) -> anyhow::Result<()> {
    let legacy_map = [
        ("models.d", "models"),
        ("configs.d", "configs"),
        ("profiles.d", "profiles"),
    ];

    for (old, new) in legacy_map {
        let old_path = config_dir.join(old);
        let new_path = config_dir.join(new);

        if old_path.exists() && !new_path.exists() {
            tracing::info!("Renaming legacy directory '{}' to '{}'", old, new);
            if let Err(e) = std::fs::rename(&old_path, &new_path) {
                tracing::warn!("Failed to rename {} to {}: {}", old, new, e);
            }
        }
    }

    Ok(())
}
