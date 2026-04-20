use anyhow::{Context, Result};
use koji_core::config::Config;
use koji_core::db::OpenResult;

/// Remove a server
pub fn cmd_server_rm(config: &Config, name: &str, force: bool) -> Result<()> {
    let db_dir = config
        .loaded_from
        .clone()
        .unwrap_or_else(|| koji_core::config::Config::config_dir().unwrap());
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    let model_configs = koji_core::db::load_model_configs(&conn)?;

    if !model_configs.contains_key(name) {
        anyhow::bail!("Server '{}' not found.", name);
    }

    // Check if a service is installed for this server
    let service_name = Config::service_name(name);
    let service_installed = {
        #[cfg(target_os = "windows")]
        {
            koji_core::platform::windows::query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(target_os = "linux")]
        {
            koji_core::platform::linux::auto_query_service(&service_name)
                .map(|s| s != "NOT_INSTALLED")
                .unwrap_or(true)
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = &service_name;
            false
        }
    };

    if service_installed {
        anyhow::bail!(
            "Server '{}' has an installed service '{}'. Remove it first with: koji service remove {}",
            name, service_name, name
        );
    }

    if !force {
        let confirm = inquire::Confirm::new(&format!("Remove model '{}'?", name))
            .with_default(false)
            .prompt()
            .context("Confirmation cancelled")?;
        if !confirm {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Remove from DB — look up by config name (HashMap key), not repo_id.
    // The HashMap key is the config's double-dash formatted name, which
    // matches `name` exactly since we already checked contains_key above.
    let model_id = model_configs.get(name).and_then(|mc| mc.db_id);
    if let Some(id) = model_id {
        koji_core::db::queries::delete_model_config(&conn, id)?;
    }

    println!("Model '{}' removed.", name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use koji_core::config::ModelConfig;

    /// Test the core fix: deletion uses config name (HashMap key) instead of repo_id.
    ///
    /// The original bug was: `get_all_model_configs` returns records with `repo_id`
    /// (e.g. "org/model-a"), but the function parameter `name` is a config key
    /// (e.g. "org--model-a"). Comparing `c.repo_id == name` never matches.
    /// The fix uses `load_model_configs` which returns a HashMap keyed by the
    /// double-dash formatted name, then looks up by that exact key.
    #[test]
    fn test_server_rm_deletes_by_config_name_not_repo_id() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();

        // Create two model configs.
        // Config "org--model-a" maps to repo "org/model-a" (repo_id in DB).
        // Config "org--model-b" maps to repo "org/model-b".
        let mc_a = ModelConfig {
            backend: "llama_cpp".to_string(),
            model: Some("org/model-a".to_string()),
            api_name: Some("org/model-a".to_string()),
            ..Default::default()
        };
        let mc_b = ModelConfig {
            backend: "llama_cpp".to_string(),
            model: Some("org/model-b".to_string()),
            api_name: Some("org/model-b".to_string()),
            ..Default::default()
        };

        let id_a = koji_core::db::save_model_config(&conn, "org--model-a", &mc_a).unwrap();
        let id_b = koji_core::db::save_model_config(&conn, "org--model-b", &mc_b).unwrap();

        // Verify both exist before deletion.
        let configs = koji_core::db::load_model_configs(&conn).unwrap();
        assert!(configs.contains_key("org--model-a"));
        assert!(configs.contains_key("org--model-b"));

        // Simulate the fix: look up by config name (HashMap key), not repo_id.
        let model_id_to_delete = configs.get("org--model-a").and_then(|mc| mc.db_id);
        assert_eq!(model_id_to_delete, Some(id_a));

        // Delete using the id found by config name lookup.
        koji_core::db::queries::delete_model_config(&conn, id_a).unwrap();

        // Verify org--model-a is gone but org--model-b remains.
        let configs = koji_core::db::load_model_configs(&conn).unwrap();
        assert!(!configs.contains_key("org--model-a"));
        assert!(configs.contains_key("org--model-b"));

        // The remaining config should still have the correct id.
        let server_b = configs.get("org--model-b").unwrap();
        assert_eq!(server_b.db_id, Some(id_b));
    }

    /// Test that looking up by repo_id would fail to find the right record.
    /// This demonstrates the original bug.
    #[test]
    fn test_repo_id_comparison_would_fail() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();

        let mc = ModelConfig {
            backend: "llama_cpp".to_string(),
            model: Some("org/model-a".to_string()),
            api_name: Some("org/model-a".to_string()),
            ..Default::default()
        };

        let id = koji_core::db::save_model_config(&conn, "org--model-a", &mc).unwrap();

        // The original buggy code would do:
        //   all_configs.iter().find(|c| c.repo_id == name)
        // where `name` is the config key "org--model-a".
        let all_configs = koji_core::db::queries::get_all_model_configs(&conn).unwrap();

        // repo_id in DB is "org/model-a", not "org--model-a"
        let buggy_match = all_configs.iter().find(|c| c.repo_id == "org--model-a");
        assert!(
            buggy_match.is_none(),
            "repo_id != config key: bug confirmed"
        );

        // The fix uses the HashMap from load_model_configs, keyed by double-dash name.
        let configs = koji_core::db::load_model_configs(&conn).unwrap();
        let correct_match = configs.get("org--model-a");
        assert!(
            correct_match.is_some(),
            "HashMap lookup by config key works"
        );
        assert_eq!(correct_match.unwrap().db_id, Some(id));
    }
}
