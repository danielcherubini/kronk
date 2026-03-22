/// Tests for extract_kronk_flags helper function
use kronk::extract_kronk_flags;

#[test]
fn test_extract_model_huggingface_ref() {
    let args = vec!["--model".to_string(), "unsloth/Qwen3.5-0.8B".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert_eq!(result.model, Some("unsloth/Qwen3.5-0.8B".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_model_local_path() {
    let args = vec!["--model".to_string(), "/path/to/file.gguf".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert!(result.model.is_none());
    assert_eq!(
        result.remaining_args,
        vec!["--model".to_string(), "/path/to/file.gguf".to_string()]
    );
}

#[test]
fn test_extract_model_short_flag() {
    let args = vec!["-m".to_string(), "/path/to/file.gguf".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert!(result.model.is_none());
    assert_eq!(
        result.remaining_args,
        vec!["-m".to_string(), "/path/to/file.gguf".to_string()]
    );
}

#[test]
fn test_extract_profile() {
    let args = vec!["--profile".to_string(), "chat".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert_eq!(result.profile, Some("chat".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_quant() {
    let args = vec!["--quant".to_string(), "Q4_K_M".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert_eq!(result.quant, Some("Q4_K_M".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_port() {
    let args = vec!["--port".to_string(), "8081".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert_eq!(result.port, Some(8081));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_context_length() {
    let args = vec!["--ctx".to_string(), "8192".to_string()];
    let result = extract_kronk_flags(args).unwrap();
    assert_eq!(result.context_length, Some(8192));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_mixed_kronk_and_backend_flags() {
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
    let result = extract_kronk_flags(args).unwrap();
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
fn test_no_kronk_flags_all_remain() {
    let args = vec![
        "--host".to_string(),
        "0.0.0.0".to_string(),
        "-t".to_string(),
        "8".to_string(),
    ];
    let result = extract_kronk_flags(args).unwrap();
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
    let result = extract_kronk_flags(args);
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
    let result = extract_kronk_flags(args).unwrap();
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
    let result = extract_kronk_flags(args).unwrap();
    assert!(result.model.is_none());
    assert_eq!(result.quant, Some("Q4_K_M".to_string()));
    assert!(result.remaining_args.is_empty());
}

#[test]
fn test_extract_port_invalid() {
    let args = vec!["--port".to_string(), "invalid".to_string()];
    let result = extract_kronk_flags(args);
    assert!(result.is_err());
}

#[test]
fn test_extract_ctx_invalid() {
    let args = vec!["--ctx".to_string(), "invalid".to_string()];
    let result = extract_kronk_flags(args);
    assert!(result.is_err());
}
