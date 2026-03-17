/// Tests for inject_context_size helper function
use super::args::inject_context_size;

#[test]
fn injects_context_size_when_no_existing_args() {
    let mut args = vec![];
    inject_context_size(&mut args, 8192);

    assert_eq!(args, vec!["-c", "8192"]);
}

#[test]
fn replaces_existing_c_flag() {
    let mut args = vec!["-c".to_string(), "4096".to_string()];
    inject_context_size(&mut args, 8192);

    assert_eq!(args, vec!["-c", "8192"]);
}

#[test]
fn replaces_existing_ctx_size_flag() {
    let mut args = vec!["--ctx-size".to_string(), "4096".to_string()];
    inject_context_size(&mut args, 8192);

    assert_eq!(args, vec!["-c", "8192"]);
}

#[test]
fn preserves_other_args_and_replaces_c() {
    let mut args = vec![
        "--model".to_string(),
        "llama3".to_string(),
        "-c".to_string(),
        "4096".to_string(),
        "--temp".to_string(),
        "0.7".to_string(),
    ];
    inject_context_size(&mut args, 8192);

    assert_eq!(
        args,
        vec![
            "--model".to_string(),
            "llama3".to_string(),
            "-c".to_string(),
            "8192".to_string(),
            "--temp".to_string(),
            "0.7".to_string(),
        ]
    );
}

#[test]
fn handles_multiple_c_flags() {
    let mut args = vec![
        "-c".to_string(),
        "4096".to_string(),
        "-c".to_string(),
        "8192".to_string(),
    ];
    inject_context_size(&mut args, 16384);

    assert_eq!(args, vec!["-c", "16384"]);
}

#[test]
fn handles_mixed_flag_formats() {
    let mut args = vec![
        "--model".to_string(),
        "llama3".to_string(),
        "-c".to_string(),
        "4096".to_string(),
        "--ctx-size".to_string(),
        "8192".to_string(),
        "--temp".to_string(),
        "0.7".to_string(),
    ];
    inject_context_size(&mut args, 16384);

    assert_eq!(
        args,
        vec![
            "--model".to_string(),
            "llama3".to_string(),
            "-c".to_string(),
            "16384".to_string(),
            "--temp".to_string(),
            "0.7".to_string(),
        ]
    );
}
