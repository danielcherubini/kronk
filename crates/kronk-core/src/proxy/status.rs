use std::time::{Duration, Instant, UNIX_EPOCH};

use super::types::{ModelState, ProxyState};

impl ProxyState {
    /// Build a comprehensive status response for the proxy.
    ///
    /// Returns JSON matching the spec: models as an object keyed by name,
    /// fields flat per model (not nested in a `runtime` sub-object),
    /// `idle_timeout_secs` at top level.
    pub async fn build_status_response(&self) -> serde_json::Value {
        use std::sync::atomic::Ordering::Relaxed;

        let sys_metrics = self.system_metrics.read().await.clone();

        let idle_timeout_secs = self.config.proxy.idle_timeout_secs;
        let models = self.models.read().await;
        let mut models_obj = serde_json::Map::new();

        for (model_name, model_config) in &self.config.models {
            let backend_path = match self.config.backends.get(&model_config.backend) {
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
                        "profile": model_config.profile.as_ref().map(|p| p.to_string()),
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
                        "profile": model_config.profile.as_ref().map(|p| p.to_string()),
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
                        "profile": model_config.profile.as_ref().map(|p| p.to_string()),
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
