use crate::config::BackendConfig;
use crate::db::queries::BackendInstallationRecord;
use crate::db::{open_in_memory, queries::insert_backend_installation};

use super::make_test_config;

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
        gpu_variant: "cpu".to_string(),
        source: None,
        is_active: false,
    };
    insert_backend_installation(&conn, &record).unwrap();

    let config = make_test_config(None);
    let result = config
        .resolve_backend_path("llama_cpp", None, &conn)
        .unwrap();
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
    let result = config
        .resolve_backend_path("llama_cpp", None, &conn)
        .unwrap();
    assert_eq!(result, std::path::PathBuf::from("/fallback/llama-server"));
}

#[test]
fn test_resolve_backend_path_error() {
    let crate::db::OpenResult { conn, .. } = open_in_memory().unwrap();
    // Empty DB, path = None

    let config = make_test_config(None);
    let result = config.resolve_backend_path("llama_cpp", None, &conn);
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
        gpu_variant: "cpu".to_string(),
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
        gpu_variant: "cpu".to_string(),
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
            gpu_variant: None,
        },
    );

    let result = config
        .resolve_backend_path("llama_cpp", None, &conn)
        .unwrap();
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
            gpu_variant: None,
        },
    );

    let result = config.resolve_backend_path("llama_cpp", None, &conn);
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
