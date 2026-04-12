use std::time::{Duration, Instant, UNIX_EPOCH};

use super::types::{ModelState, ProxyState};

impl ProxyState {
    /// Build the per-model status snapshot embedded in `MetricSample.models`.
    ///
    /// Iterates over every configured model, resolves its servers, and reports
    /// `loaded: true` iff at least one of the server entries returned by
    /// `Config::resolve_servers_for_model` is in `ModelState::Ready`. The
    /// returned vector is sorted by `id` so dashboard rows do not shuffle
    /// between SSE samples.
    pub async fn collect_model_statuses(&self) -> Vec<crate::gpu::ModelStatus> {
        let config = self.config.read().await;
        let runtime = self.models.read().await;
        let mut out: Vec<crate::gpu::ModelStatus> = Vec::with_capacity(config.models.len());
        for (model_id, model_cfg) in &config.models {
            // A model is "loaded" iff at least one of its server entries
            // in `state.models` is in the Ready state. Mirrors the logic
            // used by build_status_response().
            let loaded = config.resolve_servers_for_model(model_id).into_iter().any(
                |(server_name, _, _)| {
                    runtime
                        .get(&server_name)
                        .map(|s| s.is_ready())
                        .unwrap_or(false)
                },
            );
            out.push(crate::gpu::ModelStatus {
                id: model_id.clone(),
                api_name: model_cfg.api_name.clone(),
                display_name: model_cfg.display_name.clone(),
                backend: model_cfg.backend.clone(),
                loaded,
            });
        }
        // Stable order so dashboard rows don't shuffle between samples.
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Build a comprehensive status response for the proxy.
    ///
    /// Returns JSON matching the spec: models as an object keyed by name,
    /// fields flat per model (not nested in a `runtime` sub-object),
    /// `idle_timeout_secs` at top level.
    pub async fn build_status_response(&self) -> serde_json::Value {
        use std::sync::atomic::Ordering::Relaxed;

        let sys_metrics = self.system_metrics.read().await.clone();

        let config = self.config.read().await;
        let idle_timeout_secs = config.proxy.idle_timeout_secs;
        let models = self.models.read().await;
        let mut models_obj = serde_json::Map::new();

        for (model_name, model_config) in &config.models {
            let backend_path = match config.backends.get(&model_config.backend) {
                Some(b) => b.path.clone(),
                None => continue,
            };

            let model_state = models.get(model_name);

            let model_json = match model_state {
                Some(ModelState::Ready {
                    backend_pid,
                    load_time,
                    last_accessed,
                    consecutive_failures,
                    ..
                }) => {
                    let now = Instant::now();
                    let last_accessed_secs_ago = now.duration_since(*last_accessed).as_secs();
                    let timeout = Duration::from_secs(idle_timeout_secs);
                    let elapsed = now.duration_since(*last_accessed);
                    let idle_timeout_remaining_secs = if elapsed < timeout {
                        (timeout - elapsed).as_secs()
                    } else {
                        0
                    };
                    let load_time_secs = load_time
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    serde_json::json!({
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": true,
                        "backend_pid": *backend_pid,
                        "load_time_secs": load_time_secs,
                        "last_accessed_secs_ago": last_accessed_secs_ago,
                        "idle_timeout_remaining_secs": idle_timeout_remaining_secs,
                        "consecutive_failures": consecutive_failures.load(Relaxed),
                    })
                }
                Some(ModelState::Starting {
                    consecutive_failures,
                    ..
                }) => {
                    serde_json::json!({
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": false,
                        "backend_pid": null,
                        "load_time_secs": null,
                        "last_accessed_secs_ago": null,
                        "idle_timeout_remaining_secs": null,
                        "consecutive_failures": consecutive_failures.load(Relaxed),
                    })
                }
                _ => {
                    // Not loaded or failed
                    serde_json::json!({
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": false,
                        "backend_pid": null,
                        "load_time_secs": null,
                        "last_accessed_secs_ago": null,
                        "idle_timeout_remaining_secs": null,
                        "consecutive_failures": null,
                    })
                }
            };

            models_obj.insert(model_name.clone(), model_json);
        }

        drop(models);

        let metrics = &self.metrics;

        serde_json::json!({
            "cpu_usage_pct": sys_metrics.cpu_usage_pct,
            "ram_used_mib": sys_metrics.ram_used_mib,
            "ram_total_mib": sys_metrics.ram_total_mib,
            "gpu_utilization_pct": sys_metrics.gpu_utilization_pct,
            "vram": sys_metrics.vram.map(|v| serde_json::json!({
                "used_mib": v.used_mib,
                "total_mib": v.total_mib,
            })),
            "idle_timeout_secs": idle_timeout_secs,
            "metrics": {
                "total_requests": metrics.total_requests.load(Relaxed),
                "successful_requests": metrics.successful_requests.load(Relaxed),
                "failed_requests": metrics.failed_requests.load(Relaxed),
                "models_loaded": metrics.models_loaded.load(Relaxed),
                "models_unloaded": metrics.models_unloaded.load(Relaxed),
            },
            "models": models_obj,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BackendConfig, Config, ModelConfig};
    use std::collections::BTreeMap;

    fn make_model_config(backend: &str) -> ModelConfig {
        ModelConfig {
            backend: backend.to_string(),
            args: vec![],
            sampling: None,
            model: None,
            quant: None,

            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            api_name: None,
            gpu_layers: None,
            quants: BTreeMap::new(),
            modalities: None,
            display_name: None,
        }
    }

    /// When `state.models` has no runtime entries, every configured model
    /// should be reported as `loaded == false`, with the returned vector
    /// sorted by id ascending and the `backend` field matching the
    /// corresponding `ModelConfig.backend` value.
    #[tokio::test]
    async fn test_collect_model_statuses_reports_idle_when_no_runtime_entry() {
        let mut config = Config::default();
        // Clear default fixtures so the test only sees the models we add.
        config.models.clear();
        config.backends.insert(
            "vllm".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );
        config
            .models
            .insert("zephyr".to_string(), make_model_config("llama_cpp"));
        config
            .models
            .insert("alpha".to_string(), make_model_config("vllm"));

        let state = ProxyState::new(config, None);

        // Sanity check: no runtime entries.
        assert!(state.models.read().await.is_empty());

        let statuses = state.collect_model_statuses().await;

        // Length matches the number of configured models.
        assert_eq!(statuses.len(), 2);

        // Every entry is reported as not loaded.
        assert!(
            statuses.iter().all(|s| !s.loaded),
            "expected every status to have loaded == false, got: {:?}",
            statuses
        );

        // Entries are sorted by id ascending.
        let ids: Vec<&str> = statuses.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zephyr"]);

        // Backend field matches the configured backend name for each model.
        assert_eq!(statuses[0].id, "alpha");
        assert_eq!(statuses[0].backend, "vllm");
        assert_eq!(statuses[1].id, "zephyr");
        assert_eq!(statuses[1].backend, "llama_cpp");
    }

    /// When `state.models` contains a `ModelState::Ready` entry under the
    /// server name that resolves for one of the configured models, that
    /// model should be reported as `loaded == true` while all other
    /// configured models remain `loaded == false`. The returned vector
    /// must still be sorted by id ascending and carry the configured
    /// `backend` value.
    #[tokio::test]
    async fn test_collect_model_statuses_reports_loaded_when_server_is_ready() {
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;
        use std::time::{Instant, SystemTime};

        let mut config = Config::default();
        // Clear default fixtures so the test only sees the models we add.
        config.models.clear();
        config.backends.insert(
            "vllm".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );
        config
            .models
            .insert("zephyr".to_string(), make_model_config("llama_cpp"));
        config
            .models
            .insert("alpha".to_string(), make_model_config("vllm"));

        let state = ProxyState::new(config, None);

        // Insert a Ready entry for "alpha" under the server name that
        // `resolve_servers_for_model("alpha")` will return — the config key
        // itself, since `make_model_config` leaves `model` as `None`.
        {
            let mut runtime = state.models.write().await;
            runtime.insert(
                "alpha".to_string(),
                ModelState::Ready {
                    model_name: "alpha".to_string(),
                    backend: "vllm".to_string(),
                    backend_pid: 12345,
                    backend_url: "http://127.0.0.1:8000".to_string(),
                    load_time: SystemTime::now(),
                    last_accessed: Instant::now(),
                    consecutive_failures: Arc::new(AtomicU32::new(0)),
                    failure_timestamp: None,
                },
            );
        }

        let statuses = state.collect_model_statuses().await;

        // Length matches the number of configured models.
        assert_eq!(statuses.len(), 2);

        // Entries are sorted by id ascending.
        let ids: Vec<&str> = statuses.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zephyr"]);

        // Exactly one model is reported as loaded.
        let loaded_count = statuses.iter().filter(|s| s.loaded).count();
        assert_eq!(
            loaded_count, 1,
            "expected exactly one loaded model, got: {:?}",
            statuses
        );

        // alpha is loaded with the configured backend.
        assert_eq!(statuses[0].id, "alpha");
        assert!(statuses[0].loaded, "expected alpha to be loaded");
        assert_eq!(statuses[0].backend, "vllm");

        // zephyr is not loaded but still carries its configured backend.
        assert_eq!(statuses[1].id, "zephyr");
        assert!(!statuses[1].loaded, "expected zephyr to not be loaded");
        assert_eq!(statuses[1].backend, "llama_cpp");
    }

    /// `collect_model_statuses` should only treat `ModelState::Ready` as
    /// "loaded". Other variants like `Starting` and `Failed` must be
    /// reported as `loaded == false` so the dashboard does not falsely
    /// claim a model is serving traffic while it is still booting or has
    /// crashed.
    #[tokio::test]
    async fn test_collect_model_statuses_ignores_non_ready_states() {
        use std::sync::atomic::AtomicU32;
        use std::sync::Arc;
        use std::time::Instant;

        let mut config = Config::default();
        // Clear default fixtures so the test only sees the model we add.
        config.models.clear();
        config
            .models
            .insert("alpha".to_string(), make_model_config("llama_cpp"));

        let state = ProxyState::new(config, None);

        // The server name `resolve_servers_for_model("alpha")` returns is
        // the config key itself, since `make_model_config` leaves
        // `model` as `None`.
        let server_name = "alpha".to_string();

        // --- Case 1: Starting must NOT count as loaded ---------------------
        {
            let mut runtime = state.models.write().await;
            runtime.insert(
                server_name.clone(),
                ModelState::Starting {
                    model_name: "alpha".to_string(),
                    backend: "llama_cpp".to_string(),
                    backend_url: "http://127.0.0.1:8000".to_string(),
                    last_accessed: Instant::now(),
                    consecutive_failures: Arc::new(AtomicU32::new(0)),
                    failure_timestamp: None,
                },
            );
        }

        let statuses = state.collect_model_statuses().await;
        assert_eq!(
            statuses.len(),
            1,
            "expected one status entry per configured model, got: {:?}",
            statuses
        );
        let alpha = statuses
            .iter()
            .find(|s| s.id == "alpha")
            .expect("alpha entry missing from collect_model_statuses output");
        assert!(
            !alpha.loaded,
            "ModelState::Starting must not be reported as loaded, got: {:?}",
            alpha
        );

        // --- Case 2: Failed must NOT count as loaded -----------------------
        {
            let mut runtime = state.models.write().await;
            runtime.insert(
                server_name.clone(),
                ModelState::Failed {
                    model_name: "alpha".to_string(),
                    backend: "llama_cpp".to_string(),
                    error: "backend exited with status 1".to_string(),
                },
            );
        }

        let statuses = state.collect_model_statuses().await;
        assert_eq!(
            statuses.len(),
            1,
            "expected one status entry per configured model, got: {:?}",
            statuses
        );
        let alpha = statuses
            .iter()
            .find(|s| s.id == "alpha")
            .expect("alpha entry missing from collect_model_statuses output");
        assert!(
            !alpha.loaded,
            "ModelState::Failed must not be reported as loaded, got: {:?}",
            alpha
        );
    }
}
