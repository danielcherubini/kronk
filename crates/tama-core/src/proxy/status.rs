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
    #[allow(deprecated)]
    pub async fn collect_model_statuses(&self) -> Vec<crate::gpu::ModelStatus> {
        let config = self.config.read().await;
        let model_configs = self.model_configs.read().await;
        let runtime = self.models.read().await;
        let mut out: Vec<crate::gpu::ModelStatus> = Vec::with_capacity(model_configs.len());
        for (model_id, model_cfg) in model_configs.iter() {
            // Determine the model's lifecycle state from its server entries.
            let servers = config.resolve_servers_for_model(&model_configs, model_id);
            let mut best_state: Option<&ModelState> = None;
            for (server_name, _, _) in servers {
                if let Some(state) = runtime.get(&server_name) {
                    match state {
                        ModelState::Ready { .. } => {
                            best_state = Some(state);
                            break; // Ready is the best possible state
                        }
                        ModelState::Starting { .. }
                        | ModelState::Unloading { .. }
                        | ModelState::Failed { .. } => {
                            if best_state.is_none() {
                                best_state = Some(state);
                            }
                        }
                    }
                }
            }

            let (loaded, state_str) = match best_state {
                Some(ModelState::Ready { .. }) => (true, "ready".to_string()),
                Some(ModelState::Starting { .. }) => (false, "loading".to_string()),
                Some(ModelState::Unloading { .. }) => (false, "unloading".to_string()),
                Some(ModelState::Failed { .. }) => (false, "failed".to_string()),
                None => (false, "idle".to_string()),
            };

            out.push(crate::gpu::ModelStatus {
                id: model_id.clone(),
                db_id: model_cfg.db_id,
                api_name: model_cfg.api_name.clone(),
                display_name: model_cfg.display_name.clone(),
                backend: model_cfg.backend.clone(),
                loaded,
                state: state_str,
                quant: model_cfg.quant.clone(),
                context_length: model_cfg.context_length,
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
        let model_configs = self.model_configs.read().await;
        let auto_unload = config.proxy.auto_unload;
        let idle_timeout_secs = config.proxy.idle_timeout_secs;
        let models = self.models.read().await;
        let mut models_obj = serde_json::Map::new();

        for (model_name, model_config) in model_configs.iter() {
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
                    let elapsed = now.duration_since(*last_accessed);
                    let idle_timeout_remaining_secs: serde_json::Value = if auto_unload {
                        let timeout = Duration::from_secs(idle_timeout_secs);
                        if elapsed < timeout {
                            serde_json::json!((timeout - elapsed).as_secs())
                        } else {
                            serde_json::json!(0)
                        }
                    } else {
                        // Auto-unload disabled — no countdown
                        serde_json::Value::Null
                    };
                    let load_time_secs = load_time
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);

                    serde_json::json!({
                        "id": model_config.db_id,
                        "display_name": model_config.display_name,
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": true,
                        "state": "ready",
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
                        "id": model_config.db_id,
                        "display_name": model_config.display_name,
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": false,
                        "state": "loading",
                        "backend_pid": null,
                        "load_time_secs": null,
                        "last_accessed_secs_ago": null,
                        "idle_timeout_remaining_secs": null,
                        "consecutive_failures": consecutive_failures.load(Relaxed),
                    })
                }
                Some(ModelState::Unloading { .. }) => {
                    serde_json::json!({
                        "id": model_config.db_id,
                        "display_name": model_config.display_name,
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": false,
                        "state": "unloading",
                        "backend_pid": null,
                        "load_time_secs": null,
                        "last_accessed_secs_ago": null,
                        "idle_timeout_remaining_secs": null,
                        "consecutive_failures": null,
                    })
                }
                Some(ModelState::Failed { .. }) => {
                    serde_json::json!({
                        "id": model_config.db_id,
                        "display_name": model_config.display_name,
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": false,
                        "state": "failed",
                        "backend_pid": null,
                        "load_time_secs": null,
                        "last_accessed_secs_ago": null,
                        "idle_timeout_remaining_secs": null,
                        "consecutive_failures": null,
                    })
                }
                _ => {
                    // Not loaded or failed
                    serde_json::json!({
                        "id": model_config.db_id,
                        "display_name": model_config.display_name,
                        "backend": model_config.backend,
                        "backend_path": backend_path,
                        "model": model_config.model,
                        "quant": model_config.quant,
                        "context_length": model_config.context_length,
                        "enabled": model_config.enabled,
                        "api_name": model_config.api_name,
                        "loaded": false,
                        "state": "idle",
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
            "auto_unload": auto_unload,
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
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: None,
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            quants: BTreeMap::new(),
            modalities: None,
            display_name: None,
            db_id: None,
        }
    }

    /// When `state.models` has no runtime entries, every configured model
    /// should be reported as `loaded == false`, with the returned vector
    /// sorted by id ascending and the `backend` field matching the
    /// corresponding `ModelConfig.backend` value.
    #[tokio::test]
    async fn test_collect_model_statuses_reports_idle_when_no_runtime_entry() {
        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Populate model_configs
        {
            let mut mc = state.model_configs.write().await;
            mc.insert("zephyr".to_string(), make_model_config("llama_cpp"));
            mc.insert("alpha".to_string(), make_model_config("vllm"));
        }

        // Sanity check: no runtime entries.
        assert!(state.models.read().await.is_empty());

        let statuses = state.collect_model_statuses().await;

        // Length matches the number of configured models.
        assert_eq!(statuses.len(), 2);

        // Every entry is reported as not loaded.
        assert!(
            statuses.iter().all(|s| s.state != "ready"),
            "expected every status to not be ready, got: {:?}",
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
        // Add backends so resolve_servers_for_model can match models.
        config.backends.insert(
            "vllm".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );
        config.backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );
        let state = ProxyState::new(config, None);

        // Populate model_configs
        {
            let mut mc = state.model_configs.write().await;
            mc.insert("zephyr".to_string(), make_model_config("llama_cpp"));
            mc.insert("alpha".to_string(), make_model_config("vllm"));
        }

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
        let loaded_count = statuses.iter().filter(|s| s.state == "ready").count();
        assert_eq!(
            loaded_count, 1,
            "expected exactly one loaded model, got: {:?}",
            statuses
        );

        // alpha is loaded with the configured backend.
        assert_eq!(statuses[0].id, "alpha");
        assert_eq!(statuses[0].state, "ready", "expected alpha to be ready");
        assert_eq!(statuses[0].backend, "vllm");

        // zephyr is not loaded but still carries its configured backend.
        assert_eq!(statuses[1].id, "zephyr");
        assert_eq!(statuses[1].state, "idle", "expected zephyr to not be ready");
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

        let config = Config::default();
        let state = ProxyState::new(config, None);

        // Populate model_configs
        {
            let mut mc = state.model_configs.write().await;
            mc.insert("alpha".to_string(), make_model_config("llama_cpp"));
        }

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
        assert_eq!(
            alpha.state, "loading",
            "ModelState::Starting must not be reported as ready, got: {:?}",
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
        assert_eq!(
            alpha.state, "failed",
            "ModelState::Failed must not be reported as ready, got: {:?}",
            alpha
        );
    }
}
