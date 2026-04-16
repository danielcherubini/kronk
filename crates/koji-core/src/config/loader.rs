use super::migrate::rename_legacy_directories;
use super::types::{BackendConfig, Config, General, ProxyConfig, Supervisor};
use crate::profiles::Profile;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Normalize all `default_args` and `args` lists in the config from
/// legacy flat form to grouped form. Returns `true` if anything changed.
fn normalize_grouped_args(config: &mut Config) -> bool {
    use crate::config::group_legacy_flat_args;
    let mut changed = false;
    for backend in config.backends.values_mut() {
        let (migrated, did) = group_legacy_flat_args(&backend.default_args);
        if did {
            backend.default_args = migrated;
            changed = true;
        }
    }
    changed
}

impl Config {
    /// Base directory for all koji data.
    /// Windows: `%APPDATA%\koji`
    /// Linux: `~/.config/koji`
    ///
    /// On first run after the rename from `kronk` to `koji`, this function
    /// also performs a one-time auto-migration of the legacy `kronk` data
    /// directory to the new `koji` location (including renaming `kronk.db`
    /// to `koji.db`).
    pub fn base_dir() -> Result<PathBuf> {
        let proj = directories::ProjectDirs::from("", "", "koji")
            .context("Failed to determine config directory")?;
        // config_dir() on Windows = %APPDATA%\koji\config, we want the parent
        // On Linux config_dir() = ~/.config/koji which is already the base
        #[cfg(target_os = "windows")]
        let base = proj
            .config_dir()
            .parent()
            .unwrap_or(proj.config_dir())
            .to_path_buf();
        #[cfg(not(target_os = "windows"))]
        let base = proj.config_dir().to_path_buf();

        // One-time auto-migration from the legacy kronk directory. This is
        // a no-op if the new directory already exists or if no legacy
        // directory is present.
        if let Err(e) = super::rename_legacy::migrate_legacy_data_dir(&base) {
            tracing::warn!("Legacy data directory migration failed: {}", e);
        }

        Ok(base)
    }

    pub fn config_dir() -> Result<PathBuf> {
        Self::base_dir()
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let config_dir = Self::config_dir()?;
        Self::load_from(&config_dir)
    }

    /// Load config from an explicit directory path.
    /// Used by the Windows service which runs as SYSTEM and needs
    /// the installing user's config directory.
    pub fn load_from(config_dir: &std::path::Path) -> Result<Self> {
        fs::create_dir_all(config_dir).context("Failed to create config directory")?;

        // Rename legacy .d directories if they exist
        let _ = rename_legacy_directories(config_dir);

        let config_path = config_dir.join("config.toml");

        let mut config = if config_path.exists() {
            let contents =
                fs::read_to_string(&config_path).context("Failed to read config file")?;
            let c: Config = toml::from_str(&contents).context("Failed to parse config file")?;
            c
        } else {
            let default = Self::default();
            let toml_str =
                toml::to_string_pretty(&default).context("Failed to serialize default config")?;
            fs::write(&config_path, &toml_str).context("Failed to write default config")?;
            tracing::info!("Created default config at {}", config_path.display());
            default
        };

        // Migrate legacy flat args to grouped form. If anything changed,
        // persist the migrated config back to disk so the next load is a
        // no-op and `koji status` shows the new format.
        let args_migrated = normalize_grouped_args(&mut config);
        if args_migrated {
            tracing::info!(
                "Migrated legacy flat args to grouped form in {}",
                config_path.display()
            );
        }

        config.loaded_from = Some(config_dir.to_path_buf());

        if args_migrated {
            // Best-effort save; if it fails (e.g. read-only filesystem),
            // log a warning but do not fail the load.
            if let Err(e) = config.save_to(config_dir) {
                tracing::warn!(
                    "Failed to persist migrated args to {}: {}",
                    config_path.display(),
                    e
                );
            }
        }

        Ok(config)
    }

    /// Save config to the location it was loaded from, or the default location.
    pub fn save(&self) -> Result<()> {
        if let Some(ref loaded) = self.loaded_from {
            return self.save_to(loaded);
        }
        let config_path = Self::config_path()?;
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, &toml_str).context("Failed to write config")?;
        Ok(())
    }

    /// Save config to a specific directory path.
    /// Used by tests and Windows service which need to save to non-standard locations.
    pub fn save_to(&self, config_dir: &std::path::Path) -> Result<()> {
        let config_path = config_dir.join("config.toml");
        fs::create_dir_all(config_dir).context("Failed to create config directory")?;
        let toml_str = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&config_path, &toml_str).context("Failed to write config")?;
        Ok(())
    }

    /// Resolve the logs directory path.
    /// Uses `general.logs_dir` if set, otherwise defaults to `<base_dir>/logs/`.
    /// On Windows this is `%APPDATA%\koji\logs\`, on Linux `~/.config/koji/logs/`.
    pub fn logs_dir(&self) -> Result<PathBuf> {
        if let Some(ref dir) = self.general.logs_dir {
            Ok(PathBuf::from(dir))
        } else if let Some(ref loaded) = self.loaded_from {
            Ok(loaded.join("logs"))
        } else {
            Ok(Self::base_dir()?.join("logs"))
        }
    }

    pub fn with_models_dir(&self, dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            general: General {
                models_dir: Some(dir.to_string_lossy().to_string()),
                ..self.general.clone()
            },
            ..self.clone()
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut backends = HashMap::new();
        backends.insert(
            "llama_cpp".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: Some("http://localhost:8080/health".to_string()),
                version: None,
            },
        );
        backends.insert(
            "ik_llama".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: Some("http://localhost:8080/health".to_string()),
                version: None,
            },
        );

        // Built-in sampling templates for all profiles
        let mut sampling_templates = HashMap::new();
        for (_, _, profile) in Profile::all() {
            let params = match profile {
                Profile::Coding => crate::profiles::SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(50),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.1),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Chat => crate::profiles::SamplingParams {
                    temperature: Some(0.7),
                    top_p: Some(0.95),
                    top_k: Some(40),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Analysis => crate::profiles::SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(20),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
                Profile::Creative => crate::profiles::SamplingParams {
                    temperature: Some(0.9),
                    top_p: Some(0.95),
                    top_k: Some(50),
                    min_p: Some(0.02),
                    presence_penalty: Some(0.0),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
            };
            sampling_templates.insert(profile.to_string(), params);
        }

        Config {
            general: General::default(),
            backends,
            supervisor: Supervisor::default(),
            proxy: ProxyConfig::default(),
            sampling_templates,
            loaded_from: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_migrates_flat_backend_default_args() {
        let mut config = Config::default();
        config.backends.insert(
            "flat_backend".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![
                    "-fa".to_string(),
                    "1".to_string(),
                    "-b".to_string(),
                    "2048".to_string(),
                    "--mlock".to_string(),
                ],
                health_check_url: None,
                version: None,
            },
        );

        let changed = normalize_grouped_args(&mut config);
        assert!(changed);

        let migrated = &config.backends["flat_backend"].default_args;
        assert_eq!(
            migrated,
            &vec![
                "-fa 1".to_string(),
                "-b 2048".to_string(),
                "--mlock".to_string()
            ]
        );
    }

    #[test]
    fn normalize_default_config_is_noop() {
        // Config::default() must already be in grouped form, so calling
        // normalize_grouped_args on it should not change anything. We
        // compare only the args/default_args fields rather than whole
        // structs because BackendConfig/ModelConfig/QuantEntry don't
        // currently derive PartialEq, and adding those derives is
        // out-of-scope for this PR.
        let mut config = Config::default();

        // Snapshot the args fields before normalization. Use BTreeMap to
        // get deterministic ordering for the comparison.
        let before_backend_args: std::collections::BTreeMap<String, Vec<String>> = config
            .backends
            .iter()
            .map(|(k, b)| (k.clone(), b.default_args.clone()))
            .collect();

        let changed = normalize_grouped_args(&mut config);
        assert!(
            !changed,
            "Config::default() must already be in grouped form"
        );

        let after_backend_args: std::collections::BTreeMap<String, Vec<String>> = config
            .backends
            .iter()
            .map(|(k, b)| (k.clone(), b.default_args.clone()))
            .collect();

        assert_eq!(
            before_backend_args, after_backend_args,
            "default backend default_args drifted"
        );
    }

    #[test]
    fn normalize_already_grouped_is_noop() {
        let mut config = Config::default();
        config.backends.insert(
            "grouped".to_string(),
            BackendConfig {
                path: None,
                default_args: vec!["-fa 1".to_string(), "-b 2048".to_string()],
                health_check_url: None,
                version: None,
            },
        );

        let changed = normalize_grouped_args(&mut config);
        assert!(!changed);
    }

    #[test]
    fn load_from_persists_migration_to_disk() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        let legacy_toml = r#"
[general]
log_level = "info"

[backends.llama_cpp]
default_args = ["-fa", "1", "-b", "2048"]
"#;
        std::fs::write(&config_path, legacy_toml).expect("write");

        let _config = Config::load_from(temp_dir.path()).expect("load");

        let after = std::fs::read_to_string(&config_path).expect("read after");
        // After load, the file on disk must contain grouped form.
        assert!(
            after.contains("\"-fa 1\""),
            "expected grouped -fa 1 in {}",
            after
        );
        assert!(
            after.contains("\"-b 2048\""),
            "expected grouped -b 2048 in {}",
            after
        );
        // The flat tokens must NOT remain.
        assert!(!after.contains("\"-fa\","), "flat -fa, leaked: {}", after);
    }

    #[test]
    fn load_from_already_grouped_does_not_rewrite() {
        // Pin the "don't churn already-grouped configs" invariant: if the
        // file on disk is already in grouped form, Config::load_from must
        // NOT rewrite it. We verify by snapshotting the byte content
        // before and after.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        let grouped_toml = r#"
[general]
log_level = "info"

[backends.llama_cpp]
default_args = ["-fa 1", "-b 2048"]
"#;
        std::fs::write(&config_path, grouped_toml).expect("write");
        let before = std::fs::read_to_string(&config_path).expect("read before");

        let _config = Config::load_from(temp_dir.path()).expect("load");

        let after = std::fs::read_to_string(&config_path).expect("read after");
        assert_eq!(
            before, after,
            "already-grouped config was rewritten unnecessarily.\nBefore:\n{}\nAfter:\n{}",
            before, after
        );
    }
}
