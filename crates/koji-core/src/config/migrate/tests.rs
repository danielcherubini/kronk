use std::collections::{BTreeMap, HashMap};
use std::fs;

use tempfile::tempdir;

use crate::profiles::SamplingParams;

use super::*;

/// Tests migration of model cards to unified config format
#[test]
fn test_migrate_cards_to_unified() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path();

    // Create configs/ directory
    let configs_dir = config_dir.join("configs");
    fs::create_dir_all(&configs_dir).unwrap();

    // Create config.toml
    let config_path = config_dir.join("config.toml");
    let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/repo"
quant = "Q4_K_M"
"#;
    fs::write(&config_path, config_toml).unwrap();

    // Create model card: configs/org--repo.toml
    let card_path = configs_dir.join("org--repo.toml");
    let card_toml = r#"
[model]
name = "TestModel"
source = "org/repo"
default_gpu_layers = 99
default_context_length = 8192

[quants.Q4_K_M]
file = "model-Q4_K_M.gguf"
size_bytes = 4000000000

[sampling.coding]
temperature = 0.2
top_k = 40
"#;
    fs::write(&card_path, card_toml).unwrap();

    // Setup config object
    let mut config = Config {
        general: crate::config::types::General::default(),
        backends: HashMap::new(),
        models: {
            let mut m = HashMap::new();
            let model = crate::config::ModelConfig {
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
                api_name: None,
                gpu_layers: None,
                quants: BTreeMap::new(),
                modalities: None,
            };
            m.insert("test-model".to_string(), model);
            m
        },
        supervisor: crate::config::types::Supervisor::default(),
        sampling_templates: {
            let mut t = HashMap::new();
            t.insert(
                "coding".to_string(),
                SamplingParams {
                    temperature: Some(0.2),
                    top_p: Some(0.9),
                    top_k: Some(40),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.1),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
            );
            t
        },
        proxy: crate::config::types::ProxyConfig::default(),
        loaded_from: Some(config_dir.to_path_buf()),
    };

    // Run migration
    migrate_cards_to_unified_config(&mut config).unwrap();

    // Assertions
    let model_config = config.models.get("test-model").unwrap();
    assert_eq!(model_config.api_name, Some("org/repo".to_string()));
    assert_eq!(model_config.gpu_layers, Some(99));
    assert_eq!(model_config.context_length, Some(8192));
    assert_eq!(
        model_config.quants.get("Q4_K_M").unwrap().file,
        "model-Q4_K_M.gguf"
    );
    assert_eq!(
        model_config.sampling.as_ref().unwrap().temperature,
        Some(0.2)
    );

    // Card file should be gone
    assert!(!card_path.exists());

    // Backup file should exist
    assert!(config_dir
        .join("config.toml.pre-unified-migration")
        .exists());
}

/// Tests that migration is idempotent when run multiple times
#[test]
fn test_migrate_idempotent() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path();

    // Create configs/ directory
    let configs_dir = config_dir.join("configs");
    fs::create_dir_all(&configs_dir).unwrap();

    // Create config.toml
    let config_path = config_dir.join("config.toml");
    let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/repo"
quant = "Q4_K_M"
"#;
    fs::write(&config_path, config_toml).unwrap();

    // Create model card
    let card_path = configs_dir.join("org--repo.toml");
    let card_toml = r#"
[model]
name = "TestModel"
source = "org/repo"
default_gpu_layers = 99
default_context_length = 8192

[quants.Q4_K_M]
file = "model-Q4_K_M.gguf"
size_bytes = 4000000000

[sampling.coding]
temperature = 0.2
top_k = 40
"#;
    fs::write(&card_path, card_toml).unwrap();

    // Setup config object
    let mut config = Config {
        general: crate::config::types::General::default(),
        backends: HashMap::new(),
        models: {
            let mut m = HashMap::new();
            let model = crate::config::ModelConfig {
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
                api_name: None,
                gpu_layers: None,
                quants: BTreeMap::new(),
                modalities: None,
            };
            m.insert("test-model".to_string(), model);
            m
        },
        supervisor: crate::config::types::Supervisor::default(),
        sampling_templates: {
            let mut t = HashMap::new();
            t.insert(
                "coding".to_string(),
                SamplingParams {
                    temperature: Some(0.3),
                    top_p: Some(0.9),
                    top_k: Some(50),
                    min_p: Some(0.05),
                    presence_penalty: Some(0.1),
                    frequency_penalty: None,
                    repeat_penalty: None,
                },
            );
            t
        },
        proxy: crate::config::types::ProxyConfig::default(),
        loaded_from: Some(config_dir.to_path_buf()),
    };

    // First migration
    migrate_cards_to_unified_config(&mut config).unwrap();

    // Verify migration happened
    let model_config = config.models.get("test-model").unwrap();
    assert_eq!(model_config.api_name, Some("org/repo".to_string()));
    assert_eq!(model_config.gpu_layers, Some(99));
    assert_eq!(model_config.context_length, Some(8192));
    assert!(!card_path.exists());

    // Second migration - should be no-op
    migrate_cards_to_unified_config(&mut config).unwrap();

    // Verify nothing changed
    let model_config = config.models.get("test-model").unwrap();
    assert_eq!(model_config.api_name, Some("org/repo".to_string()));
    assert_eq!(model_config.gpu_layers, Some(99));
    assert_eq!(model_config.context_length, Some(8192));
}

/// Tests that existing quants field is preserved during migration
#[test]
fn test_migrate_preserves_existing_quants() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path();

    // Create configs/ directory
    let configs_dir = config_dir.join("configs");
    fs::create_dir_all(&configs_dir).unwrap();

    // Create config.toml with existing quants
    let config_path = config_dir.join("config.toml");
    let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/repo"
quant = "Q4_K_M"
"#;
    fs::write(&config_path, config_toml).unwrap();

    // Create model card with different quant data
    let card_path = configs_dir.join("org--repo.toml");
    let card_toml = r#"
[model]
name = "TestModel"
source = "org/repo"

[quants.Q4_K_M]
file = "different-file.gguf"
size_bytes = 9999999999

[quants.Q8_0]
file = "model-Q8_0.gguf"
size_bytes = 8000000000
"#;
    fs::write(&card_path, card_toml).unwrap();

    // Setup config object with existing quants
    let mut config = Config {
        general: crate::config::types::General::default(),
        backends: HashMap::new(),
        models: {
            let mut m = HashMap::new();
            let model = crate::config::ModelConfig {
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
                api_name: None,
                gpu_layers: None,
                quants: {
                    let mut quants = std::collections::BTreeMap::new();
                    quants.insert(
                        "Q4_K_M".to_string(),
                        crate::config::types::QuantEntry {
                            file: "existing-model-Q4_K_M.gguf".to_string(),
                            kind: Default::default(),
                            size_bytes: Some(1000000000),
                            context_length: None,
                        },
                    );
                    quants
                },
                modalities: None,
            };
            m.insert("test-model".to_string(), model);
            m
        },
        supervisor: crate::config::types::Supervisor::default(),
        sampling_templates: HashMap::new(),
        proxy: crate::config::types::ProxyConfig::default(),
        loaded_from: Some(config_dir.to_path_buf()),
    };

    // Run migration
    migrate_cards_to_unified_config(&mut config).unwrap();

    // Verify existing quant was NOT overwritten
    let model_config = config.models.get("test-model").unwrap();
    let existing_quant = model_config.quants.get("Q4_K_M").unwrap();
    assert_eq!(existing_quant.file, "existing-model-Q4_K_M.gguf");
    assert_eq!(existing_quant.size_bytes, Some(1000000000));

    // New quant from card should be added
    assert!(model_config.quants.contains_key("Q8_0"));
    assert_eq!(
        model_config.quants.get("Q8_0").unwrap().file,
        "model-Q8_0.gguf"
    );

    // api_name should be set from model field (HF repo ID)
    assert_eq!(model_config.api_name, Some("org/repo".to_string()));
}

/// Helper that builds a minimal `ModelConfig` with a quants map containing
/// a Mmproj entry, used by the cleanup tests.
fn build_test_model_config_with_mmproj(
    args: Vec<&str>,
    mmproj: Option<&str>,
) -> crate::config::types::ModelConfig {
    let mut quants = BTreeMap::new();
    quants.insert(
        "mmproj-F16".to_string(),
        crate::config::types::QuantEntry {
            file: "mmproj-F16.gguf".to_string(),
            kind: crate::config::QuantKind::Mmproj,
            size_bytes: None,
            context_length: None,
        },
    );
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model-Q4_K_M.gguf".to_string(),
            kind: crate::config::QuantKind::Model,
            size_bytes: None,
            context_length: None,
        },
    );
    crate::config::types::ModelConfig {
        backend: "llama_cpp".to_string(),
        args: args.into_iter().map(String::from).collect(),
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: mmproj.map(String::from),
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants,
        modalities: None,
    }
}

/// Tests that grouped --mmproj arg is stripped and selection recovered from filename
#[test]
fn test_cleanup_strips_grouped_mmproj_arg_and_recovers_selection() {
    let mut config = Config::default();
    config.models.insert(
        "m".to_string(),
        build_test_model_config_with_mmproj(
            vec![
                "-fa 1",
                "--mmproj models/org/repo/mmproj-F16.gguf",
                "-c 4096",
            ],
            None,
        ),
    );

    let changed = cleanup_stale_mmproj_args(&mut config);

    assert!(changed);
    let m = &config.models["m"];
    assert_eq!(m.args, vec!["-fa 1".to_string(), "-c 4096".to_string()]);
    // Selection recovered from filename match.
    assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
}

/// Tests that two-token --mmproj arg is stripped and selection recovered
#[test]
fn test_cleanup_strips_two_token_mmproj_arg() {
    let mut config = Config::default();
    config.models.insert(
        "m".to_string(),
        build_test_model_config_with_mmproj(
            vec![
                "-fa 1",
                "--mmproj",
                "models/org/repo/mmproj-F16.gguf",
                "-c 4096",
            ],
            None,
        ),
    );

    let changed = cleanup_stale_mmproj_args(&mut config);

    assert!(changed);
    let m = &config.models["m"];
    assert_eq!(m.args, vec!["-fa 1".to_string(), "-c 4096".to_string()]);
    assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
}

/// Tests that inline equals --mmproj=arg is stripped and selection recovered
#[test]
fn test_cleanup_strips_inline_equals_mmproj_arg() {
    let mut config = Config::default();
    config.models.insert(
        "m".to_string(),
        build_test_model_config_with_mmproj(
            vec!["-fa 1", "--mmproj=models/org/repo/mmproj-F16.gguf"],
            None,
        ),
    );

    let changed = cleanup_stale_mmproj_args(&mut config);

    assert!(changed);
    let m = &config.models["m"];
    assert_eq!(m.args, vec!["-fa 1".to_string()]);
    assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
}

/// Tests that pre-existing mmproj field is preserved when cleaning stale args
#[test]
fn test_cleanup_preserves_existing_mmproj_field() {
    // If the user has already set mmproj via the editor, the recovered
    // value from a stale arg must NOT overwrite it.
    let mut config = Config::default();
    config.models.insert(
        "m".to_string(),
        build_test_model_config_with_mmproj(
            vec!["--mmproj models/org/repo/some-other-name.gguf"],
            Some("mmproj-F16"),
        ),
    );

    let changed = cleanup_stale_mmproj_args(&mut config);

    assert!(changed);
    let m = &config.models["m"];
    assert!(m.args.is_empty());
    // Pre-existing selection preserved.
    assert_eq!(m.mmproj.as_deref(), Some("mmproj-F16"));
}

/// Tests that cleanup returns false when no stale mmproj args are present
#[test]
fn test_cleanup_returns_false_when_no_stale_args() {
    let mut config = Config::default();
    config.models.insert(
        "m".to_string(),
        build_test_model_config_with_mmproj(vec!["-fa 1", "-c 4096"], Some("mmproj-F16")),
    );

    let changed = cleanup_stale_mmproj_args(&mut config);

    assert!(!changed);
    let m = &config.models["m"];
    assert_eq!(m.args, vec!["-fa 1".to_string(), "-c 4096".to_string()]);
}

#[test]
fn test_api_name_derived_from_model_field_without_card() {
    let dir = tempdir().unwrap();
    let config_dir = dir.path();

    // Create configs/ directory (empty - no card files)
    let configs_dir = config_dir.join("configs");
    fs::create_dir_all(&configs_dir).unwrap();

    // Create config.toml with api_name: None and model: Some(...)
    let config_path = config_dir.join("config.toml");
    let config_toml = r#"
[models.test-model]
backend = "llama_cpp"
model = "org/model-name"
quant = "Q4_K_M"
"#;
    fs::write(&config_path, config_toml).unwrap();

    // Setup config object with api_name: None and model: Some("org/model-name")
    let mut config = Config {
        general: crate::config::types::General::default(),
        backends: HashMap::new(),
        models: {
            let mut m = HashMap::new();
            let model = crate::config::ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![],
                sampling: None,
                model: Some("org/model-name".to_string()),
                quant: Some("Q4_K_M".to_string()),
                mmproj: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                api_name: None, // This is what should be derived during migration
                gpu_layers: None,
                quants: BTreeMap::new(),
                modalities: None,
            };
            m.insert("test-model".to_string(), model);
            m
        },
        supervisor: crate::config::types::Supervisor::default(),
        sampling_templates: HashMap::new(),
        proxy: crate::config::types::ProxyConfig::default(),
        loaded_from: Some(config_dir.to_path_buf()),
    };

    // Run migration (no card files exist)
    migrate_cards_to_unified_config(&mut config).unwrap();

    // After migration, api_name should be derived from model field
    let model_config = config.models.get("test-model").unwrap();
    assert_eq!(model_config.api_name, Some("org/model-name".to_string()));
}
