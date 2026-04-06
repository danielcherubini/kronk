use std::time::{Duration, Instant, UNIX_EPOCH};

use super::types::{ModelState, ProxyState};

impl ProxyState {
    /// Build a comprehensive status response for the proxy.
    ///
    /// Returns JSON matching the spec: models as an object keyed by name,
    /// fields flat per model (not nested in a `runtime` sub-object),
    /// `idle_timeout_secs` at top level.
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
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants: BTreeMap::new(),
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
}
