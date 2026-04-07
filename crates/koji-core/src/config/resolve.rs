use super::types::{BackendConfig, Config, HealthCheck, ModelConfig};
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

    /// Build the merged arg list for a server, returning **flat tokens**
    /// suitable for `Command::args`.
    ///
    /// Merging order: `backend.default_args` → `server.args` →
    /// `server.sampling.to_args()`. Each later layer's flags fully replace
    /// the same flag in the earlier layers via `merge_args`.
    pub fn build_args(&self, server: &ModelConfig, backend: &BackendConfig) -> Vec<String> {
        let mut grouped = crate::config::merge_args(&backend.default_args, &server.args);
        if let Some(sampling) = &server.sampling {
            if !sampling.is_empty() {
                grouped = crate::config::merge_args(&grouped, &sampling.to_args());
            }
        }
        crate::config::flatten_args(&grouped)
    }

    /// Build the full argument list for a model, including model config args
    /// (`-m`, `-c`, `-ngl`) and sampling. Returns **flat tokens** suitable for
    /// `Command::args`.
    ///
    /// Merging order:
    /// 1. `backend.default_args`
    /// 2. `server.args`     (replaces same-flag entries from #1)
    /// 3. Injected `-m`/`-c`/`-ngl` (only if not already present after #1+#2)
    /// 4. `server.sampling.to_args()` (replaces same-flag entries from #1+#2+#3)
    ///
    /// **Invariant:** the returned `Vec<String>` is always flat (one token
    /// per element). Callers like `proxy/lifecycle.rs::override_arg` and
    /// `bench/runner.rs::_override_arg` depend on this. The final
    /// `flatten_args` call enforces it; the `debug_assert!` makes accidental
    /// regressions visible in test/debug builds.
    pub fn build_full_args(
        &self,
        server: &ModelConfig,
        backend: &BackendConfig,
        ctx_override: Option<u32>,
    ) -> Result<Vec<String>> {
        let mut grouped = crate::config::merge_args(&backend.default_args, &server.args);

        // Inject -m from model card, only if not already present.
        if let (Some(ref model_id), Some(ref quant_name)) = (&server.model, &server.quant) {
            if let Some(quant_entry) = server.quants.get(quant_name.as_str()) {
                let models_dir = self.models_dir()?;
                let model_path = models_dir.join(model_id).join(&quant_entry.file);
                let already_has_m = grouped
                    .iter()
                    .any(|e| matches!(crate::config::flag_name(e), Some("-m") | Some("--model")));
                if !already_has_m {
                    let path_str = model_path.to_string_lossy();
                    let quoted = crate::config::quote_value(&path_str);
                    grouped.push(format!("-m {}", quoted));
                }
            } else {
                tracing::warn!(
                    "Quant '{}' not found in ModelConfig for model '{}'",
                    quant_name,
                    model_id
                );
            }
        }

        // Inject --mmproj from model card, only if not already present.
        // The mmproj entry must exist in `server.quants` and have kind = Mmproj.
        if let (Some(ref model_id), Some(ref mmproj_name)) = (&server.model, &server.mmproj) {
            if let Some(mmproj_entry) = server.quants.get(mmproj_name.as_str()) {
                if mmproj_entry.kind == crate::config::QuantKind::Mmproj {
                    let models_dir = self.models_dir()?;
                    let mmproj_path = models_dir.join(model_id).join(&mmproj_entry.file);
                    let already_has_mmproj = grouped
                        .iter()
                        .any(|e| matches!(crate::config::flag_name(e), Some("--mmproj")));
                    if !already_has_mmproj {
                        let path_str = mmproj_path.to_string_lossy();
                        let quoted = crate::config::quote_value(&path_str);
                        grouped.push(format!("--mmproj {}", quoted));
                    }
                } else {
                    tracing::warn!(
                        "mmproj '{}' for model '{}' has kind={:?}, expected Mmproj",
                        mmproj_name,
                        model_id,
                        mmproj_entry.kind
                    );
                }
            } else {
                tracing::warn!(
                    "mmproj '{}' not found in ModelConfig for model '{}'",
                    mmproj_name,
                    model_id
                );
            }
        }

        // Inject -c (context length) only if not already present.
        let ctx = ctx_override.or(server.context_length).or_else(|| {
            server
                .quant
                .as_ref()
                .and_then(|q| server.quants.get(q).and_then(|qe| qe.context_length))
        });
        if let Some(ctx) = ctx {
            let already_has_c = grouped
                .iter()
                .any(|e| matches!(crate::config::flag_name(e), Some("-c") | Some("--ctx-size")));
            if !already_has_c {
                grouped.push(format!("-c {}", ctx));
            }
        }

        // Inject -ngl only if not already present.
        if let Some(ngl) = server.gpu_layers {
            let already_has_ngl = grouped.iter().any(|e| {
                matches!(
                    crate::config::flag_name(e),
                    Some("-ngl") | Some("--n-gpu-layers")
                )
            });
            if !already_has_ngl {
                grouped.push(format!("-ngl {}", ngl));
            }
        }

        // Sampling: each sampling flag fully replaces the same flag in
        // anything injected so far.
        if let Some(sampling) = &server.sampling {
            if !sampling.is_empty() {
                grouped = crate::config::merge_args(&grouped, &sampling.to_args());
            }
        }

        let flat = crate::config::flatten_args(&grouped);
        // INVARIANT: build_full_args returns flat tokens. Callers like
        // proxy/lifecycle.rs::override_arg depend on this. The check
        // catches the failure mode where a *grouped* entry (e.g.
        // "-b 4096") leaks through unflattened: such an element starts
        // with '-' AND contains whitespace AND is not quoted.
        // Legitimate value-side tokens like "system: hi" or
        // "/path with space/m.gguf" contain whitespace but do NOT start
        // with '-', so they pass. We also allow tokens that start with a
        // quote character (escaped quotes from shlex unquoting edge cases).
        debug_assert!(
            flat.iter().all(|t| {
                !t.starts_with('-')
                    || !t.contains(char::is_whitespace)
                    || t.starts_with('"')
                    || t.starts_with('\'')
            }),
            "build_full_args invariant violated: element looks like a grouped entry (flag + space + value): {:?}",
            flat
        );
        Ok(flat)
    }

    pub fn service_name(server_name: &str) -> String {
        format!("koji-{}", server_name)
    }

    /// Open the application database, falling back to an in-memory connection on error.
    ///
    /// Tries `crate::db::open(&Config::base_dir()?)`. On failure, emits a `tracing::warn!`
    /// and returns a freshly-initialised in-memory connection so callers always get a
    /// usable `rusqlite::Connection` without duplicating the fallback boilerplate.
    pub fn open_db() -> rusqlite::Connection {
        match Config::base_dir().and_then(|dir| crate::db::open(&dir)) {
            Ok(crate::db::OpenResult { conn, .. }) => conn,
            Err(e) => {
                tracing::warn!(
                    "Failed to open DB, falling back to in-memory connection: {}",
                    e
                );
                crate::db::open_in_memory()
                    .expect("in-memory DB must always open")
                    .conn
            }
        }
    }

    /// Open the application database from an explicit directory, with fallback.
    ///
    /// Tries `crate::db::open(explicit_dir)` if `explicit_dir` is `Some`, then
    /// falls back to `Config::base_dir()`, and finally to an in-memory connection.
    /// Emits a `tracing::warn!` only when all on-disk attempts fail.
    pub fn open_db_from(explicit_dir: Option<&std::path::Path>) -> rusqlite::Connection {
        // 1. Explicit dir
        if let Some(dir) = explicit_dir {
            if let Ok(crate::db::OpenResult { conn, .. }) = crate::db::open(dir) {
                return conn;
            }
        }
        // 2. Default base dir
        if let Ok(base_dir) = Config::base_dir() {
            if let Ok(crate::db::OpenResult { conn, .. }) = crate::db::open(&base_dir) {
                return conn;
            }
        }
        // 3. In-memory fallback
        tracing::warn!("Failed to open DB, falling back to in-memory connection");
        crate::db::open_in_memory()
            .expect("in-memory DB must always open")
            .conn
    }

    /// Build the proxy base URL from config, e.g. `http://0.0.0.0:11411`.
    /// Always returns a URL since the proxy may be running even if not
    /// marked as enabled in config (e.g. started manually via `koji serve`).
    pub fn proxy_url(&self) -> String {
        format!("http://{}:{}", self.proxy.host, self.proxy.port)
    }

    /// Resolve the filesystem path for a named backend binary.
    ///
    /// Priority:
    /// 1. If `config.backends[name].version` is `Some(v)`, look up that exact `(name, version)`
    ///    in the DB. If found, return its path. If not found, return a descriptive error.
    /// 2. Otherwise, use the active (latest) installation from the DB.
    /// 3. Fallback to `path` field in `config.toml` [backends] section (for custom/manual installs).
    ///
    /// Returns an error if neither source has a path.
    pub fn resolve_backend_path(
        &self,
        name: &str,
        conn: &rusqlite::Connection,
    ) -> Result<std::path::PathBuf> {
        // Check if a specific version is pinned in config
        if let Some(pinned_version) = self.backends.get(name).and_then(|b| b.version.as_deref()) {
            return match crate::db::queries::get_backend_by_version(conn, name, pinned_version)? {
                Some(record) => Ok(std::path::PathBuf::from(record.path)),
                None => anyhow::bail!(
                    "Backend '{}' version '{}' not found in DB. Run `koji backend install {}` first.",
                    name,
                    pinned_version,
                    name
                ),
            };
        }

        // No version pin — use the active (latest) installation
        if let Some(record) = crate::db::queries::get_active_backend(conn, name)? {
            return Ok(std::path::PathBuf::from(record.path));
        }

        // Fallback to config path (for custom/manual installs)
        self.backends
            .get(name)
            .and_then(|b| b.path.as_deref())
            .map(std::path::PathBuf::from)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Backend '{}' has no installed path. Run `koji backend install {}` first.",
                    name,
                    name
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::QuantEntry;
    use crate::config::BackendConfig;
    use crate::db::queries::BackendInstallationRecord;
    use crate::db::{open_in_memory, queries::insert_backend_installation};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn make_test_config(llama_cpp_path: Option<&str>) -> Config {
        let mut config = Config::default();
        if let Some(path) = llama_cpp_path {
            config.backends.insert(
                "llama_cpp".to_string(),
                BackendConfig {
                    path: Some(path.to_string()),
                    default_args: vec![],
                    health_check_url: None,
                    version: None,
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
                    version: None,
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

    #[test]
    fn test_resolve_backend_path_version_pin() {
        let crate::db::OpenResult { conn, .. } = open_in_memory().unwrap();

        // Insert v1.0.0 and v2.0.0 (v2.0.0 will be active)
        let r1 = BackendInstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: "v1.0.0".to_string(),
            path: "/v1/llama-server".to_string(),
            installed_at: 1000,
            gpu_type: None,
            source: None,
            is_active: false,
        };
        insert_backend_installation(&conn, &r1).unwrap();

        let r2 = BackendInstallationRecord {
            id: 0,
            name: "llama_cpp".to_string(),
            backend_type: "llama_cpp".to_string(),
            version: "v2.0.0".to_string(),
            path: "/v2/llama-server".to_string(),
            installed_at: 2000,
            gpu_type: None,
            source: None,
            is_active: false,
        };
        insert_backend_installation(&conn, &r2).unwrap();

        // Pin config to v1.0.0
        let mut config = make_test_config(None);
        config.backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: Some("v1.0.0".to_string()),
            },
        );

        let result = config.resolve_backend_path("llama_cpp", &conn).unwrap();
        // Should return v1 path, not v2 (which is active)
        assert_eq!(result, std::path::PathBuf::from("/v1/llama-server"));
    }

    #[test]
    fn test_resolve_backend_path_version_pin_not_found() {
        let crate::db::OpenResult { conn, .. } = open_in_memory().unwrap();
        // Empty DB — version pin won't find anything

        let mut config = make_test_config(None);
        config.backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: Some("nonexistent".to_string()),
            },
        );

        let result = config.resolve_backend_path("llama_cpp", &conn);
        assert!(
            result.is_err(),
            "Expected Err when pinned version not in DB"
        );
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("not found in DB"),
            "Expected 'not found in DB' in error message, got: {}",
            err
        );
    }

    #[test]
    fn test_build_full_args_unified() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model-Q4_K_M.gguf");

        // Create the model directory structure and file
        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

        let mut quants = BTreeMap::new();
        quants.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "model-Q4_K_M.gguf".to_string(),
                kind: Default::default(),
                size_bytes: None,
                context_length: Some(8192),
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: Some(crate::profiles::SamplingParams {
                temperature: Some(0.3),
                ..Default::default()
            }),
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: Some(4096),
            profile: None,
            display_name: None,
            gpu_layers: Some(99),
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![],
            health_check_url: None,
            version: None,
        };

        let args = config
            .build_full_args(&server, &backend, None)
            .expect("build_full_args failed");

        // Verify model path arg
        assert!(
            args.iter().any(|a| a.contains("model-Q4_K_M.gguf")),
            "Args should contain model path: {:?}",
            args
        );

        // Verify context length from server
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"4096".to_string()));

        // Verify gpu_layers
        assert!(args.contains(&"-ngl".to_string()));
        assert!(args.contains(&"99".to_string()));

        // Verify sampling args (flattened)
        assert!(args.iter().any(|a| a == "--temp"));
        assert!(args.iter().any(|a| a == "0.30"));
    }

    #[test]
    fn test_build_full_args_ctx_override() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model-Q4_K_M.gguf");

        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

        let mut quants = BTreeMap::new();
        quants.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "model-Q4_K_M.gguf".to_string(),
                kind: Default::default(),
                size_bytes: None,
                context_length: Some(8192),
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: Some(crate::profiles::SamplingParams {
                temperature: Some(0.3),
                ..Default::default()
            }),
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: Some(4096),
            profile: None,
            display_name: None,
            gpu_layers: Some(99),
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![],
            health_check_url: None,
            version: None,
        };

        // ctx_override should take priority over server.context_length
        let args = config
            .build_full_args(&server, &backend, Some(2048))
            .expect("build_full_args failed");

        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"2048".to_string()));
        assert!(!args.contains(&"4096".to_string()));
    }

    #[test]
    fn test_build_full_args_no_sampling() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model-Q4_K_M.gguf");

        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

        let mut quants = BTreeMap::new();
        quants.insert(
            "Q4_K_M".to_string(),
            QuantEntry {
                file: "model-Q4_K_M.gguf".to_string(),
                kind: Default::default(),
                size_bytes: None,
                context_length: None,
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None, // No sampling params
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: Some(99),
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![],
            health_check_url: None,
            version: None,
        };

        let args = config
            .build_full_args(&server, &backend, None)
            .expect("build_full_args failed");

        // Verify no sampling args
        assert!(!args.iter().any(|a| a.starts_with("--temp")));
        assert!(!args.iter().any(|a| a.starts_with("--top-k")));
        assert!(!args.iter().any(|a| a.starts_with("--top-p")));
    }

    #[test]
    fn test_build_full_args_no_quants() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models");

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: Some(99),
            quants: BTreeMap::new(), // Empty quants map
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![],
            health_check_url: None,
            version: None,
        };

        // Should not crash when quants is empty
        let args = config.build_full_args(&server, &backend, None);
        assert!(args.is_ok());

        // Should not emit -m arg when quant lookup fails
        let args = args.expect("build_full_args failed");
        assert!(!args.iter().any(|a| a == "-m"));
    }

    #[test]
    fn build_args_dedupes_backend_vs_model_flags() {
        let mut config = Config::default();
        config.backends.insert(
            "test_backend".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![
                    "-b 2048".to_string(),
                    "-ub 512".to_string(),
                    "-t 14".to_string(),
                ],
                health_check_url: None,
                version: None,
            },
        );

        let server = ModelConfig {
            backend: "test_backend".to_string(),
            args: vec!["-b 4096".to_string(), "-ub 4096".to_string()],
            sampling: None,
            model: None,
            quant: None,
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
        };

        let backend = config.backends.get("test_backend").unwrap().clone();
        let flat = config.build_args(&server, &backend);

        // -t 14 from base must survive (flattened to separate tokens)
        assert!(flat.iter().any(|t| *t == "-t"));
        assert!(flat.iter().any(|t| *t == "14"));
        // -b appears exactly once with value 4096
        let b_count = flat.iter().filter(|t| *t == "-b").count();
        assert_eq!(b_count, 1, "expected exactly one -b flag, got {:?}", flat);
        assert!(flat.iter().any(|t| *t == "-b"));
        // -ub appears exactly once with value 4096
        let ub_count = flat.iter().filter(|t| *t == "-ub").count();
        assert_eq!(ub_count, 1, "expected exactly one -ub flag, got {:?}", flat);
        assert!(flat.iter().any(|t| *t == "-ub"));
        // 2048 and 512 must NOT appear
        assert!(!flat.iter().any(|t| t.contains("2048")));
        assert!(!flat.iter().any(|t| t.contains("512")));
    }

    #[test]
    fn build_args_sampling_overrides_inline_temp_in_args() {
        // Requires SamplingParams::to_args to already be in grouped form
        // (done earlier in this same task, section 2a.1). If this test
        // fails with a flat-token mismatch instead of a dedup failure,
        // the to_args rewrite was skipped.
        let mut config = Config::default();
        config.backends.insert(
            "test_backend".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );

        let server = ModelConfig {
            backend: "test_backend".to_string(),
            // inline --temp in args should be overridden by sampling.temperature
            args: vec!["--temp 0.10".to_string()],
            sampling: Some(crate::profiles::SamplingParams {
                temperature: Some(0.5),
                ..Default::default()
            }),
            model: None,
            quant: None,
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
        };

        let backend = config.backends.get("test_backend").unwrap().clone();
        let flat = config.build_args(&server, &backend);

        // --temp appears exactly once with value 0.50 (flattened)
        let temp_count = flat.iter().filter(|t| *t == "--temp").count();
        assert_eq!(
            temp_count, 1,
            "expected exactly one --temp flag, got {:?}",
            flat
        );
        assert!(flat.iter().any(|t| *t == "--temp"));
        assert!(flat.iter().any(|t| *t == "0.50"));
        assert!(!flat.iter().any(|t| t.contains("0.10")));
    }

    #[test]
    fn build_full_args_dedupes_backend_vs_model_flags() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model-Q4_K_M.gguf");
        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

        let mut quants = std::collections::BTreeMap::new();
        quants.insert(
            "Q4_K_M".to_string(),
            crate::config::types::QuantEntry {
                file: "model-Q4_K_M.gguf".to_string(),
                kind: Default::default(),
                size_bytes: None,
                context_length: None,
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec!["-b 4096".to_string(), "-ub 4096".to_string()],
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: Some(4096),
            profile: None,
            display_name: None,
            gpu_layers: Some(99),
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![
                "-b 2048".to_string(),
                "-ub 512".to_string(),
                "-t 14".to_string(),
            ],
            health_check_url: None,
            version: None,
        };

        let args = config
            .build_full_args(&server, &backend, None)
            .expect("build_full_args failed");

        // -t 14 must survive from backend defaults
        assert!(
            args.windows(2).any(|w| w == ["-t", "14"]),
            "expected -t 14 in args, got {:?}",
            args
        );
        // -b appears exactly once with value 4096
        let b_count = args.iter().filter(|t| *t == "-b").count();
        assert_eq!(b_count, 1, "expected exactly one -b token, got {:?}", args);
        assert!(args.windows(2).any(|w| w == ["-b", "4096"]));
        // -ub appears exactly once with value 4096
        let ub_count = args.iter().filter(|t| *t == "-ub").count();
        assert_eq!(
            ub_count, 1,
            "expected exactly one -ub token, got {:?}",
            args
        );
        assert!(args.windows(2).any(|w| w == ["-ub", "4096"]));
        // No 2048 or 512 anywhere
        assert!(!args.iter().any(|t| t == "2048"));
        assert!(!args.iter().any(|t| t == "512"));
    }

    #[test]
    fn build_full_args_returns_flat_tokens_with_quoted_path() {
        // Path with spaces must round-trip through grouped → flat correctly.
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models with space");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model.gguf");
        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy").expect("Failed to write model file");

        let mut quants = std::collections::BTreeMap::new();
        quants.insert(
            "Q4".to_string(),
            crate::config::types::QuantEntry {
                file: "model.gguf".to_string(),
                kind: Default::default(),
                size_bytes: None,
                context_length: None,
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4".to_string()),
            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![],
            health_check_url: None,
            version: None,
        };

        let args = config
            .build_full_args(&server, &backend, None)
            .expect("build_full_args failed");

        // -m and the path must appear as adjacent flat tokens, with the
        // space-containing path preserved as a single token.
        let m_pos = args.iter().position(|t| t == "-m").expect("-m not found");
        let path_token = &args[m_pos + 1];
        assert!(
            path_token.contains("models with space"),
            "expected path with spaces preserved as a single token, got {:?}",
            path_token
        );
        assert!(path_token.ends_with("model.gguf"));
    }
}
