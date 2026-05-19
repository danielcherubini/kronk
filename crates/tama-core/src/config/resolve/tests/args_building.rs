use std::collections::BTreeMap;

use tempfile::tempdir;

use crate::config::types::{QuantEntry, SpecDecodingConfig};

use super::super::*;

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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    // ctx_override should take priority over server.context_length
    let args = config
        .build_full_args(&server, &backend, Some(2048), &[])
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        cache_type_k: None,
        cache_type_v: None,
        quants: BTreeMap::new(), // Empty quants map
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    // Should not crash when quants is empty
    let args = config.build_full_args(&server, &backend, None, &[]);
    assert!(args.is_ok());

    // Should not emit -m arg when quant lookup fails
    let args = args.expect("build_full_args failed");
    assert!(!args.iter().any(|a| a == "-m"));
}

/// Tests that inline temperature in args is overridden by sampling params
#[test]
fn test_build_args_sampling_overrides_inline_temp_in_args() {
    // Requires SamplingParams::to_args to already be in grouped form
    // (done earlier in this same task, section 2a.1). If this test
    // fails with a flat-token mismatch instead of a dedup failure,
    // the to_args rewrite was skipped.
    let mut config = Config::default();
    config.backends.insert(
        "test_backend".to_string(),
        BackendConfig {
            path: None,
            version: None,
            gpu_variant: None,
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        cache_type_k: None,
        cache_type_v: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        db_id: None,
        ..Default::default()
    };

    let backend = config.backends.get("test_backend").unwrap().clone();
    let flat = config.build_args(&server, &backend, &[]);

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

/// Tests that flat tokens are preserved with quoted paths in full args
#[test]
fn test_build_full_args_returns_flat_tokens_with_quoted_path() {
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
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
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

/// Tests that context length is multiplied by num_parallel in build_full_args.
#[test]
fn test_build_full_args_context_multiplied_by_num_parallel() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // context_length=4096, num_parallel=2 → effective ctx = 8192
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
        context_length: Some(4096),
        num_parallel: Some(2),
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Context should be 4096 * 2 = 8192
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"8192".to_string()),
        "Expected -c 8192 (4096*2), got: {:?}",
        args
    );
    // Raw context value should NOT appear alone
    assert!(
        !args.contains(&"4096".to_string()),
        "Raw context 4096 should not appear, got: {:?}",
        args
    );
}

/// Tests that saturating_mul prevents overflow for large context × num_parallel.
#[test]
fn test_build_full_args_context_saturating_overflow() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // context_length=1_000_000, num_parallel=10_000
    // 1_000_000 * 10_000 = 10_000_000_000 > u32::MAX (4_294_967_295)
    // saturating_mul should clamp to u32::MAX without panicking
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
        context_length: Some(1_000_000),
        num_parallel: Some(10_000),
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    // Should not panic — saturating_mul clamps to u32::MAX
    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args should not panic with large values");

    assert!(args.contains(&"-c".to_string()));
    // Should be clamped to u32::MAX (4294967295), not overflow
    assert!(
        args.contains(&"4294967295".to_string()),
        "Expected -c 4294967295 (u32::MAX from saturating_mul), got: {:?}",
        args
    );
}

/// Tests that context is NOT multiplied when num_parallel is None (defaults to 1).
#[test]
fn test_build_full_args_context_no_num_parallel_defaults_to_one() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // num_parallel is None → should default to 1, so ctx stays at 8192
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
        context_length: Some(8192),
        num_parallel: None, // No parallel setting
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Context should be 8192 * 1 = 8192 (unchanged)
    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"8192".to_string()));
}

/// Tests that -np flag is injected when num_parallel > 1.
#[test]
fn test_build_full_args_injects_np_flag() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // num_parallel=2 → should inject -np 2
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
        context_length: Some(8192),
        num_parallel: Some(2),
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -np flag should be present with value 2
    assert!(
        args.contains(&"-np".to_string()),
        "Expected -np flag in args, got: {:?}",
        args
    );
    assert!(
        args.contains(&"2".to_string()),
        "Expected value 2 after -np, got: {:?}",
        args
    );
    // -c should still be multiplied by num_parallel
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"16384".to_string()),
        "Expected -c 16384 (8192*2), got: {:?}",
        args
    );
}

/// Tests that -np flag is NOT injected when num_parallel is None or 1.
#[test]
fn test_build_full_args_no_np_when_default() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // num_parallel=1 → should NOT inject -np (it's the default)
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
        context_length: Some(8192),
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
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // -np should NOT be present when num_parallel is 1
    assert!(
        !args.contains(&"-np".to_string()),
        "Expected no -np flag when num_parallel=1, got: {:?}",
        args
    );
}

/// Tests that kv_unified=true uses per-slot context (no multiplication).
#[test]
fn test_build_full_args_unified_n_slots() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // kv_unified=true, num_parallel=4, context_length=8192 → -c 8192 (not 32768)
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
        context_length: Some(8192),
        num_parallel: Some(4),
        kv_unified: true, // Unified KV
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // With kv_unified=true, -c should be per-slot context (8192), not multiplied
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"8192".to_string()),
        "Expected -c 8192 (unified: no multiplication), got: {:?}",
        args
    );
    // --kv-unified flag should be injected
    assert!(
        args.contains(&"--kv-unified".to_string()),
        "Expected --kv-unified flag in args, got: {:?}",
        args
    );
}

/// Tests that kv_unified=false uses context_length * num_parallel.
#[test]
fn test_build_full_args_non_unified_n_slots() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // kv_unified=false, num_parallel=4, context_length=8192 → -c 32768
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
        context_length: Some(8192),
        num_parallel: Some(4),
        kv_unified: false, // Non-unified (default)
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // With kv_unified=false, -c should be 8192 * 4 = 32768
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"32768".to_string()),
        "Expected -c 32768 (non-unified: 8192*4), got: {:?}",
        args
    );
    // --kv-unified flag should NOT be injected
    assert!(
        !args.contains(&"--kv-unified".to_string()),
        "Expected no --kv-unified flag when kv_unified=false, got: {:?}",
        args
    );
}

/// Tests that default (kv_unified omitted/false) preserves non-unified behavior.
#[test]
fn test_build_full_args_unified_default() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // kv_unified defaults to false via serde, num_parallel=2 → -c = 8192 * 2 = 16384
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        context_length: Some(8192),
        num_parallel: Some(2),
        quants,
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Default (false) should use non-unified formula: 8192 * 2 = 16384
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"16384".to_string()),
        "Expected -c 16384 (default non-unified: 8192*2), got: {:?}",
        args
    );
    // --kv-unified flag should NOT be injected
    assert!(
        !args.contains(&"--kv-unified".to_string()),
        "Expected no --kv-unified flag with default kv_unified, got: {:?}",
        args
    );
}

/// Tests that ctx_override is treated as raw per-slot context with unified KV.
#[test]
fn test_build_full_args_ctx_override_unified() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // ctx_override=Some(4096), kv_unified=true, num_parallel=3 → -c 4096 (not 12288)
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
        context_length: Some(8192), // Ignored because ctx_override takes priority
        num_parallel: Some(3),
        kv_unified: true, // Unified KV
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    // ctx_override=4096, kv_unified=true → -c 4096 (not 12288)
    let args = config
        .build_full_args(&server, &backend, Some(4096), &[])
        .expect("build_full_args failed");

    // With kv_unified=true and ctx_override=4096, -c should be 4096 (per-slot)
    assert!(args.contains(&"-c".to_string()));
    assert!(
        args.contains(&"4096".to_string()),
        "Expected -c 4096 (unified ctx_override), got: {:?}",
        args
    );
    // --kv-unified flag should be injected
    assert!(
        args.contains(&"--kv-unified".to_string()),
        "Expected --kv-unified flag in args, got: {:?}",
        args
    );
}

/// Tests that --kv-unified is not duplicated when the user manually adds it
/// in their args array AND server.kv_unified=true.
#[test]
fn test_build_full_args_kv_unified_not_duplicated_when_in_user_args() {
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
            context_length: Some(8192),
        },
    );

    let mut config = Config::default();
    config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
    config.loaded_from = Some(temp_dir.path().to_path_buf());

    // User manually added --kv-unified in args, AND kv_unified=true in config.
    // The flag should appear exactly once (not duplicated).
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["--kv-unified".to_string()], // User manually added it
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
        num_parallel: Some(2),
        kv_unified: true, // Config also says unified
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

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    let kv_count = args.iter().filter(|a| *a == "--kv-unified").count();
    assert_eq!(
        kv_count, 1,
        "--kv-unified should appear exactly once, got {} in: {:?}",
        kv_count, args
    );
}

/// Tests that spec decoding flags (--spec-type, --spec-draft-n-max, --spec-draft-n-min)
/// are injected when spec_decoding is configured on a llama.cpp backend.
#[test]
fn test_spec_decoding_flags_injected() {
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
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should be injected with comma-separated types
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"draft-mtp,ngram-simple".to_string()),
        "Expected spec types value, got: {:?}",
        args
    );

    // --spec-draft-n-max should be injected
    assert!(
        args.contains(&"--spec-draft-n-max".to_string()),
        "Expected --spec-draft-n-max flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"4".to_string()),
        "Expected n_max=4, got: {:?}",
        args
    );

    // --spec-draft-n-min should be injected
    assert!(
        args.contains(&"--spec-draft-n-min".to_string()),
        "Expected --spec-draft-n-min flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"2".to_string()),
        "Expected n_min=2, got: {:?}",
        args
    );

    // --spec-draft-ngl should be injected (draft-mtp is in spec_types)
    assert!(
        args.contains(&"--spec-draft-ngl".to_string()),
        "Expected --spec-draft-ngl flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"16".to_string()),
        "Expected draft_ngl=16, got: {:?}",
        args
    );
}

/// Tests that if the user already has --spec-type in their args, we don't inject another.
#[test]
fn test_spec_decoding_no_duplicate_when_in_args() {
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

    // User manually added --spec-type in args
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec!["--spec-type draft-mtp".to_string()],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string(), "ngram-simple".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: None,
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should appear exactly once (user's version, not duplicated)
    let spec_type_count = args.iter().filter(|a| *a == "--spec-type").count();
    assert_eq!(
        spec_type_count, 1,
        "--spec-type should appear exactly once, got {} in: {:?}",
        spec_type_count, args
    );

    // n_max and n_min should still be injected (they weren't in user args)
    assert!(
        args.contains(&"--spec-draft-n-max".to_string()),
        "Expected --spec-draft-n-max flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"--spec-draft-n-min".to_string()),
        "Expected --spec-draft-n-min flag, got: {:?}",
        args
    );
}

/// Tests that --spec-draft-ngl is only injected when "draft-mtp" is in spec_types.
#[test]
fn test_spec_decoding_draft_ngl_only_for_mtp() {
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

    // spec_types does NOT contain "draft-mtp", so draft_ngl should NOT be injected
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
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["ngram-simple".to_string()], // No draft-mtp
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16), // Set but should be ignored without draft-mtp
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type should be injected
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );

    // --spec-draft-ngl should NOT be injected (no draft-mtp in spec_types)
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl when draft-mtp not in spec_types, got: {:?}",
        args
    );
}

/// Tests that multiple spec_types are joined with commas in --spec-type.
#[test]
fn test_spec_decoding_multi_type_comma_separated() {
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
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec![
                "draft-mtp".to_string(),
                "ngram-simple".to_string(),
                "ngram-mod".to_string(),
            ],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: None,
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-type value should be comma-separated
    assert!(
        args.contains(&"--spec-type".to_string()),
        "Expected --spec-type flag, got: {:?}",
        args
    );
    assert!(
        args.contains(&"draft-mtp,ngram-simple,ngram-mod".to_string()),
        "Expected comma-separated spec types, got: {:?}",
        args
    );
}

/// Tests that a non-llama backend does NOT inject spec decoding flags.
#[test]
fn test_spec_decoding_non_llama_backend_no_flags() {
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

    // Use a non-llama backend
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
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // No spec decoding flags should be present for non-llama backend
    assert!(
        !args.contains(&"--spec-type".to_string()),
        "Expected no --spec-type for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-max".to_string()),
        "Expected no --spec-draft-n-max for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-min".to_string()),
        "Expected no --spec-draft-n-min for non-llama backend, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl for non-llama backend, got: {:?}",
        args
    );
}

/// Tests that each of the 4 spec decoding flags has its own already_has guard,
/// so pre-existing flags in user args are not duplicated.
#[test]
fn test_spec_decoding_all_already_has_checks() {
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

    // User already has all 4 flags in args
    let server = ModelConfig {
        backend: "llama_cpp".to_string(),
        args: vec![
            "--spec-type user-type".to_string(),
            "--spec-draft-n-max 8".to_string(),
            "--spec-draft-n-min 1".to_string(),
            "--spec-draft-ngl 32".to_string(),
        ],
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // Each flag should appear exactly once (user's version, not duplicated)
    let spec_type_count = args.iter().filter(|a| *a == "--spec-type").count();
    assert_eq!(
        spec_type_count, 1,
        "--spec-type should appear exactly once, got {} in: {:?}",
        spec_type_count, args
    );

    let n_max_count = args.iter().filter(|a| *a == "--spec-draft-n-max").count();
    assert_eq!(
        n_max_count, 1,
        "--spec-draft-n-max should appear exactly once, got {} in: {:?}",
        n_max_count, args
    );

    let n_min_count = args.iter().filter(|a| *a == "--spec-draft-n-min").count();
    assert_eq!(
        n_min_count, 1,
        "--spec-draft-n-min should appear exactly once, got {} in: {:?}",
        n_min_count, args
    );

    let ngl_count = args.iter().filter(|a| *a == "--spec-draft-ngl").count();
    assert_eq!(
        ngl_count, 1,
        "--spec-draft-ngl should appear exactly once, got {} in: {:?}",
        ngl_count, args
    );
}

/// Tests that draft_ngl=99 is injected as-is (not truncated, not quoted).
#[test]
fn test_spec_decoding_draft_ngl_value_99() {
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
        sampling: None,
        model: Some("org/repo".to_string()),
        quant: Some("Q4_K_M".to_string()),
        mmproj: None,
        port: None,
        health_check: None,
        enabled: true,
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec!["draft-mtp".to_string()],
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(99),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // --spec-draft-ngl should be present
    assert!(
        args.contains(&"--spec-draft-ngl".to_string()),
        "Expected --spec-draft-ngl flag, got: {:?}",
        args
    );

    // Value should be "99" — not truncated, not quoted
    assert!(
        args.contains(&"99".to_string()),
        "Expected draft_ngl value 99, got: {:?}",
        args
    );

    // Verify the value is exactly "99" (not "9" or "'99'")
    let ngl_pos = args
        .iter()
        .position(|a| a == "--spec-draft-ngl")
        .expect("--spec-draft-ngl not found");
    let ngl_value = &args[ngl_pos + 1];
    assert_eq!(
        ngl_value, "99",
        "draft_ngl value should be exactly '99', got '{}'",
        ngl_value
    );
}

/// Tests that empty spec_types produces no spec decoding flags.
#[test]
fn test_spec_decoding_empty_types_no_flags() {
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

    // Empty spec_types → no spec decoding flags should be injected
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
        context_length: Some(8192),
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
        spec_decoding: SpecDecodingConfig {
            spec_types: vec![], // Empty
            n_max: Some(4),
            n_min: Some(2),
            draft_ngl: Some(16),
        },
        ..Default::default()
    };

    let backend = BackendConfig {
        path: None,
        version: None,
        gpu_variant: None,
    };

    let args = config
        .build_full_args(&server, &backend, None, &[])
        .expect("build_full_args failed");

    // No spec decoding flags should be present
    assert!(
        !args.contains(&"--spec-type".to_string()),
        "Expected no --spec-type when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-max".to_string()),
        "Expected no --spec-draft-n-max when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-n-min".to_string()),
        "Expected no --spec-draft-n-min when spec_types is empty, got: {:?}",
        args
    );
    assert!(
        !args.contains(&"--spec-draft-ngl".to_string()),
        "Expected no --spec-draft-ngl when spec_types is empty, got: {:?}",
        args
    );
}
