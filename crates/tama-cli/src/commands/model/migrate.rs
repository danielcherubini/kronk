use anyhow::Result;
use tama_core::config::Config;
use tama_core::models::ModelManager;

pub(super) fn cmd_migrate(config: &Config) -> Result<()> {
    let db_dir = tama_core::config::Config::config_dir()?;
    let mgr = ModelManager::open(&db_dir)?;

    // We need a mutable config to call migrate_models_to_db.
    let mut mutable_config = config.clone();

    let migrated = tama_core::config::migrate::model_to_db::migrate_models_to_db(
        mgr.conn(),
        &mut mutable_config,
    )?;

    if migrated == 0 {
        println!("Nothing to migrate.");
    } else {
        println!("Successfully migrated {} models to the database.", migrated);
    }

    Ok(())
}
