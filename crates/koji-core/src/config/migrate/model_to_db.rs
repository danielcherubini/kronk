use crate::config::Config;
use crate::db::queries::get_all_model_configs;
use crate::db::{save_model_config, Connection};
use std::fs;

/// If the DB has no model_configs rows but koji.toml contains a [models] section,
/// import all models into DB, then save the config file (removing the [models] section).
/// Returns the number of models migrated (0 = nothing to do).
pub fn migrate_models_to_db(conn: &Connection, config: &mut Config) -> anyhow::Result<usize> {
    // 1. If the DB already has model_configs, we've already migrated.
    let existing_configs = get_all_model_configs(conn)?;
    if !existing_configs.is_empty() {
        return Ok(0);
    }

    // 2. Find where the config file is.
    let config_path = config
        .loaded_from
        .as_ref()
        .map(|p| p.join("config.toml"))
        .ok_or_else(|| anyhow::anyhow!("Config has no loaded_from path"))?;

    if !config_path.exists() {
        return Ok(0);
    }

    // 3. Read the raw TOML content.
    let content = fs::read_to_string(&config_path)?;
    let value: toml::Value = toml::from_str(&content)?;

    // 4. Extract the [models] section.
    let models_table = match value.get("models").and_then(|v| v.as_table()) {
        Some(table) => table,
        None => return Ok(0), // No [models] section found.
    };

    let migrated_count = models_table.len();
    tracing::info!(
        "Migrating {} models from koji.toml to database",
        migrated_count
    );

    // 5. Import each model config into the DB.
    for (key, val) in models_table {
        if let Ok(mc) = val.clone().try_into() {
            save_model_config(conn, key, &mc)?;
        } else {
            tracing::warn!(key = %key, "Failed to deserialize model config from TOML");
        }
    }

    // 6. Save the current Config struct back to the file.
    // Since Config.models is gone, this will overwrite koji.toml WITHOUT the [models] section.
    config.save()?;

    Ok(migrated_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ModelConfig;
    use crate::db::{open_in_memory, OpenResult};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_migrate_models_to_db_imports_all() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let toml_content = r#"
[general]
log_level = "info"

[models]
model1 = { backend = "llama_cpp", enabled = true }
model2 = { backend = "llama_cpp", enabled = false }
"#;
        fs::write(&config_path, toml_content).unwrap();

        let mut config = Config {
            loaded_from: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };

        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let migrated = migrate_models_to_db(&conn, &mut config).unwrap();

        assert_eq!(migrated, 2);

        // Verify DB has rows
        let all_configs = get_all_model_configs(&conn).unwrap();
        assert_eq!(all_configs.len(), 2);

        // Verify file no longer has [models]
        let final_content = fs::read_to_string(&config_path).unwrap();
        assert!(!final_content.contains("[models]"));
    }

    #[test]
    fn test_migrate_models_to_db_skips_if_db_has_rows() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let toml_content = r#"
[general]
log_level = "info"

[models]
model1 = { backend = "llama_cpp", enabled = true }
"#;
        fs::write(&config_path, toml_content).unwrap();

        let mut config = Config {
            loaded_from: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };

        let OpenResult { conn, .. } = open_in_memory().unwrap();

        // Pre-populate DB
        let mc = ModelConfig {
            backend: "llama_cpp".to_string(),
            ..Default::default()
        };
        save_model_config(&conn, "existing", &mc).unwrap();

        let migrated = migrate_models_to_db(&conn, &mut config).unwrap();

        assert_eq!(migrated, 0);

        // Verify file still has [models]
        let final_content = fs::read_to_string(&config_path).unwrap();
        assert!(final_content.contains("[models]"));
    }

    #[test]
    fn test_migrate_models_to_db_skips_if_no_models_section() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let toml_content = r#"
[general]
log_level = "info"
"#;
        fs::write(&config_path, toml_content).unwrap();

        let mut config = Config {
            loaded_from: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };

        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let migrated = migrate_models_to_db(&conn, &mut config).unwrap();

        assert_eq!(migrated, 0);
    }
}
