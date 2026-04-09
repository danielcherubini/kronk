use crate::config::Config;
use crate::models::card::ModelCard;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Migrates model card data into the unified ModelConfig.
pub fn migrate_cards_to_unified_config(config: &mut Config) -> anyhow::Result<()> {
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

    // 4. For each (key, model_config) in config.models
    let mut migrated_count = 0;
    for model_config in config.models.values_mut() {
        // Derive api_name from model field (HF repo ID) if not set
        // This applies to ALL models, not just those with card files
        if model_config.api_name.is_none() {
            if let Some(repo_id) = &model_config.model {
                model_config.api_name = Some(repo_id.clone());
            }
        }

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

                if let Ok(profile_def) =
                    toml::from_str::<TempProfileDef>(&std::fs::read_to_string(&path)?)
                {
                    profiles.push((name.clone(), profile_def.sampling));
                } else {
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
mod tests {
    use super::*;
    use crate::profiles::SamplingParams;
    use std::collections::{BTreeMap, HashMap};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_migrate_cards_to_unified() {
        let dir = tempdir().unwrap();
        let config_dir = dir.path();

        // Create configs/ directory
        let configs_dir = config_dir.join("configs");
        fs::create_dir_all(&configs_dir).unwrap();

        // Create config.toml
        let config_path = config_dir.join("config.toml");
        let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/repo"
quant = "Q4_K_M"
"#;
        fs::write(&config_path, config_toml).unwrap();

        // Create model card: configs/org--repo.toml
        let card_path = configs_dir.join("org--repo.toml");
        let card_toml = r#"
[model]
name = "TestModel"
source = "org/repo"
default_gpu_layers = 99
default_context_length = 8192

[quants.Q4_K_M]
file = "model-Q4_K_M.gguf"
size_bytes = 4000000000

[sampling.coding]
temperature = 0.2
top_k = 40
"#;
        fs::write(&card_path, card_toml).unwrap();

        // Setup config object
        let mut config = Config {
            general: crate::config::types::General::default(),
            backends: HashMap::new(),
            models: {
                let mut m = HashMap::new();
                let model = crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    args: vec![],
                    sampling: None,
                    model: Some("org/repo".to_string()),
                    quant: Some("Q4_K_M".to_string()),
                    mmproj: None,
                    port: None,
                    health_check: None,
                    enabled: true,
                    context_length: None,
                    profile: None,
                    api_name: None,
                    gpu_layers: None,
                    quants: BTreeMap::new(),
                };
                m.insert("test-model".to_string(), model);
                m
            },
            supervisor: crate::config::types::Supervisor::default(),
            sampling_templates: {
                let mut t = HashMap::new();
                t.insert(
                    "coding".to_string(),
                    SamplingParams {
                        temperature: Some(0.3),
                        top_p: Some(0.9),
                        top_k: Some(50),
                        min_p: Some(0.05),
                        presence_penalty: Some(0.1),
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                );
                t
            },
            proxy: crate::config::types::ProxyConfig::default(),
            loaded_from: Some(config_dir.to_path_buf()),
        };

        // Run migration
        migrate_cards_to_unified_config(&mut config).unwrap();

        // Assertions
        let model_config = config.models.get("test-model").unwrap();
        assert_eq!(model_config.api_name, Some("org/repo".to_string()));
        assert_eq!(model_config.gpu_layers, Some(99));
        assert_eq!(model_config.context_length, Some(8192));
        assert_eq!(
            model_config.quants.get("Q4_K_M").unwrap().file,
            "model-Q4_K_M.gguf"
        );
        assert_eq!(
            model_config.sampling.as_ref().unwrap().temperature,
            Some(0.2)
        );

        // Card file should be gone
        assert!(!card_path.exists());

        // Backup file should exist
        assert!(config_dir
            .join("config.toml.pre-unified-migration")
            .exists());
    }

    #[test]
    fn test_migrate_idempotent() {
        let dir = tempdir().unwrap();
        let config_dir = dir.path();

        // Create configs/ directory
        let configs_dir = config_dir.join("configs");
        fs::create_dir_all(&configs_dir).unwrap();

        // Create config.toml
        let config_path = config_dir.join("config.toml");
        let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/repo"
quant = "Q4_K_M"
"#;
        fs::write(&config_path, config_toml).unwrap();

        // Create model card
        let card_path = configs_dir.join("org--repo.toml");
        let card_toml = r#"
[model]
name = "TestModel"
source = "org/repo"
default_gpu_layers = 99
default_context_length = 8192

[quants.Q4_K_M]
file = "model-Q4_K_M.gguf"
size_bytes = 4000000000

[sampling.coding]
temperature = 0.2
top_k = 40
"#;
        fs::write(&card_path, card_toml).unwrap();

        // Setup config object
        let mut config = Config {
            general: crate::config::types::General::default(),
            backends: HashMap::new(),
            models: {
                let mut m = HashMap::new();
                let model = crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    args: vec![],
                    sampling: None,
                    model: Some("org/repo".to_string()),
                    quant: Some("Q4_K_M".to_string()),
                    mmproj: None,
                    port: None,
                    health_check: None,
                    enabled: true,
                    context_length: None,
                    profile: None,
                    api_name: None,
                    gpu_layers: None,
                    quants: BTreeMap::new(),
                };
                m.insert("test-model".to_string(), model);
                m
            },
            supervisor: crate::config::types::Supervisor::default(),
            sampling_templates: {
                let mut t = HashMap::new();
                t.insert(
                    "coding".to_string(),
                    SamplingParams {
                        temperature: Some(0.3),
                        top_p: Some(0.9),
                        top_k: Some(50),
                        min_p: Some(0.05),
                        presence_penalty: Some(0.1),
                        frequency_penalty: None,
                        repeat_penalty: None,
                    },
                );
                t
            },
            proxy: crate::config::types::ProxyConfig::default(),
            loaded_from: Some(config_dir.to_path_buf()),
        };

        // First migration
        migrate_cards_to_unified_config(&mut config).unwrap();

        // Verify migration happened
        let model_config = config.models.get("test-model").unwrap();
        assert_eq!(model_config.api_name, Some("org/repo".to_string()));
        assert_eq!(model_config.gpu_layers, Some(99));
        assert_eq!(model_config.context_length, Some(8192));
        assert!(!card_path.exists());

        // Second migration - should be no-op
        migrate_cards_to_unified_config(&mut config).unwrap();

        // Verify nothing changed
        let model_config = config.models.get("test-model").unwrap();
        assert_eq!(model_config.api_name, Some("org/repo".to_string()));
        assert_eq!(model_config.gpu_layers, Some(99));
        assert_eq!(model_config.context_length, Some(8192));
    }

    #[test]
    fn test_migrate_preserves_existing_quants() {
        let dir = tempdir().unwrap();
        let config_dir = dir.path();

        // Create configs/ directory
        let configs_dir = config_dir.join("configs");
        fs::create_dir_all(&configs_dir).unwrap();

        // Create config.toml with existing quants
        let config_path = config_dir.join("config.toml");
        let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/repo"
quant = "Q4_K_M"
"#;
        fs::write(&config_path, config_toml).unwrap();

        // Create model card with different quant data
        let card_path = configs_dir.join("org--repo.toml");
        let card_toml = r#"
[model]
name = "TestModel"
source = "org/repo"

[quants.Q4_K_M]
file = "different-file.gguf"
size_bytes = 9999999999

[quants.Q8_0]
file = "model-Q8_0.gguf"
size_bytes = 8000000000
"#;
        fs::write(&card_path, card_toml).unwrap();

        // Setup config object with existing quants
        let mut config = Config {
            general: crate::config::types::General::default(),
            backends: HashMap::new(),
            models: {
                let mut m = HashMap::new();
                let model = crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    args: vec![],
                    sampling: None,
                    model: Some("org/repo".to_string()),
                    quant: Some("Q4_K_M".to_string()),
                    mmproj: None,
                    port: None,
                    health_check: None,
                    enabled: true,
                    context_length: None,
                    profile: None,
                    api_name: None,
                    gpu_layers: None,
                    quants: {
                        let mut quants = std::collections::BTreeMap::new();
                        quants.insert(
                            "Q4_K_M".to_string(),
                            crate::config::types::QuantEntry {
                                file: "existing-model-Q4_K_M.gguf".to_string(),
                                kind: Default::default(),
                                size_bytes: Some(1000000000),
                                context_length: None,
                            },
                        );
                        quants
                    },
                };
                m.insert("test-model".to_string(), model);
                m
            },
            supervisor: crate::config::types::Supervisor::default(),
            sampling_templates: HashMap::new(),
            proxy: crate::config::types::ProxyConfig::default(),
            loaded_from: Some(config_dir.to_path_buf()),
        };

        // Run migration
        migrate_cards_to_unified_config(&mut config).unwrap();

        // Verify existing quant was NOT overwritten
        let model_config = config.models.get("test-model").unwrap();
        let existing_quant = model_config.quants.get("Q4_K_M").unwrap();
        assert_eq!(existing_quant.file, "existing-model-Q4_K_M.gguf");
        assert_eq!(existing_quant.size_bytes, Some(1000000000));

        // New quant from card should be added
        assert!(model_config.quants.contains_key("Q8_0"));
        assert_eq!(
            model_config.quants.get("Q8_0").unwrap().file,
            "model-Q8_0.gguf"
        );

        // api_name should be set from model field (HF repo ID)
        assert_eq!(model_config.api_name, Some("org/repo".to_string()));
    }

    /// Helper that builds a minimal `ModelConfig` with a quants map containing
    /// a Mmproj entry, used by the cleanup tests.
    fn build_test_model_config_with_mmproj(
        args: Vec<&str>,
        mmproj: Option<&str>,
    ) -> crate::config::types::ModelConfig {
        let mut quants = BTreeMap::new();
        quants.insert(
            "mmproj-F16".to_string(),
            crate::config::types::QuantEntry {
                file: "mmproj-F16.gguf".to_string(),
                kind: crate::config::QuantKind::Mmproj,
                size_bytes: None,
                context_length: None,
            },
        );
        quants.insert(
            "Q4_K_M".to_string(),
            crate::config::types::QuantEntry {
                file: "model-Q4_K_M.gguf".to_string(),
                kind: crate::config::QuantKind::Model,
                size_bytes: None,
                context_length: None,
            },
        );
        crate::config::types::ModelConfig {
            backend: "llama_cpp".to_string(),
            args: args.into_iter().map(String::from).collect(),
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: mmproj.map(String::from),
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            api_name: None,
            gpu_layers: None,
            quants,
        }
    }

    #[test]
    fn cleanup_strips_grouped_mmproj_arg_and_recovers_selection() {
        let mut config = Config::default();
        config.models.insert(
            "m".to_string(),
            build_test_model_config_with_mmproj(
                vec![
                    "-fa 1",
                    "--mmproj models/org/repo/mmproj-F16.gguf",
                    "-c 4096",
                ],
                None,
            ),
        );

        let changed = cleanup_stale_mmproj_args(&mut config);

        assert!(changed);
        let m = &config.models["m"];
        assert_eq!(m.args, vec!["-fa 1".to_string(), "-c 4096".to_string()]);
        // Selection recovered from filename match.
        assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
    }

    #[test]
    fn cleanup_strips_two_token_mmproj_arg() {
        let mut config = Config::default();
        config.models.insert(
            "m".to_string(),
            build_test_model_config_with_mmproj(
                vec![
                    "-fa 1",
                    "--mmproj",
                    "models/org/repo/mmproj-F16.gguf",
                    "-c 4096",
                ],
                None,
            ),
        );

        let changed = cleanup_stale_mmproj_args(&mut config);

        assert!(changed);
        let m = &config.models["m"];
        assert_eq!(m.args, vec!["-fa 1".to_string(), "-c 4096".to_string()]);
        assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
    }

    #[test]
    fn cleanup_strips_inline_equals_mmproj_arg() {
        let mut config = Config::default();
        config.models.insert(
            "m".to_string(),
            build_test_model_config_with_mmproj(
                vec!["-fa 1", "--mmproj=models/org/repo/mmproj-F16.gguf"],
                None,
            ),
        );

        let changed = cleanup_stale_mmproj_args(&mut config);

        assert!(changed);
        let m = &config.models["m"];
        assert_eq!(m.args, vec!["-fa 1".to_string()]);
        assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
    }

    #[test]
    fn cleanup_preserves_existing_mmproj_field() {
        // If the user has already set mmproj via the editor, the recovered
        // value from a stale arg must NOT overwrite it.
        let mut config = Config::default();
        config.models.insert(
            "m".to_string(),
            build_test_model_config_with_mmproj(
                vec!["--mmproj models/org/repo/some-other-name.gguf"],
                Some("mmproj-F16"),
            ),
        );

        let changed = cleanup_stale_mmproj_args(&mut config);

        assert!(changed);
        let m = &config.models["m"];
        assert!(m.args.is_empty());
        // Pre-existing selection preserved.
        assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
    }

    #[test]
    fn cleanup_returns_false_when_no_stale_args() {
        let mut config = Config::default();
        config.models.insert(
            "m".to_string(),
            build_test_model_config_with_mmproj(vec!["-fa 1", "-c 4096"], Some("mmproj-F16")),
        );

        let changed = cleanup_stale_mmproj_args(&mut config);

        assert!(!changed);
        let m = &config.models["m"];
        assert_eq!(m.args, vec!["-fa 1".to_string(), "-c 4096".to_string()]);
    }

    #[test]
    fn test_api_name_derived_from_model_field_without_card() {
        let dir = tempdir().unwrap();
        let config_dir = dir.path();

        // Create configs/ directory (empty - no card files)
        let configs_dir = config_dir.join("configs");
        fs::create_dir_all(&configs_dir).unwrap();

        // Create config.toml with api_name: None and model: Some(...)
        let config_path = config_dir.join("config.toml");
        let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/model-name"
quant = "Q4_K_M"
"#;
        fs::write(&config_path, config_toml).unwrap();

        // Setup config object with api_name: None and model: Some("org/model-name")
        let mut config = Config {
            general: crate::config::types::General::default(),
            backends: HashMap::new(),
            models: {
                let mut m = HashMap::new();
                let model = crate::config::ModelConfig {
                    backend: "llama_cpp".to_string(),
                    args: vec![],
                    sampling: None,
                    model: Some("org/model-name".to_string()),
                    quant: Some("Q4_K_M".to_string()),
                    mmproj: None,
                    port: None,
                    health_check: None,
                    enabled: true,
                    context_length: None,
                    profile: None,
                    api_name: None, // This is what should be derived during migration
                    gpu_layers: None,
                    quants: BTreeMap::new(),
                };
                m.insert("test-model".to_string(), model);
                m
            },
            supervisor: crate::config::types::Supervisor::default(),
            sampling_templates: HashMap::new(),
            proxy: crate::config::types::ProxyConfig::default(),
            loaded_from: Some(config_dir.to_path_buf()),
        };

        // Run migration (no card files exist)
        migrate_cards_to_unified_config(&mut config).unwrap();

        // After migration, api_name should be derived from model field
        let model_config = config.models.get("test-model").unwrap();
        assert_eq!(model_config.api_name, Some("org/model-name".to_string()));
    }
}
