//! CLI-argument construction for the llama-cli binary with speculative decoding.
//!
//! Isolated from the orchestrator so the flag-encoding rules can be exercised
//! as pure unit tests without needing a live llama-cli process.

use super::{SpecBenchConfig, SpecType};

/// Build the command-line arguments for a single llama-cli speculative decoding run.
///
/// Each spec-type only gets the knobs it uses:
/// - **ngram-simple**: `--spec-type ngram-simple --draft-max N`
/// - **ngram-mod**: `--spec-type ngram-mod --spec-ngram-size-n N --draft-min M --draft-max MAX`
///   (where M = draft_max / 2, clamped to ≥ 1)
/// - **ngram-map-k**: `--spec-type ngram-map-k --spec-ngram-size-n N --spec-ngram-size-m M --draft-max MAX`
/// - **ngram-map-k4v**: same as ngram-map-k but with `--spec-type ngram-map-k4v`
///
/// Always included: `-m <model_path>`, `-n <gen_tokens>`, `-ngl <ngl>` (if Some), `-fa 1|0`
///
/// The prompt is generated via `crate::bench::build_prompt(512)`.
pub(super) fn build_args(
    config: &SpecBenchConfig,
    spec_type: SpecType,
    draft_max: u32,
    ngram_n: Option<u32>,
    ngram_m: Option<u32>,
) -> Vec<String> {
    let mut args = Vec::new();

    // Model path
    args.push("-m".to_string());
    args.push(config.model_path.to_string_lossy().to_string());

    // Prompt (512 tokens)
    let prompt = crate::bench::build_prompt(512);
    args.push("-p".to_string());
    args.push(prompt);

    // Generation length
    args.push("-n".to_string());
    args.push(config.gen_tokens.to_string());

    // GPU layers (if specified)
    if let Some(ngl) = config.ngl {
        args.push("--n-gpu-layers".to_string());
        args.push(ngl.to_string());
    }

    // Flash attention
    args.push("-fa".to_string());
    args.push(if config.flash_attn { "1" } else { "0" }.to_string());

    // Suppress conversation mode and prompt echoing
    args.push("-no-cnv".to_string());
    args.push("-sp".to_string());
    args.push("0".to_string());

    // Spec-decoding flags — only for the knobs each type uses
    match spec_type {
        SpecType::NgramSimple => {
            args.push("--spec-type".to_string());
            args.push("ngram-simple".to_string());
            args.push("--draft-max".to_string());
            args.push(draft_max.to_string());
        }
        SpecType::NgramMod => {
            let n = ngram_n.expect("ngram_n required for ngram-mod");
            let draft_min = (draft_max / 2).max(1);

            args.push("--spec-type".to_string());
            args.push("ngram-mod".to_string());
            args.push("--spec-ngram-size-n".to_string());
            args.push(n.to_string());
            args.push("--draft-min".to_string());
            args.push(draft_min.to_string());
            args.push("--draft-max".to_string());
            args.push(draft_max.to_string());
        }
        SpecType::NgramMapK | SpecType::NgramMapK4v => {
            let n = ngram_n.expect("ngram_n required for ngram-map-*");
            let m = ngram_m.expect("ngram_m required for ngram-map-*");

            args.push("--spec-type".to_string());
            args.push(spec_type.as_str().to_string());
            args.push("--spec-ngram-size-n".to_string());
            args.push(n.to_string());
            args.push("--spec-ngram-size-m".to_string());
            args.push(m.to_string());
            args.push("--draft-max".to_string());
            args.push(draft_max.to_string());
        }
    }

    args
}

/// Build command-line arguments for a baseline run (no speculative decoding).
///
/// Omits all `--spec-*` and `--draft-*` flags entirely. Still includes model,
/// prompt, generation length, GPU layers, and flash attention settings.
pub(super) fn build_baseline_args(config: &SpecBenchConfig) -> Vec<String> {
    let mut args = Vec::new();

    // Model path
    args.push("-m".to_string());
    args.push(config.model_path.to_string_lossy().to_string());

    // Prompt (512 tokens)
    let prompt = crate::bench::build_prompt(512);
    args.push("-p".to_string());
    args.push(prompt);

    // Generation length
    args.push("-n".to_string());
    args.push(config.gen_tokens.to_string());

    // GPU layers (if specified)
    if let Some(ngl) = config.ngl {
        args.push("--n-gpu-layers".to_string());
        args.push(ngl.to_string());
    }

    // Flash attention
    args.push("-fa".to_string());
    args.push(if config.flash_attn { "1" } else { "0" }.to_string());

    // Suppress conversation mode and prompt echoing
    args.push("-no-cnv".to_string());
    args.push("-sp".to_string());
    args.push("0".to_string());

    args
}

/// Find the value following a flag in an argument list.
#[cfg(test)]
fn find_arg<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .map(|idx| args.get(idx + 1).map(|s| s.as_str()).unwrap_or(""))
}

/// Check if a flag is present in an argument list.
#[cfg(test)]
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> SpecBenchConfig {
        SpecBenchConfig {
            model_path: std::path::PathBuf::from("/test/model.gguf"),
            spec_types: vec![],
            draft_max_values: vec![],
            ngram_n_values: vec![],
            ngram_m_values: vec![],
            ngram_min_hits: 1,
            gen_tokens: 256,
            runs: 3,
            ngl: None,
            flash_attn: true,
        }
    }

    /// Verifies that `build_args` for ngram-simple emits only the correct flags.
    #[test]
    fn test_build_args_ngram_simple() {
        let config = make_config();
        let args = build_args(&config, SpecType::NgramSimple, 32, None, None);

        assert!(has_flag(&args, "-m"));
        assert_eq!(find_arg(&args, "-m"), Some("/test/model.gguf"));
        assert!(has_flag(&args, "-n"));
        assert_eq!(find_arg(&args, "-n"), Some("256"));
        assert!(has_flag(&args, "-fa"));
        assert_eq!(find_arg(&args, "-fa"), Some("1"));
        assert!(has_flag(&args, "--spec-type"));
        assert_eq!(find_arg(&args, "--spec-type"), Some("ngram-simple"));
        assert!(has_flag(&args, "--draft-max"));
        assert_eq!(find_arg(&args, "--draft-max"), Some("32"));

        // ngram-simple should NOT have ngram flags
        assert!(!has_flag(&args, "--spec-ngram-size-n"));
        assert!(!has_flag(&args, "--spec-ngram-size-m"));
        assert!(!has_flag(&args, "--draft-min"));
    }

    /// Verifies that `build_args` for ngram-mod emits correct flags with computed draft-min.
    #[test]
    fn test_build_args_ngram_mod() {
        let config = make_config();
        let args = build_args(&config, SpecType::NgramMod, 32, Some(5), None);

        assert!(has_flag(&args, "--spec-type"));
        assert_eq!(find_arg(&args, "--spec-type"), Some("ngram-mod"));
        assert!(has_flag(&args, "--spec-ngram-size-n"));
        assert_eq!(find_arg(&args, "--spec-ngram-size-n"), Some("5"));
        assert!(has_flag(&args, "--draft-min"));
        // draft_min = 32 / 2 = 16
        assert_eq!(find_arg(&args, "--draft-min"), Some("16"));
        assert!(has_flag(&args, "--draft-max"));
        assert_eq!(find_arg(&args, "--draft-max"), Some("32"));

        // ngram-mod should NOT have size-m flag
        assert!(!has_flag(&args, "--spec-ngram-size-m"));
    }

    /// Verifies that `build_args` for ngram-mod clamps draft-min to 1 when draft_max is 1.
    #[test]
    fn test_build_args_ngram_mod_draft_min_clamped() {
        let config = make_config();
        let args = build_args(&config, SpecType::NgramMod, 1, Some(3), None);

        assert_eq!(find_arg(&args, "--draft-min"), Some("1")); // (1 / 2).max(1) = 1
    }

    /// Verifies that `build_args` for ngram-map-k emits correct flags.
    #[test]
    fn test_build_args_ngram_map_k() {
        let config = make_config();
        let args = build_args(&config, SpecType::NgramMapK, 64, Some(7), Some(3));

        assert!(has_flag(&args, "--spec-type"));
        assert_eq!(find_arg(&args, "--spec-type"), Some("ngram-map-k"));
        assert!(has_flag(&args, "--spec-ngram-size-n"));
        assert_eq!(find_arg(&args, "--spec-ngram-size-n"), Some("7"));
        assert!(has_flag(&args, "--spec-ngram-size-m"));
        assert_eq!(find_arg(&args, "--spec-ngram-size-m"), Some("3"));
        assert!(has_flag(&args, "--draft-max"));
        assert_eq!(find_arg(&args, "--draft-max"), Some("64"));
    }

    /// Verifies that `build_args` for ngram-map-k4v emits correct flags.
    #[test]
    fn test_build_args_ngram_map_k4v() {
        let config = make_config();
        let args = build_args(&config, SpecType::NgramMapK4v, 64, Some(7), Some(3));

        assert!(has_flag(&args, "--spec-type"));
        assert_eq!(find_arg(&args, "--spec-type"), Some("ngram-map-k4v"));
        assert!(has_flag(&args, "--spec-ngram-size-n"));
        assert_eq!(find_arg(&args, "--spec-ngram-size-n"), Some("7"));
        assert!(has_flag(&args, "--spec-ngram-size-m"));
        assert_eq!(find_arg(&args, "--spec-ngram-size-m"), Some("3"));
        assert!(has_flag(&args, "--draft-max"));
        assert_eq!(find_arg(&args, "--draft-max"), Some("64"));
    }

    /// Verifies that `build_baseline_args` returns args WITHOUT any spec or draft flags.
    #[test]
    fn test_build_baseline_args_no_spec_flags() {
        let config = make_config();
        let args = build_baseline_args(&config);

        assert!(has_flag(&args, "-m"));
        assert_eq!(find_arg(&args, "-m"), Some("/test/model.gguf"));
        assert!(has_flag(&args, "-n"));
        assert_eq!(find_arg(&args, "-n"), Some("256"));
        assert!(has_flag(&args, "-fa"));
        assert_eq!(find_arg(&args, "-fa"), Some("1"));

        // Must NOT contain any spec or draft flags
        assert!(!has_flag(&args, "--spec-type"));
        assert!(!has_flag(&args, "--draft-max"));
        assert!(!has_flag(&args, "--draft-min"));
        assert!(!has_flag(&args, "--spec-ngram-size-n"));
        assert!(!has_flag(&args, "--spec-ngram-size-m"));
    }

    /// Verifies that `build_baseline_args` includes GPU layers when configured.
    #[test]
    fn test_build_baseline_args_with_ngl() {
        let mut config = make_config();
        config.ngl = Some(99);
        let args = build_baseline_args(&config);

        assert!(has_flag(&args, "--n-gpu-layers"));
        assert_eq!(find_arg(&args, "--n-gpu-layers"), Some("99"));
    }

    /// Verifies that `build_args` includes GPU layers when configured.
    #[test]
    fn test_build_args_with_ngl() {
        let mut config = make_config();
        config.ngl = Some(45);
        let args = build_args(&config, SpecType::NgramSimple, 16, None, None);

        assert!(has_flag(&args, "--n-gpu-layers"));
        assert_eq!(find_arg(&args, "--n-gpu-layers"), Some("45"));
    }

    /// Verifies that flash_attn=false emits `-fa 0`.
    #[test]
    fn test_build_args_flash_attn_off() {
        let mut config = make_config();
        config.flash_attn = false;
        let args = build_args(&config, SpecType::NgramSimple, 16, None, None);

        assert_eq!(find_arg(&args, "-fa"), Some("0"));
    }

    /// Verifies that `build_args` includes the prompt argument.
    #[test]
    fn test_build_args_includes_prompt() {
        let config = make_config();
        let args = build_args(&config, SpecType::NgramSimple, 16, None, None);

        assert!(has_flag(&args, "-p"));
        let prompt = find_arg(&args, "-p").expect("prompt should be present");
        // Prompt should be approximately 512 * 4 = 2048 chars
        assert!(prompt.len() >= 1500 && prompt.len() <= 2500);
    }
}
