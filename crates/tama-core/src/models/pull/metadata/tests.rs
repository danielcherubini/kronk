use super::*;

/// Test MoE model parsing (Qwen3.6-35B-A3B style)
#[test]
fn test_parse_readme_moe_model() {
    let readme = r#"
---
tags:
  - qwen3.6
  - generative-models
pipeline_tag: text-generation
---

# Qwen3.6-35B-A3B

## Model Info

| Parameter | Value |
|-----------|-------|
| Number of Parameters | 35B |
| Active Parameters | 3.8B |
| Context Length | 262,144 |
| Number of Layers | 40 |
"#;
    let meta = parse_readme_metadata(readme);

    assert_eq!(meta.hf_total_params, Some("35B".to_string()));
    assert_eq!(meta.hf_active_params, Some("3.8B".to_string()));
    assert_eq!(meta.hf_architecture_type, Some("MoE".to_string()));
    assert_eq!(meta.hf_context_length, Some(262144));
    assert_eq!(meta.hf_num_layers, Some(40));
}

/// Test Dense model parsing (Gemma 4 26B A4B style)
#[test]
fn test_parse_readme_dense_model() {
    let readme = r#"
---
tags:
  - gemma-4
pipeline_tag: text-generation
---

# Gemma 4 26B A4B

## Model Description

This is a dense model with 26 billion parameters.

Number of Parameters: 26B
Context Length: 8192
Number of Layers: 46
"#;
    let meta = parse_readme_metadata(readme);

    assert_eq!(meta.hf_total_params, Some("26B".to_string()));
    assert_eq!(meta.hf_active_params, None);
    assert_eq!(meta.hf_architecture_type, Some("Dense".to_string()));
    assert_eq!(meta.hf_context_length, Some(8192));
    assert_eq!(meta.hf_num_layers, Some(46));
}

/// Test Mamba model detection (Nemotron 3 Nano style)
#[test]
fn test_parse_readme_mamba_model() {
    let readme = r#"
---
tags:
  - nemotron
pipeline_tag: text-generation
---

# Nemotron 3 Nano

This model uses Mamba2 architecture for efficient sequence modeling.

Number of Parameters: 1.2B
Context Length: 256K tokens
Layers | 30
"#;
    let meta = parse_readme_metadata(readme);

    assert_eq!(meta.hf_total_params, Some("1.2B".to_string()));
    assert_eq!(meta.hf_active_params, None);
    assert_eq!(
        meta.hf_architecture_type,
        Some("Mamba2-Transformer MoE".to_string())
    );
    assert_eq!(meta.hf_context_length, Some(262144)); // 256 * 1024
    assert_eq!(meta.hf_num_layers, Some(30));
}

/// Test "K tokens" context length parsing
#[test]
fn test_parse_readme_context_k_tokens() {
    let readme = r#"
Context Length: 128K tokens
"#;
    let meta = parse_readme_metadata(readme);
    assert_eq!(meta.hf_context_length, Some(131072)); // 128 * 1024
}

/// Test comma-separated context length
#[test]
fn test_parse_readme_context_comma() {
    let readme = r#"
Context Length: 262,144
"#;
    let meta = parse_readme_metadata(readme);
    assert_eq!(meta.hf_context_length, Some(262144));
}

/// Test table-style parsing for all fields
#[test]
fn test_parse_readme_table_style() {
    let readme = r#"
| Parameter | Value |
|-----------|-------|
| Total Parameters | 25.2B |
| Active Parameters | 3B |
| Context Length | 131072 |
| Layers | 30 |
"#;
    let meta = parse_readme_metadata(readme);

    assert_eq!(meta.hf_total_params, Some("25.2B".to_string()));
    assert_eq!(meta.hf_active_params, Some("3B".to_string()));
    assert_eq!(meta.hf_architecture_type, Some("MoE".to_string()));
    assert_eq!(meta.hf_context_length, Some(131072));
    assert_eq!(meta.hf_num_layers, Some(30));
}

/// Test empty/unknown README returns defaults
#[test]
fn test_parse_readme_empty() {
    let meta = parse_readme_metadata("");
    assert_eq!(meta.hf_total_params, None);
    assert_eq!(meta.hf_active_params, None);
    assert_eq!(meta.hf_architecture_type, None); // empty README can't infer architecture
    assert_eq!(meta.hf_context_length, None);
    assert_eq!(meta.hf_num_layers, None);
}

/// Test "activated" shorthand pattern for active params
#[test]
fn test_parse_readme_activated_pattern() {
    let readme = r#"
Number of Parameters: 35B
3B activated
Context Length: 4096
"#;
    let meta = parse_readme_metadata(readme);
    assert_eq!(meta.hf_total_params, Some("35B".to_string()));
    assert_eq!(meta.hf_active_params, Some("3B".to_string()));
    assert_eq!(meta.hf_architecture_type, Some("MoE".to_string()));
}

/// Test M suffix for context length
#[test]
fn test_parse_readme_context_m_suffix() {
    let readme = r#"
Context Length: 2M
"#;
    let meta = parse_readme_metadata(readme);
    assert_eq!(meta.hf_context_length, Some(2097152)); // 2 * 1024 * 1024
}
