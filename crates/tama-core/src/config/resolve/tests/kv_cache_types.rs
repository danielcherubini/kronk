use tempfile::tempdir;

use super::super::*;

/// Tests that -ctk and -ctv flags are injected when cache_type_k/v are set
/// and backend is llama.cpp.
#[test]
fn test_kv_cache_type_args_injected_when_set() {
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

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
        num_parallel: None,
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: Some("q4_0".to_string()),
        cache_type_v: Some("q8_0".to_string()),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, None)
        .expect("build_full_args failed");

    // -ctk q4_0 should be present
    assert!(
        args.windows(2).any(|w| w == ["-ctk", "q4_0"]),
        "Expected -ctk q4_0 in args, got: {:?}",
        args
    );
    // -ctv q8_0 should be present
    assert!(
        args.windows(2).any(|w| w == ["-ctv", "q8_0"]),
        "Expected -ctv q8_0 in args, got: {:?}",
        args
    );
}

/// Tests that -ctk and -ctv are NOT injected when cache_type_k/v are None
/// on a llama.cpp backend.
#[test]
fn test_kv_cache_type_args_not_injected_when_none() {
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

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
        num_parallel: None,
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
    };

    let args = config
        .build_full_args(&server, &backend, None, None)
        .expect("build_full_args failed");

    // -ctk and -ctv should NOT be present
    assert!(
        !args.iter().any(|a| *a == "-ctk"),
        "Expected no -ctk when cache_type_k is None, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| *a == "-ctv"),
        "Expected no -ctv when cache_type_v is None, got: {:?}",
        args
    );
}

/// Tests that -ctk and -ctv are NOT injected for non-llama.cpp backends,
/// even when cache_type_k/v are set.
#[test]
fn test_kv_cache_type_args_not_injected_for_non_llama_backend() {
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let server = ModelConfig {
        backend: "ollama".to_string(),
        args: vec![],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: Some("q4_0".to_string()),
        cache_type_v: Some("q8_0".to_string()),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, None)
        .expect("build_full_args failed");

    // -ctk and -ctv should NOT be present for non-llama.cpp backends
    assert!(
        !args.iter().any(|a| *a == "-ctk"),
        "Expected no -ctk for non-llama.cpp backend, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| *a == "-ctv"),
        "Expected no -ctv for non-llama.cpp backend, got: {:?}",
        args
    );
}

/// Tests that -ctk and -ctv are not duplicated when already present in
/// user-provided args on a llama.cpp backend.
#[test]
fn test_kv_cache_type_args_no_duplicate_when_in_user_args() {
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["-ctk f16".to_string(), "-ctv f16".to_string()],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: None,
        num_parallel: None,
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: Some("q4_0".to_string()),
        cache_type_v: Some("q8_0".to_string()),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, None)
        .expect("build_full_args failed");

    // -ctk should appear exactly once (from args, not injected)
    let ctk_count = args.iter().filter(|a| *a == "-ctk").count();
    assert_eq!(
        ctk_count, 1,
        "Expected exactly one -ctk (no duplicate), got {} in: {:?}",
        ctk_count, args
    );
    // -ctv should appear exactly once
    let ctv_count = args.iter().filter(|a| *a == "-ctv").count();
    assert_eq!(
        ctv_count, 1,
        "Expected exactly one -ctv (no duplicate), got {} in: {:?}",
        ctv_count, args
    );
}

/// Tests that -ctk and -ctv are NOT injected when cache_type_k/v are empty
/// strings on a llama.cpp backend.
#[test]
fn test_kv_cache_type_args_not_injected_for_empty_string() {
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

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
        num_parallel: None,
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: Some("".to_string()),
        cache_type_v: Some("".to_string()),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let args = config
        .build_full_args(&server, &backend, None, None)
        .expect("build_full_args failed");

    assert!(
        !args.iter().any(|a| *a == "-ctk"),
        "Expected no -ctk when cache_type_k is empty string, got: {:?}",
        args
    );
    assert!(
        !args.iter().any(|a| *a == "-ctv"),
        "Expected no -ctv when cache_type_v is empty string, got: {:?}",
        args
    );
}
