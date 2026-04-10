use crate::config::Config;
use crate::models::card::ModelCard;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Migrates model card data into the unified ModelConfig.
pub fn migrate_cards_to_unified_config(config: &mut Config) -> anyhow::Result<()> {
    // Derive api_name from model field (HF repo ID) for ALL models, even if configs/ dir is missing
    // This ensures api_name is always populated for models with a model field
    for model_config in config.models.values_mut() {
        if model_config.api_name.is_none() {
            if let Some(repo_id) = &model_config.model {
                model_config.api_name = Some(repo_id.clone());
            }
        }
    }

    let configs_dir = config.configs_dir()?;
    if !configs_dir.exists() {
        return Ok(());
    }

    // 2. Back up config.toml
    let dir = config
        .loaded_from
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Config has no loaded_from path"))?;
    let config_path = dir.join("config.toml");
    let backup_path = config_path.with_extension("toml.pre-unified-migration");
    if !backup_path.exists() {
        fs::copy(&config_path, &backup_path)?;
    }

    // 3. Read ALL card files into memory first
    let mut card_data: HashMap<String, ModelCard> = HashMap::new();
    for entry in fs::read_dir(&configs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                let filename = file_name.to_string();
                let card = ModelCard::load(&path)?;
                card_data.insert(filename, card);
            } else {
                tracing::warn!("Skipping file with invalid name: {}", path.display());
                continue;
            }
        }
    }

    // 3.5. Migrate card model.name to use full repo ID instead of truncated name
    // Cards created before this fix had model.name set to just the last part after "/"
    // We update them to use model.source (which has the full repo ID)
    for (filename, card) in card_data.iter_mut() {
        if card.model.name != card.model.source {
            tracing::info!(
                "Migrating card {}: updating model.name from '{}' to '{}'",
                filename,
                card.model.name,
                card.model.source
            );
            card.model.name = card.model.source.clone();
            // Save the updated card back to disk
            let card_path = configs_dir.join(filename);
            if let Err(e) = card.save(&card_path) {
                tracing::warn!("Failed to save migrated card {}: {}", filename, e);
            }
        }
    }

    // 4. For each (key, model_config) in config.models
    let mut migrated_count = 0;
    for model_config in config.models.values_mut() {
        // First, handle sampling resolution for ALL models (with or without model field)
        if model_config.sampling.is_none() {
            if let Some(profile_name) = &model_config.profile {
                // look up card.sampling[profile_name] if we have the card, otherwise use sampling_templates
                if let Some(repo_id) = &model_config.model {
                    let filename = repo_id.replace('/', "--") + ".toml";
                    if let Some(card) = card_data.get(&filename) {
                        if let Some(card_sampling) = card.sampling.get(profile_name) {
                            model_config.sampling = Some(card_sampling.clone());
                        } else if let Some(template_sampling) =
                            config.sampling_templates.get(profile_name)
                        {
                            model_config.sampling = Some(template_sampling.clone());
                        }
                    } else if let Some(template_sampling) =
                        config.sampling_templates.get(profile_name)
                    {
                        model_config.sampling = Some(template_sampling.clone());
                    }
                } else {
                    // No model field, use sampling_templates directly
                    if let Some(template_sampling) = config.sampling_templates.get(profile_name) {
                        model_config.sampling = Some(template_sampling.clone());
                    }
                }
            } else {
                // profile is None, check if card has "coding" entry
                if let Some(repo_id) = &model_config.model {
                    let filename = repo_id.replace('/', "--") + ".toml";
                    if let Some(card) = card_data.get(&filename) {
                        if let Some(card_sampling) = card.sampling.get("coding") {
                            model_config.sampling = Some(card_sampling.clone());
                        }
                    }
                }
            }
        }

        // Now handle card-based migrations for models with a model field
        if let Some(repo_id) = &model_config.model {
            let filename = repo_id.replace('/', "--") + ".toml";

            // If there's a card, migrate card data
            if let Some(card) = card_data.get(&filename) {
                // Set gpu_layers from card.model.default_gpu_layers if gpu_layers is None
                if model_config.gpu_layers.is_none() {
                    model_config.gpu_layers = card.model.default_gpu_layers;
                }
                // Set context_length from card.model.default_context_length if None
                if model_config.context_length.is_none() {
                    model_config.context_length = card.model.default_context_length;
                }
                // For each (quant_name, quant_info) in card.quants, insert into model_config.quants if key not already present.
                for (quant_name, quant_info) in &card.quants {
                    if !model_config.quants.contains_key(quant_name) {
                        model_config.quants.insert(
                            quant_name.clone(),
                            crate::config::types::QuantEntry {
                                file: quant_info.file.clone(),
                                kind: quant_info.kind,
                                size_bytes: quant_info.size_bytes,
                                context_length: quant_info.context_length,
                            },
                        );
                    }
                }

                // Backfill: any quant entry whose kind is the default `Model`
                // but whose filename matches an mmproj pattern should be tagged
                // as `Mmproj`. This handles configs written before the kind
                // field existed (e.g. v1.15.0 broken mmproj support).
                for entry in model_config.quants.values_mut() {
                    if entry.kind == crate::config::QuantKind::Model {
                        let detected = crate::config::QuantKind::from_filename(&entry.file);
                        if detected != crate::config::QuantKind::Model {
                            entry.kind = detected;
                        }
                    }
                }
            }

            // Increment migrated_count if we migrated from a card or resolved a profile
            if card_data.contains_key(&filename) || model_config.sampling.is_some() {
                migrated_count += 1;
            }
        }
    }

    // 5. Save config
    config.save()?;

    // 6. Delete migrated card files (best-effort)
    for filename in card_data.keys() {
        let path = configs_dir.join(filename);
        if path.exists() {
            if let Err(e) = fs::remove_file(&path) {
                tracing::warn!(
                    "Failed to remove migrated card file {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }

    // 7. If configs/ directory is now empty, remove it.
    let mut empty = true;
    if let Ok(entries) = fs::read_dir(&configs_dir) {
        if entries.count() > 0 {
            empty = false;
        }
    }
    if empty {
        let _ = fs::remove_dir(&configs_dir);
    }

    if migrated_count > 0 {
        tracing::info!(
            "Migrated {} model cards to unified ModelConfig",
            migrated_count
        );
    }

    Ok(())
}

/// Strip stale `--mmproj <path>` entries from `args` in every model config.
///
/// These were written by the broken v1.15.0 frontend code that munged the
/// `args` field directly. The new path is `ModelConfig.mmproj` + automatic
/// `--mmproj` injection in `build_full_args`.
///
/// As a best-effort recovery, if a stale `--mmproj` argument is found and
/// `model_config.mmproj` is currently `None`, the function tries to find a
/// quant entry in `model_config.quants` whose `file` matches the basename of
/// the stripped path. If found, that entry's key is set as the active
/// `mmproj`. This preserves the user's intent across the migration.
///
/// Returns `true` if any model config was modified (so the caller can persist
/// the cleanup to disk).
pub fn cleanup_stale_mmproj_args(config: &mut Config) -> bool {
    let mut changed = false;

    for (model_config_id, model_config) in config.models.iter_mut() {
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

#[cfg(test)]
mod tests;
