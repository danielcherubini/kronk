mod forward;
mod handlers;
pub mod koji_handlers;
mod lifecycle;
pub mod process;
pub mod pull_jobs;
mod rename;
pub mod server;
mod state;
mod status;
mod types;

pub use forward::forward_request;
pub use handlers::{
    handle_chat_completions, handle_fallback, handle_get_model, handle_health, handle_list_models,
    handle_metrics, handle_status, handle_stream_chat_completions, json_error_response,
};
pub use process::{check_health, force_kill_process, is_process_alive, kill_process, override_arg};
pub use server::ProxyServer;
pub use types::{ModelState, ProxyMetrics, ProxyState};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[tokio::test]
    async fn test_proxy_state_new() {
        let config = Config::default();
        let state = ProxyState::new(config.clone(), None);
        assert!(state.models.read().await.is_empty());
        assert_eq!(
            state.config.read().await.proxy.idle_timeout_secs,
            config.proxy.idle_timeout_secs
        );
    }

    #[tokio::test]
    async fn test_no_available_server_for_unknown_model() {
        let config = Config::default();
        let state = ProxyState::new(config, None);
        let result = state.get_available_server_for_model("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_build_status_response() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        let response = state.build_status_response().await;

        // VRAM may or may not be present depending on GPU availability
        let vram = response.get("vram");
        assert!(vram.is_some(), "vram key should be present (even if null)");

        // idle_timeout_secs at top level per spec
        assert!(response.get("idle_timeout_secs").is_some());

        // models is an object keyed by model name
        let models = response.get("models").unwrap();
        assert!(models.is_object());

        let metrics = response.get("metrics").unwrap();
        assert!(metrics.is_object());
    }

    #[tokio::test]
    async fn test_build_status_response_model_fields() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        let response = state.build_status_response().await;

        // models is an object, default config has a "default" model
        let models = response.get("models").unwrap().as_object().unwrap();
        assert!(
            !models.is_empty(),
            "default config should have at least one model"
        );

        let (_, first_model) = models.iter().next().unwrap();

        // Per spec: flat fields, not nested in runtime
        assert!(first_model.get("backend").is_some());
        assert!(first_model.get("backend_path").is_some());
        assert!(first_model.get("enabled").is_some());
        assert!(first_model.get("loaded").is_some());
        // Unloaded model should have loaded=false
        assert_eq!(
            first_model.get("loaded").and_then(|v| v.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn test_rename_model_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.loaded_from = Some(temp_dir.path().to_path_buf());
        config.models.insert(
            "old-name".to_string(),
            crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
            },
        );
        let state = ProxyState::new(config, None);

        // Rename should succeed
        state.rename_model("old-name", "new-name").await.unwrap();

        // Verify old name is gone, new name exists
        let config = state.config.read().await;
        assert!(!config.models.contains_key("old-name"));
        assert!(config.models.contains_key("new-name"));
    }

    #[tokio::test]
    async fn test_rename_model_new_name_taken() {
        let mut config = Config::default();
        config.models.insert(
            "old-name".to_string(),
            crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
            },
        );
        config.models.insert(
            "new-name".to_string(),
            crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
            },
        );
        let state = ProxyState::new(config, None);

        // Rename should fail because new name is taken
        let result = state.rename_model("old-name", "new-name").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "model name 'new-name' already taken"
        );
    }

    #[tokio::test]
    async fn test_rename_model_old_name_not_found() {
        let mut config = Config::default();
        config.models.insert(
            "existing-name".to_string(),
            crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
            },
        );
        let state = ProxyState::new(config, None);

        // Rename should fail because old name doesn't exist
        let result = state.rename_model("non-existent", "new-name").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "model 'non-existent' does not exist"
        );
    }

    #[tokio::test]
    async fn test_rename_model_empty_name() {
        let mut config = Config::default();
        config.models.insert(
            "old-name".to_string(),
            crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
            },
        );
        let state = ProxyState::new(config, None);

        // Rename should fail because new name is empty
        let result = state.rename_model("old-name", "").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "new name cannot be empty");
    }

    #[tokio::test]
    async fn test_rename_model_same_name() {
        let mut config = Config::default();
        config.models.insert(
            "same-name".to_string(),
            crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: std::collections::BTreeMap::new(),
            },
        );
        let state = ProxyState::new(config, None);

        // Rename should fail because old and new name are the same
        let result = state.rename_model("same-name", "same-name").await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "old name and new name must differ"
        );
    }
}
