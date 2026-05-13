use super::super::*;

#[test]
fn test_resolve_by_api_name() {
    let mut config = Config::default();
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            version: None,
            gpu_variant: None,
        },
    );

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model.Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: None,
        },
    );

    let mut models = std::collections::HashMap::new();
    models.insert(
        "my-custom-name".to_string(),
        ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("other-model-id".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: Some(8080),
            health_check: None,
            enabled: true,
            context_length: None,
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: Some("bartowski/Qwen3-8B-GGUF".to_string()),
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
            ..Default::default()
        },
    );

    // Should find model by api_name (not by model field)
    let results = config.resolve_servers_for_model(&models, "bartowski/Qwen3-8B-GGUF");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "my-custom-name");
}

#[test]
fn test_api_name_takes_priority() {
    let mut config = Config::default();
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            version: None,
            gpu_variant: None,
        },
    );

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model.Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: None,
        },
    );

    let mut models = std::collections::HashMap::new();
    models.insert(
        "slug".to_string(),
        ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("other-model".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: Some(8080),
            health_check: None,
            enabled: true,
            context_length: None,
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: Some("friendly-name".to_string()),
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
            ..Default::default()
        },
    );

    // Querying by "friendly-name" (api_name) should resolve correctly
    let results = config.resolve_servers_for_model(&models, "friendly-name");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "slug");
}

#[test]
fn test_backward_compat_no_api_name() {
    let mut config = Config::default();
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            version: None,
            gpu_variant: None,
        },
    );

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model.Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: None,
        },
    );

    let mut models = std::collections::HashMap::new();
    models.insert(
        "config-key-name".to_string(),
        ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: Some(8080),
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
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
            ..Default::default()
        },
    );

    // Should still resolve by config key
    let results = config.resolve_servers_for_model(&models, "config-key-name");
    assert_eq!(results.len(), 1);

    // Should also resolve by model field
    let results = config.resolve_servers_for_model(&models, "org/repo");
    assert_eq!(results.len(), 1);
}

#[test]
fn test_resolve_server_by_api_name() {
    let mut config = Config::default();
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            version: None,
            gpu_variant: None,
        },
    );

    let mut quants = std::collections::BTreeMap::new();
    quants.insert(
        "Q4_K_M".to_string(),
        crate::config::types::QuantEntry {
            file: "model.Q4_K_M.gguf".to_string(),
            kind: Default::default(),
            size_bytes: None,
            context_length: None,
        },
    );

    let mut models = std::collections::HashMap::new();
    models.insert(
        "my-custom-name".to_string(),
        ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("other-model-id".to_string()),
            quant: Some("Q4_K_M".to_string()),
            mmproj: None,
            port: Some(8080),
            health_check: None,
            enabled: true,
            context_length: None,
            num_parallel: Some(1),
            kv_unified: false,
            profile: None,
            api_name: Some("bartowski/Qwen3-8B-GGUF".to_string()),
            gpu_layers: None,
            cache_type_k: None,
            cache_type_v: None,
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
            ..Default::default()
        },
    );

    // Should find model by api_name via resolve_server
    let result = config.resolve_server(&models, "bartowski/Qwen3-8B-GGUF");
    assert!(result.is_ok());
}
