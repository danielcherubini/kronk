/// Tests for extract_koji_flags helper function
use koji::extract_koji_flags;

#[test]
fn test_extract_model_huggingface_ref() {
    let args = vec!["--model".to_string(), "unsloth/Qwen3.5-0.8B".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.model, Some("unsloth/Qwen3.5-0.8B".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_model_local_path() {
    let args = vec!["--model".to_string(), "/path/to/file.gguf".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert!(result.model.is_none());
    assert_eq!(
        result.remaining_args,
        vec!["--model".to_string(), "/path/to/file.gguf".to_string()]
    );
}

#[test]
fn test_extract_model_short_flag() {
    let args = vec!["-m".to_string(), "/path/to/file.gguf".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert!(result.model.is_none());
    assert_eq!(
        result.remaining_args,
        vec!["-m".to_string(), "/path/to/file.gguf".to_string()]
    );
}

#[test]
fn test_extract_profile() {
    let args = vec!["--profile".to_string(), "chat".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.profile, Some("chat".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_quant() {
    let args = vec!["--quant".to_string(), "Q4_K_M".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.quant, Some("Q4_K_M".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_port() {
    let args = vec!["--port".to_string(), "8081".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.port, Some(8081));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_context_length() {
    let args = vec!["--ctx".to_string(), "8192".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.context_length, Some(8192));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_mixed_koji_and_backend_flags() {
    let args = vec![
        "--model".to_string(),
        "unsloth/Qwen3.5-0.8B".to_string(),
        "--quant".to_string(),
        "Q8_0".to_string(),
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "-t".to_string(),
        "8".to_string(),
    ];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.model, Some("unsloth/Qwen3.5-0.8B".to_string()));
    assert_eq!(result.quant, Some("Q8_0".to_string()));
    assert_eq!(result.port, None);
    assert_eq!(result.context_length, None);
    assert_eq!(
        result.remaining_args,
        vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "-t".to_string(),
            "8".to_string(),
        ]
    );
}

#[test]
fn test_no_koji_flags_all_remain() {
    let args = vec![
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "-t".to_string(),
        "8".to_string(),
    ];
    let result = extract_koji_flags(args).unwrap();
    assert!(result.model.is_none());
    assert!(result.quant.is_none());
    assert!(result.port.is_none());
    assert!(result.context_length.is_none());
    assert_eq!(
        result.remaining_args,
        vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "-t".to_string(),
            "8".to_string(),
        ]
    );
}

#[test]
fn test_model_without_value_returns_error() {
    let args = vec!["--model".to_string()];
    let result = extract_koji_flags(args);
    assert!(result.is_err());
}

#[test]
fn test_complex_mixed_extraction() {
    let args = vec![
        "--model".to_string(),
        "unsloth/Qwen3.5-0.8B".to_string(),
        "--quant".to_string(),
        "Q8_0".to_string(),
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "-t".to_string(),
        "8".to_string(),
    ];
    let result = extract_koji_flags(args).unwrap();
    assert_eq!(result.model, Some("unsloth/Qwen3.5-0.8B".to_string()));
    assert_eq!(result.quant, Some("Q8_0".to_string()));
    assert_eq!(result.port, None);
    assert_eq!(result.context_length, None);
    assert_eq!(
        result.remaining_args,
        vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "-t".to_string(),
            "8".to_string(),
        ]
    );
}

#[test]
fn test_quant_without_model() {
    let args = vec!["--quant".to_string(), "Q4_K_M".to_string()];
    let result = extract_koji_flags(args).unwrap();
    assert!(result.model.is_none());
    assert_eq!(result.quant, Some("Q4_K_M".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_port_invalid() {
    let args = vec!["--port".to_string(), "invalid".to_string()];
    let result = extract_koji_flags(args);
    assert!(result.is_err());
}

#[test]
fn test_extract_ctx_invalid() {
    let args = vec!["--ctx".to_string(), "invalid".to_string()];
    let result = extract_koji_flags(args);
    assert!(result.is_err());
}

// ── Async command-level tests ────────────────────────────────────────────

use koji::{cmd_server_add, cmd_server_edit};

/// cmd_server_add with a nonexistent model card ref should return an error,
/// not panic.
#[tokio::test]
async fn test_cmd_server_add_nonexistent_model_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = koji_core::config::Config::default();
    config.loaded_from = Some(temp_dir.path().to_path_buf());
    let result: anyhow::Result<()> = cmd_server_add(
        &config,
        "test_server",
        vec![
            "llama-server".to_string(),
            "--model".to_string(),
            "nonexistent/model".to_string(),
        ],
        false,
    )
    .await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found"),
        "Expected 'not found' in error, got: {}",
        err_msg
    );
}

/// cmd_server_edit on a server that doesn't exist should return an error.
#[tokio::test]
async fn test_cmd_server_edit_nonexistent_server_errors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = koji_core::config::Config::default();
    config.loaded_from = Some(temp_dir.path().to_path_buf());
    let result: anyhow::Result<()> = cmd_server_edit(
        &mut config,
        "nonexistent",
        vec![
            "llama-server".to_string(),
            "--profile".to_string(),
            "coding".to_string(),
        ],
    )
    .await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found"),
        "Expected 'not found' in error, got: {}",
        err_msg
    );
}

/// cmd_server_edit with a valid profile should succeed (not panic).
#[tokio::test]
async fn test_cmd_server_edit_valid_profile_succeeds() {
    let temp_dir = tempfile::tempdir().unwrap();

    // Use Config::load_from() which creates the default config file
    let mut config = koji_core::config::Config::load_from(temp_dir.path())
        .expect("Failed to load/create default config");
    // Insert a dummy server first
    config.models.insert(
        "test_server".to_string(),
        koji_core::config::ModelConfig {
            backend: "test".to_string(),
            args: vec![],
            profile: None,
            sampling: None,
            model: None,
            quant: None,

            mmproj: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            api_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
            modalities: None,
        },
    );
    // Need a matching backend
    config.backends.insert(
        "test".to_string(),
        koji_core::config::BackendConfig {
            path: Some("llama-server".to_string()),
            default_args: vec![],
            health_check_url: None,
            version: None,
        },
    );
    let result: anyhow::Result<()> = cmd_server_edit(
        &mut config,
        "test_server",
        vec![
            "llama-server".to_string(),
            "--profile".to_string(),
            // Profile::from_str is infallible (Custom variant), so this won't error.
            // But we can verify the edit actually applies the profile.
            "coding".to_string(),
        ],
    )
    .await;
    // This should succeed since "coding" is a valid profile
    assert!(result.is_ok(), "Expected ok, got: {:?}", result.err());
}
