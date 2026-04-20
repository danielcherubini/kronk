//! CLI-argument construction for the llama-bench binary.
//!
//! Isolated from the orchestrator so the flag-encoding rules (empty-vec
//! omission, matched KV-pair emission, sweep-friendly comma lists) can be
//! exercised as pure unit tests without needing a live llama-bench process.

use super::LlamaBenchConfig;

/// Build the command-line arguments for llama-bench based on config.
pub(super) fn build_args(model_path: &std::path::Path, config: &LlamaBenchConfig) -> Vec<String> {
    let mut args = Vec::new();

    args.push("--model".to_string());
    args.push(model_path.to_string_lossy().to_string());

    // Prompt sizes: omit when empty (so the user can pin `-p 0` for pure-TG
    // runs without `-p ""` leaking through). Zero is a meaningful value —
    // `-p 0` skips the PP phase — so it has to survive the parser too.
    if !config.pp_sizes.is_empty() {
        args.push("-p".to_string());
        args.push(join_u32(&config.pp_sizes));
    }

    // Generation lengths: same empty/zero rules as pp_sizes above.
    if !config.tg_sizes.is_empty() {
        args.push("-n".to_string());
        args.push(join_u32(&config.tg_sizes));
    }

    args.push("-r".to_string());
    args.push(config.runs.to_string());

    if let Some(threads) = &config.threads {
        args.push("--threads".to_string());
        args.push(join_u32(threads));
    }

    if let Some(ref ngl_range) = config.ngl_range {
        args.push("--n-gpu-layers".to_string());
        args.push(ngl_range.clone());
    }

    if let Some(ctx) = config.ctx_override {
        args.push("--fit-ctx".to_string());
        args.push(ctx.to_string());
    }

    if !config.batch_sizes.is_empty() {
        args.push("-b".to_string());
        args.push(join_u32(&config.batch_sizes));
    }

    if !config.ubatch_sizes.is_empty() {
        args.push("-ub".to_string());
        args.push(join_u32(&config.ubatch_sizes));
    }

    // KV cache type — apply same value to both -ctk and -ctv. Mismatched K/V
    // quant triggers a CPU-attention fallback on most builds (~40% TG drop),
    // so the UI intentionally exposes only a single matched field.
    if let Some(ref kv) = config.kv_cache_type {
        args.push("-ctk".to_string());
        args.push(kv.clone());
        args.push("-ctv".to_string());
        args.push(kv.clone());
    }

    if !config.depth.is_empty() {
        args.push("-d".to_string());
        args.push(join_u32(&config.depth));
    }

    if let Some(fa) = config.flash_attn {
        args.push("-fa".to_string());
        args.push(if fa { "1".to_string() } else { "0".to_string() });
    }

    args.push("-o".to_string());
    args.push("json".to_string());

    args
}

fn join_u32(xs: &[u32]) -> String {
    xs.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `build_args` produces correct arguments for a single PP and TG size.
    #[test]
    fn test_build_args_single_sizes() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"/test/model.gguf".to_string()));
        assert!(args.iter().any(|a| a == "-p"));
        assert!(args.iter().any(|a| a == "512"));
        assert!(args.iter().any(|a| a == "-n"));
        assert!(args.iter().any(|a| a == "128"));
        assert!(args.contains(&"-r".to_string()));
        assert!(args.contains(&"3".to_string()));
        assert!(args.contains(&"-o".to_string()));
        assert!(args.contains(&"json".to_string()));
    }

    /// Verifies that `build_args` produces comma-separated values for multiple sizes.
    #[test]
    fn test_build_args_multiple_sizes() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![256, 512],
            tg_sizes: vec![64, 128, 256],
            runs: 5,
            warmup: 2,
            threads: Some(vec![4, 8]),
            ngl_range: Some("0-99+1".to_string()),
            ctx_override: Some(4096),
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"256,512".to_string()));
        assert!(args.contains(&"-n".to_string()));
        assert!(args.contains(&"64,128,256".to_string()));
        assert!(args.contains(&"--threads".to_string()));
        assert!(args.contains(&"4,8".to_string()));
        assert!(args.contains(&"--n-gpu-layers".to_string()));
        assert!(args.contains(&"0-99+1".to_string()));
        assert!(args.contains(&"--fit-ctx".to_string()));
        assert!(args.contains(&"4096".to_string()));
    }

    /// Verifies that `build_args` produces comma-separated values for multiple sizes.
    #[test]
    fn test_build_args_multi_values() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![128, 256, 512],
            tg_sizes: vec![32, 64],
            runs: 5,
            warmup: 2,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"-p".to_string()));
        let pp_idx = args.iter().position(|a| a == "-p").unwrap();
        assert_eq!(args[pp_idx + 1], "128,256,512");
        let tg_idx = args.iter().position(|a| a == "-n").unwrap();
        assert_eq!(args[tg_idx + 1], "32,64");
    }

    /// Verifies that `build_args` handles a single thread count correctly.
    #[test]
    fn test_build_args_with_single_thread() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: Some(vec![4]),
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let args = build_args(&model_path, &config);

        assert!(args.contains(&"--threads".to_string()));
        let threads_idx = args.iter().position(|a| a == "--threads").unwrap();
        assert_eq!(args[threads_idx + 1], "4");
    }

    /// Verifies that empty `pp_sizes` omits `-p` entirely (rather than emitting
    /// `-p ""`), and that `-p 0` survives when the user explicitly asks for
    /// pure-TG mode.
    #[test]
    fn test_build_args_empty_and_zero_sizes() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");

        let config = LlamaBenchConfig {
            pp_sizes: vec![],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };
        let args = build_args(&model_path, &config);
        assert!(!args.iter().any(|a| a == "-p"));
        assert!(args.iter().any(|a| a == "-n"));

        let config = LlamaBenchConfig {
            pp_sizes: vec![0],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };
        let args = build_args(&model_path, &config);
        let p_idx = args.iter().position(|a| a == "-p").expect("-p missing");
        assert_eq!(args[p_idx + 1], "0");
    }

    /// Verifies that methodology flags (`-b`, `-ub`, `-ctk`/`-ctv`, `-d`, `-fa`)
    /// are emitted with matched KV cache types and sweep-friendly comma lists.
    #[test]
    fn test_build_args_methodology_flags() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![1024, 2048],
            ubatch_sizes: vec![512],
            kv_cache_type: Some("q8_0".to_string()),
            depth: vec![0, 4096, 16384],
            flash_attn: Some(true),
        };

        let args = build_args(&model_path, &config);

        let find_val = |flag: &str| -> String {
            let idx = args.iter().position(|a| a == flag).expect("flag not found");
            args[idx + 1].clone()
        };

        assert_eq!(find_val("-b"), "1024,2048");
        assert_eq!(find_val("-ub"), "512");
        // K and V must be matched to avoid CPU-attention fallback.
        assert_eq!(find_val("-ctk"), "q8_0");
        assert_eq!(find_val("-ctv"), "q8_0");
        assert_eq!(find_val("-d"), "0,4096,16384");
        assert_eq!(find_val("-fa"), "1");
    }

    /// Verifies that methodology flags are omitted when unset.
    #[test]
    fn test_build_args_methodology_omitted_when_empty() {
        let model_path = std::path::PathBuf::from("/test/model.gguf");
        let config = LlamaBenchConfig {
            pp_sizes: vec![512],
            tg_sizes: vec![128],
            runs: 3,
            warmup: 1,
            threads: None,
            ngl_range: None,
            ctx_override: None,
            batch_sizes: vec![],
            ubatch_sizes: vec![],
            kv_cache_type: None,
            depth: vec![],
            flash_attn: None,
        };

        let args = build_args(&model_path, &config);
        assert!(!args.iter().any(|a| a == "-b"));
        assert!(!args.iter().any(|a| a == "-ub"));
        assert!(!args.iter().any(|a| a == "-ctk"));
        assert!(!args.iter().any(|a| a == "-ctv"));
        assert!(!args.iter().any(|a| a == "-d"));
        assert!(!args.iter().any(|a| a == "-fa"));
    }
}
