use std::collections::BTreeMap;

use tempfile::tempdir;

use crate::config::types::QuantEntry;
use crate::config::BackendConfig;
use crate::db::queries::BackendInstallationRecord;
use crate::db::{open_in_memory, queries::insert_backend_installation};

use super::*;

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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        quants: BTreeMap::new(), // Empty quants map
        modalities: None,
        display_name: None,
        db_id: None,
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

/// Tests that backend flags are deduplicated when both backend and model args contain them
#[test]
fn test_build_args_dedupes_backend_vs_model_flags() {
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        db_id: None,
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants: std::collections::BTreeMap::new(),
        modalities: None,
        display_name: None,
        db_id: None,
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

/// Tests that backend flags are deduplicated in full args when both backend and model args contain them
#[test]
fn test_build_full_args_dedupes_backend_vs_model_flags() {
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
        num_parallel: Some(1),
        kv_unified: false,
        profile: None,
        api_name: None,
        gpu_layers: Some(99),
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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

#[test]
fn test_resolve_by_api_name() {
    let mut config = Config::default();
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: Some("/usr/local/bin/llama-server".to_string()),
            default_args: vec![],
            health_check_url: None,
            version: None,
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
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
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
            default_args: vec![],
            health_check_url: None,
            version: None,
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
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
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
            default_args: vec![],
            health_check_url: None,
            version: None,
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
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
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
            default_args: vec![],
            health_check_url: None,
            version: None,
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
            quants,
            modalities: None,
            display_name: None,
            db_id: None,
        },
    );

    // Should find model by api_name via resolve_server
    let result = config.resolve_server(&models, "bartowski/Qwen3-8B-GGUF");
    assert!(result.is_ok());
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
    };

    let backend = BackendConfig {
        path: None,
        default_args: vec![],
        health_check_url: None,
        version: None,
    };

    // Should not panic — saturating_mul clamps to u32::MAX
    let args = config
        .build_full_args(&server, &backend, None)
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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

    // -np should NOT be present when num_parallel is 1
    assert!(
        !args.contains(&"-np".to_string()),
        "Expected no -np flag when num_parallel=1, got: {:?}",
        args
    );
}

/// Tests that -np is NOT injected when already present in backend/server args.
#[test]
fn test_build_full_args_skips_np_when_already_present() {
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

    // Backend already has -np 4, server has num_parallel=2 → should not inject another -np
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
    };

    let backend = BackendConfig {
        path: None,
        default_args: vec!["-np 4".to_string()],
        health_check_url: None,
        version: None,
    };

    let args = config
        .build_full_args(&server, &backend, None)
        .expect("build_full_args failed");

    // Should have exactly one -np (from backend), not two
    let np_count = args.iter().filter(|a| *a == "-np").count();
    assert_eq!(
        np_count, 1,
        "Expected exactly one -np flag, got {} in: {:?}",
        np_count, args
    );
    // The value should be 4 (from backend), not 2
    let np_idx = args.iter().position(|a| *a == "-np").unwrap();
    assert_eq!(
        args[np_idx + 1],
        "4",
        "Expected -np value to be 4 (from backend), got {:?}",
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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

    // kv_unified defaults to false, num_parallel=2 → -c = 8192 * 2 = 16384
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
        kv_unified: false, // Default (false)
        profile: None,
        api_name: None,
        gpu_layers: None,
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
    };

    let backend = BackendConfig {
        path: None,
        default_args: vec![],
        health_check_url: None,
        version: None,
    };

    // ctx_override=4096, kv_unified=true → -c 4096 (not 12288)
    let args = config
        .build_full_args(&server, &backend, Some(4096))
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
        quants,
        modalities: None,
        display_name: None,
        db_id: None,
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

    let kv_count = args.iter().filter(|a| *a == "--kv-unified").count();
    assert_eq!(
        kv_count, 1,
        "--kv-unified should appear exactly once, got {} in: {:?}",
        kv_count, args
    );
}
