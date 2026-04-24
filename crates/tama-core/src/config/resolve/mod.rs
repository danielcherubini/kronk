use super::types::{BackendConfig, Config, HealthCheck, ModelConfig};
use crate::models::repo_path;
use anyhow::Result;

impl Config {
    pub fn resolve_server<'a>(
        &'a self,
        models: &'a std::collections::HashMap<String, ModelConfig>,
        name: &str,
    ) -> Result<(&'a ModelConfig, &'a BackendConfig)> {
        use anyhow::Context;

        // First, search by api_name to avoid config key precedence issues.
        // Comparison is case-insensitive (OpenAI API model IDs are
        // case-insensitive) while the stored api_name preserves the
        // original case used by the user.
        let mut api_name_matches: Vec<_> = models
            .values()
            .filter(|s| {
                s.api_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .collect();

        let server = if api_name_matches.len() == 1 {
            // Single api_name match - use it
            api_name_matches.pop().unwrap()
        } else if api_name_matches.len() > 1 {
            // Ambiguous api_name - error out
            anyhow::bail!(
                "Ambiguous api_name '{}': multiple models share this api_name",
                name
            );
        } else if let Some(server) = models.get(name) {
            // No api_name match, try direct config key lookup
            server
        } else {
            // Fall back to searching model field
            models
                .values()
                .find(|s| s.model.as_deref() == Some(name))
                .with_context(|| format!("Model '{}' not found in config", name))?
        };

        let backend = self.backends.get(&server.backend).with_context(|| {
            format!(
                "Backend '{}' referenced by model not found in config",
                server.backend
            )
        })?;

        Ok((server, backend))
    }

    pub fn resolve_servers_for_model<'a>(
        &'a self,
        models: &'a std::collections::HashMap<String, ModelConfig>,
        model_name: &str,
    ) -> Vec<(String, &'a ModelConfig, &'a BackendConfig)> {
        let mut results = Vec::new();

        for (config_name, server) in models {
            if !server.enabled {
                continue;
            }
            let backend = match self.backends.get(&server.backend) {
                Some(b) => b,
                None => continue,
            };

            // Match on api_name (highest priority), then config key, then model field.
            // Comparisons are case-insensitive for api_name and model (OpenAI API
            // model IDs are case-insensitive), but config_name is case-sensitive.
            let api_name_match = server
                .api_name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(model_name));
            let model_match = server
                .model
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(model_name));
            if api_name_match || config_name == model_name || model_match {
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
                let model_path = repo_path(&models_dir, model_id).join(&quant_entry.file);
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
                    let mmproj_path = repo_path(&models_dir, model_id).join(&mmproj_entry.file);
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
                let slots = server.num_parallel.unwrap_or(1);
                let total_ctx = if server.kv_unified {
                    // Unified KV: all slots share one pool, -c = per-slot context
                    ctx
                } else {
                    // Non-unified: each slot gets dedicated region, -c = per_slot * slots
                    ctx.saturating_mul(slots)
                };
                grouped.push(format!("-c {}", total_ctx));
            }
        }

        // Inject -np (number of parallel slots) only if set and > 1.
        if let Some(slots) = server.num_parallel {
            if slots > 1 {
                let already_has_np = grouped.iter().any(|e| {
                    matches!(
                        crate::config::flag_name(e),
                        Some("-np") | Some("--parallel")
                    )
                });
                if !already_has_np {
                    grouped.push(format!("-np {}", slots));
                }
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

        // Inject --kv-unified flag when enabled and not already present.
        if server.kv_unified {
            let already_has_kv_unified = grouped
                .iter()
                .any(|e| matches!(crate::config::flag_name(e), Some("--kv-unified")));
            if !already_has_kv_unified {
                grouped.push("--kv-unified".to_string());
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
        format!("tama-{}", server_name)
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
    /// marked as enabled in config (e.g. started manually via `tama serve`).
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
                    "Backend '{}' version '{}' not found in DB. Run `tama backend install {}` first.",
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
                    "Backend '{}' has no installed path. Run `tama backend install {}` first.",
                    name,
                    name
                )
            })
    }
}

#[cfg(test)]
mod tests;
