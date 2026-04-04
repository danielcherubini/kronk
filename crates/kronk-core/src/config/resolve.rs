use super::types::{BackendConfig, Config, HealthCheck, ModelConfig};
use crate::profiles::SamplingParams;
use anyhow::Result;

impl Config {
    pub fn resolve_server(&self, name: &str) -> Result<(&ModelConfig, &BackendConfig)> {
        use anyhow::Context;
        let server = self
            .models
            .get(name)
            .or_else(|| {
                // Fallback: search for a server where the 'model' field matches the requested name
                self.models
                    .values()
                    .find(|s| s.model.as_deref() == Some(name))
            })
            .with_context(|| format!("Model '{}' not found in config", name))?;

        let backend = self.backends.get(&server.backend).with_context(|| {
            format!(
                "Backend '{}' referenced by model not found in config",
                server.backend
            )
        })?;

        Ok((server, backend))
    }

    pub fn resolve_servers_for_model(
        &self,
        model_name: &str,
    ) -> Vec<(String, &ModelConfig, &BackendConfig)> {
        let mut results = Vec::new();

        for (config_name, server) in &self.models {
            if !server.enabled {
                continue;
            }
            let backend = match self.backends.get(&server.backend) {
                Some(b) => b,
                None => continue,
            };

            // Match on config key (alias) or full model ID
            if config_name == model_name || server.model.as_deref() == Some(model_name) {
                results.push((config_name.clone(), server, backend));
            }
        }

        results
    }

    /// Resolve the health check URL for a server, taking into account:
    /// 1. Backend's health_check_url if set
    /// 2. Server's custom port if set
    /// 3. Fallback to http://localhost:{port}/health
    pub fn resolve_health_url(&self, server: &ModelConfig) -> Option<String> {
        let backend = match self.backends.get(&server.backend) {
            Some(b) => b,
            None => {
                tracing::warn!(
                    "Backend '{}' not found when resolving health URL",
                    server.backend
                );
                return None;
            }
        };

        // If backend has health_check_url, use it (and replace port if server.port is set)
        if let Some(ref backend_url) = backend.health_check_url {
            if let Some(port) = server.port {
                let mut url = url::Url::parse(backend_url).ok()?;
                url.set_port(Some(port)).ok()?;
                return Some(url.to_string());
            }
            return Some(backend_url.clone());
        }

        // backend.health_check_url is None, try server.port fallback
        if let Some(port) = server.port {
            return Some(format!("http://localhost:{}/health", port));
        }

        // Neither backend.health_check_url nor server.port present
        None
    }

    /// Resolve the backend URL (without /health) for a server.
    pub fn resolve_backend_url(&self, server: &ModelConfig) -> Option<String> {
        let backend = match self.backends.get(&server.backend) {
            Some(b) => b,
            None => {
                tracing::warn!(
                    "Backend '{}' not found when resolving backend URL",
                    server.backend
                );
                return None;
            }
        };

        // If backend has health_check_url, derive the base URL from it
        if let Some(ref health_url) = backend.health_check_url {
            let mut url = url::Url::parse(health_url).ok()?;

            // Override port if the server specifies one
            if let Some(port) = server.port {
                url.set_port(Some(port)).ok()?;
            }

            // Strip the path to get the base origin (scheme + host + port)
            url.set_path("");
            url.set_query(None);
            url.set_fragment(None);
            let base = url.to_string().trim_end_matches('/').to_string();
            return Some(base);
        }

        // backend.health_check_url is None, try server.port fallback
        if let Some(port) = server.port {
            return Some(format!("http://localhost:{}", port));
        }

        // Neither backend.health_check_url nor server.port present
        None
    }

    /// Resolve the effective health check config for a server.
    /// Merges: server.health_check -> backend.health_check_url -> supervisor defaults.
    pub fn resolve_health_check(&self, server: &ModelConfig) -> HealthCheck {
        let server_hc = server.health_check.as_ref();

        HealthCheck {
            url: server_hc
                .and_then(|h| h.url.clone())
                .or_else(|| self.resolve_health_url(server)),
            interval_ms: Some(
                server_hc
                    .and_then(|h| h.interval_ms)
                    .unwrap_or(self.supervisor.health_check_interval_ms),
            ),
            timeout_ms: Some(server_hc.and_then(|h| h.timeout_ms).unwrap_or(3000)),
        }
    }

    pub fn build_args(&self, server: &ModelConfig, backend: &BackendConfig) -> Vec<String> {
        let mut args = backend.default_args.clone();
        args.extend(server.args.clone());

        // Append sampling params as CLI flags, filtering out any duplicates
        // that may already be in server.args
        if let Some(sampling) = self.effective_sampling(server) {
            let sampling_args = sampling.to_args();
            let sampling_flags: std::collections::HashSet<&str> = sampling_args
                .iter()
                .filter(|a| a.starts_with("--"))
                .map(|a| a.as_str())
                .collect();

            // Remove existing sampling flags and their values from args
            if !sampling_flags.is_empty() {
                let mut filtered = Vec::with_capacity(args.len());
                let mut skip_next = false;
                for arg in &args {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if sampling_flags.contains(arg.as_str()) {
                        skip_next = true; // skip the flag and its following value
                        continue;
                    }
                    filtered.push(arg.clone());
                }
                args = filtered;
            }

            args.extend(sampling_args);
        }

        args
    }

    /// Build the full argument list for a model, including model card args (-m, -c, -ngl).
    /// This is the complete arg set needed to start a backend for a given model config.
    pub fn build_full_args(
        &self,
        server: &ModelConfig,
        backend: &BackendConfig,
        ctx_override: Option<u32>,
    ) -> Result<Vec<String>> {
        let mut args = self.build_args(server, backend);

        // Inject model card args: -m, -c, -ngl
        if let (Some(ref model_id), Some(ref quant_name)) = (&server.model, &server.quant) {
            let models_dir = self
                .models_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("models"));
            let configs_dir = self.configs_dir().unwrap_or_else(|_| {
                // Fallback: derive from models_dir if configs_dir is not available
                self.models_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("configs"))
            });
            let registry =
                crate::models::ModelRegistry::new(models_dir.clone(), configs_dir.clone());
            if let Some(installed) = registry.find(model_id)? {
                if let Some(q) = installed.card.quants.get(quant_name.as_str()) {
                    if !args.iter().any(|a| a == "-m" || a == "--model") {
                        args.push("-m".to_string());
                        args.push(installed.dir.join(&q.file).to_string_lossy().to_string());
                    }
                }
                // Context size: cli override > config override > model card
                let ctx = ctx_override
                    .or(server.context_length)
                    .or_else(|| installed.card.context_length_for(quant_name));
                if let Some(ctx) = ctx {
                    if !args.iter().any(|a| a == "-c" || a == "--ctx-size") {
                        args.push("-c".to_string());
                        args.push(ctx.to_string());
                    }
                }
                if let Some(ngl) = installed.card.model.default_gpu_layers {
                    if !args.iter().any(|a| a == "-ngl" || a == "--n-gpu-layers") {
                        args.push("-ngl".to_string());
                        args.push(ngl.to_string());
                    }
                }

                // 3-layer sampling merge with model card
                if let Some(sampling) =
                    self.effective_sampling_with_card(server, Some(&installed.card))
                {
                    // Remove any existing sampling args to avoid duplicates
                    let sampling_args = sampling.to_args();
                    let sampling_flags: std::collections::HashSet<&str> = sampling_args
                        .iter()
                        .filter(|a| a.starts_with("--"))
                        .map(|a| a.as_str())
                        .collect();

                    if !sampling_flags.is_empty() {
                        let mut filtered = Vec::with_capacity(args.len());
                        let mut skip_next = false;
                        for arg in &args {
                            if skip_next {
                                skip_next = false;
                                continue;
                            }
                            if sampling_flags.contains(arg.as_str()) {
                                skip_next = true;
                                continue;
                            }
                            filtered.push(arg.clone());
                        }
                        args = filtered;
                    }
                    args.extend(sampling_args);
                }
            } else {
                tracing::warn!(
                    "Model card for '{}' not found in registry (searched in: models={}, configs={})",
                    model_id,
                    models_dir.display(),
                    configs_dir.display()
                );
            }
        }

        Ok(args)
    }

    /// Resolve effective sampling for a server (no model card).
    /// Looks up sampling_templates by profile name, then merges server overrides.
    pub fn effective_sampling(&self, server: &ModelConfig) -> Option<SamplingParams> {
        let base = server
            .profile
            .as_ref()
            .and_then(|p| self.sampling_templates.get(&p.to_string()).cloned());

        match (base, &server.sampling) {
            (Some(base), Some(overrides)) => Some(base.merge(overrides)),
            (Some(base), None) => Some(base),
            (None, Some(sampling)) => Some(sampling.clone()),
            (None, None) => None,
        }
    }

    /// Resolve effective sampling with a 2-layer merge chain:
    /// 1. Model card `[sampling.<profile>]` (the single source of truth)
    /// 2. Server-level sampling overrides
    ///
    /// Falls back to `sampling_templates` if the card has no entry for the profile.
    pub fn effective_sampling_with_card(
        &self,
        server: &ModelConfig,
        card: Option<&crate::models::card::ModelCard>,
    ) -> Option<SamplingParams> {
        let profile_name = server.profile.as_ref().map(|p| p.to_string());

        // Layer 1: Model card sampling for this profile, falling back to templates
        let base = match (card, &profile_name) {
            (Some(card), Some(pname)) => card
                .sampling_for(pname)
                .cloned()
                .or_else(|| self.sampling_templates.get(pname).cloned()),
            (None, Some(pname)) => self.sampling_templates.get(pname).cloned(),
            _ => None,
        };

        // Layer 2: Server-level overrides
        match (base, &server.sampling) {
            (Some(base), Some(overrides)) => Some(base.merge(overrides)),
            (Some(base), None) => Some(base),
            (None, Some(sampling)) => Some(sampling.clone()),
            (None, None) => None,
        }
    }

    pub fn service_name(server_name: &str) -> String {
        format!("kronk-{}", server_name)
    }

    /// Build the proxy base URL from config, e.g. `http://0.0.0.0:11411`.
    /// Always returns a URL since the proxy may be running even if not
    /// marked as enabled in config (e.g. started manually via `kronk serve`).
    pub fn proxy_url(&self) -> String {
        format!("http://{}:{}", self.proxy.host, self.proxy.port)
    }

    /// Resolve the filesystem path for a named backend binary.
    ///
    /// Priority:
    /// 1. Active installation in the DB (via `get_active_backend`)
    /// 2. `path` field in `config.toml` [backends] section (for custom/manual installs)
    ///
    /// Returns an error if neither source has a path.
    pub fn resolve_backend_path(
        &self,
        name: &str,
        conn: &rusqlite::Connection,
    ) -> Result<std::path::PathBuf> {
        if let Some(record) = crate::db::queries::get_active_backend(conn, name)? {
            return Ok(std::path::PathBuf::from(record.path));
        }
        self.backends
            .get(name)
            .and_then(|b| b.path.as_deref())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Backend '{}' has no installed path. Run `kronk backend install {}` first.",
                    name,
                    name
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BackendConfig;
    use crate::db::queries::BackendInstallationRecord;
    use crate::db::{open_in_memory, queries::insert_backend_installation};

    fn make_test_config(llama_cpp_path: Option<&str>) -> Config {
        let mut config = Config::default();
        if let Some(path) = llama_cpp_path {
            config.backends.insert(
                "llama_cpp".to_string(),
                BackendConfig {
                    path: Some(path.to_string()),
                    default_args: vec![],
                    health_check_url: None,
                },
            );
        } else {
            // Insert with no path
            config.backends.insert(
                "llama_cpp".to_string(),
                BackendConfig {
                    path: None,
                    default_args: vec![],
                    health_check_url: None,
                },
            );
        }
        config
    }

    #[test]
    fn test_resolve_backend_path_from_db() {
        let crate::db::OpenResult { conn, .. } = open_in_memory().unwrap();
        let record = BackendInstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: "v1.0.0".to_string(),
            path: "/usr/local/bin/llama-server".to_string(),
            installed_at: 1000,
            gpu_type: None,
            source: None,
            is_active: false,
        };
        insert_backend_installation(&conn, &record).unwrap();

        let config = make_test_config(None);
        let result = config.resolve_backend_path("llama_cpp", &conn).unwrap();
        assert_eq!(
            result,
            std::path::PathBuf::from("/usr/local/bin/llama-server")
        );
    }

    #[test]
    fn test_resolve_backend_path_fallback() {
        let crate::db::OpenResult { conn, .. } = open_in_memory().unwrap();
        // Empty DB — no installed backend

        let config = make_test_config(Some("/fallback/llama-server"));
        let result = config.resolve_backend_path("llama_cpp", &conn).unwrap();
        assert_eq!(result, std::path::PathBuf::from("/fallback/llama-server"));
    }

    #[test]
    fn test_resolve_backend_path_error() {
        let crate::db::OpenResult { conn, .. } = open_in_memory().unwrap();
        // Empty DB, path = None

        let config = make_test_config(None);
        let result = config.resolve_backend_path("llama_cpp", &conn);
        assert!(
            result.is_err(),
            "Expected Err when no DB record and no path in config"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string()
                .contains("Backend 'llama_cpp' has no installed path"),
            "Unexpected error: {}",
            err
        );
    }
}
