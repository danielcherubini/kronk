//! Model rename functionality for ProxyState.

use anyhow::Result;

use crate::db::queries::rename_active_model;
use crate::proxy::types::ProxyState;

impl ProxyState {
    /// Rename a model in the configuration and in-memory state.
    ///
    /// Logic:
    /// - Validates that `new_name` is not empty and differs from `old_name`
    /// - Takes a write lock on `self.config`:
    ///   - Checks `config.models` contains `old_name`
    ///   - Checks `config.models` does NOT contain `new_name` (error: "name already taken")
    ///   - Removes the entry at `old_name`, inserts at `new_name`
    ///   - Attempts `config.save()`
    ///   - If save fails: rollback — remove `new_name`, re-insert at `old_name`, return error
    /// - Takes a write lock on `self.models`:
    ///   - If `old_name` exists in the map, removes and re-inserts at `new_name`
    /// - DB update (best-effort): calls `rename_active_model(conn, old_name, new_name)` if db is available
    pub async fn rename_model(&self, old_name: &str, new_name: &str) -> Result<()> {
        // Validate inputs
        if new_name.is_empty() {
            anyhow::bail!("new name cannot be empty");
        }
        if old_name == new_name {
            anyhow::bail!("old name and new name must differ");
        }

        // Lock config and perform rename
        let mut config = self.config.write().await;

        // Check old name exists
        if !config.models.contains_key(old_name) {
            anyhow::bail!("model '{}' does not exist", old_name);
        }

        // Check new name doesn't exist
        if config.models.contains_key(new_name) {
            anyhow::bail!("model name '{}' already taken", new_name);
        }

        // Remove old entry
        let old_config = config.models.remove(old_name).unwrap();

        // Insert new entry
        config
            .models
            .insert(new_name.to_string(), old_config.clone());

        // Attempt to save config
        let save_result = config.save();
        if save_result.is_err() {
            // Rollback: remove new name, re-insert old name
            config.models.remove(new_name);
            config.models.insert(old_name.to_string(), old_config);

            return save_result;
        }

        drop(config);

        // Update in-memory models map
        {
            let mut models = self.models.write().await;
            if let Some(model_state) = models.remove(old_name) {
                models.insert(new_name.to_string(), model_state);
            }
        }

        // Best-effort DB update
        if let Some(conn) = self.open_db() {
            let _ = rename_active_model(&conn, old_name, new_name);
        }

        Ok(())
    }
}
