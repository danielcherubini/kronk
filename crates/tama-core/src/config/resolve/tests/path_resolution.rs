use crate::backends::{BackendInfo, BackendManager, BackendType};
use crate::config::BackendConfig;

use super::make_test_config;

fn insert_active_backend(
    manager: &BackendManager,
    name: &str,
    gpu_variant: &str,
    version: &str,
    path: &str,
) {
    let info = BackendInfo {
        name: name.to_string(),
        backend_type: BackendType::LlamaCpp,
        version: version.to_string(),
        path: std::path::PathBuf::from(path),
        installed_at: 0,
        gpu_type: None,
        gpu_variant: gpu_variant.to_string(),
        source: None,
    };
    manager.add_installation(&info).unwrap();
}

#[test]
fn test_resolve_backend_path_from_db() {
    let manager = BackendManager::open_in_memory().unwrap();
    insert_active_backend(
        &manager,
        "llama_cpp",
        "cpu",
        "v1.0.0",
        "/usr/local/bin/llama-server",
    );

    let config = make_test_config(None);
    let result = config
        .resolve_backend_path("llama_cpp", None, &manager)
        .unwrap();
    assert_eq!(
        result,
        std::path::PathBuf::from("/usr/local/bin/llama-server")
    );
}

#[test]
fn test_resolve_backend_path_fallback() {
    let manager = BackendManager::open_in_memory().unwrap();
    // Empty DB — no installed backend

    let config = make_test_config(Some("/fallback/llama-server"));
    let result = config
        .resolve_backend_path("llama_cpp", None, &manager)
        .unwrap();
    assert_eq!(result, std::path::PathBuf::from("/fallback/llama-server"));
}

#[test]
fn test_resolve_backend_path_error() {
    let manager = BackendManager::open_in_memory().unwrap();
    // Empty DB, path = None

    let config = make_test_config(None);
    let result = config.resolve_backend_path("llama_cpp", None, &manager);
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
    let manager = BackendManager::open_in_memory().unwrap();

    // Insert v1.0.0 and v2.0.0 (v2.0.0 will be active since added last)
    insert_active_backend(&manager, "llama_cpp", "cpu", "v1.0.0", "/v1/llama-server");
    insert_active_backend(&manager, "llama_cpp", "cpu", "v2.0.0", "/v2/llama-server");

    // Pin config to v1.0.0
    let mut config = make_test_config(None);
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: None,
            version: Some("v1.0.0".to_string()),
            gpu_variant: None,
        },
    );

    let result = config
        .resolve_backend_path("llama_cpp", None, &manager)
        .unwrap();
    // Should return v1 path, not v2 (which is active)
    assert_eq!(result, std::path::PathBuf::from("/v1/llama-server"));
}

#[test]
fn test_resolve_backend_path_version_pin_not_found() {
    let manager = BackendManager::open_in_memory().unwrap();
    // Empty DB — version pin won't find anything

    let mut config = make_test_config(None);
    config.backends.insert(
        "llama_cpp".to_string(),
        BackendConfig {
            path: None,
            version: Some("nonexistent".to_string()),
            gpu_variant: None,
        },
    );

    let result = config.resolve_backend_path("llama_cpp", None, &manager);
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
