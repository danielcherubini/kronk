use crate::config::Config;
use crate::db::queries::get_all_model_configs;
use crate::db::{save_model_config, Connection};

/// If the DB has no model_configs rows but Config has non-empty models,
/// import all Config models into DB, then clear Config.models and save
/// the config file (removing the [models] section from koji.toml).
/// Returns the number of models migrated (0 = nothing to do).
pub fn migrate_models_to_db(conn: &Connection, config: &mut Config) -> anyhow::Result<usize> {
    // 1. If the DB has any model_configs rows, we've already migrated.
    let existing_configs = get_all_model_configs(conn)?;
    if !existing_configs.is_empty() {
        return Ok(0);
    }

    // 2. If there's nothing in the config file to migrate, we're done.
    if config.models.is_empty() {
        return Ok(0);
    }

    let migrated_count = config.models.len();
    tracing::info!(
        "Migrating {} models from koji.toml to database",
        migrated_count
    );

    // 3. Import each model config into the DB.
    for (key, mc) in &config.models {
        save_model_config(conn, key, mc)?;
    }

    // 4. Clear the models from the config struct and save to file.
    // Because we added `skip_serializing_if = "is_hashmap_empty"` to `Config.models`,
    // calling save() now will remove the [models] section from koji.toml.
    config.models.clear();
    config.save()?;

    Ok(migrated_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelConfig;
    use crate::db::{open_in_memory, OpenResult};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn setup_config_with_models() -> Config {
        let mut models = HashMap::new();
        models.insert(
            "model1".to_string(),
            ModelConfig {
                backend: "llama.cpp".to_string(),
                ..Default::default()
            },
        );
        models.insert(
            "model2".to_string(),
            ModelConfig {
                backend: "llama.cpp".to_string(),
                ..Default::default()
            },
        );

        Config {
            models,
            loaded_from: Some(PathBuf::from("test_config")),
            ..Default::default()
        }
    }

    #[test]
    fn test_migrate_models_to_db_imports_all() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let mut config = setup_config_with_models();

        let migrated = migrate_models_to_db(&conn, &mut config).unwrap();

        assert_eq!(migrated, 2);
        assert!(config.models.is_empty());

        let all_configs = get_all_model_configs(&conn).unwrap();
        assert_eq!(all_configs.len(), 2);
    }

    #[test]
    fn test_migrate_models_to_db_skips_if_db_has_rows() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let mut config = setup_config_with_models();

        // Pre-populate DB
        let mc = ModelConfig {
            backend: "llama.cpp".to_string(),
            ..Default::default()
        };
        save_model_config(&conn, "existing", &mc).unwrap();

        let migrated = migrate_models_to_db(&conn, &mut config).unwrap();

        assert_eq!(migrated, 0);
        assert!(!config.models.is_empty());
    }

    #[test]
    fn test_migrate_models_to_db_skips_if_config_empty() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let mut config = Config::default();

        let migrated = migrate_models_to_db(&conn, &mut config).unwrap();

        assert_eq!(migrated, 0);
    }
}
