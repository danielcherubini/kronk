mod state;
mod lifecycle;
mod process;
mod handlers;
mod forward;
mod server;

pub use state::{ModelState, ProxyMetrics, ProxyState};
pub use process::{override_arg, is_process_alive, kill_process, force_kill_process, check_health};
pub use server::ProxyServer;
pub use handlers::{handle_chat_completions, handle_stream_chat_completions, handle_get_model, handle_list_models, handle_health, handle_status, handle_metrics, handle_fallback, json_error_response};
pub use forward::forward_request;


#[cfg(test)]
mod tests {
    use super::*; // To bring everything from mod.rs into scope
    use crate::config::Config;

    #[tokio::test]
    async fn test_proxy_state_new() {
        let config = Config::default();
        let state = ProxyState::new(config.clone());
        assert!(state.models.read().await.is_empty());
        assert_eq!(
            state.config.proxy.idle_timeout_secs,
            config.proxy.idle_timeout_secs
        );
    }

    #[tokio::test]
    async fn test_no_available_server_for_unknown_model() {
        let config = Config::default();
        let state = ProxyState::new(config);
        let result = state.get_available_server_for_model("nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_build_status_response() {
        let config = Config::default();
        let state = ProxyState::new(config);

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
        let state = ProxyState::new(config);

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
}
