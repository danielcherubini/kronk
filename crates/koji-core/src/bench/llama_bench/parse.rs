//! JSON parser for llama-bench `-o json` output.
//!
//! llama-bench emits one array entry per test configuration with `n_prompt`,
//! `n_gen`, `avg_ts`, and `stddev_ts`. We map each entry to a [`BenchSummary`]
//! with PP and TG populated based on whichever phase the entry measured.

use crate::bench::BenchSummary;
use anyhow::{Context, Result};

/// Parse llama-bench JSON output into a vector of [`BenchSummary`].
pub(super) fn parse_bench_json(output: &str) -> Result<Vec<BenchSummary>> {
    let entries: serde_json::Value =
        serde_json::from_str(output).context("Failed to parse llama-bench JSON output")?;

    let arr = entries
        .as_array()
        .context("llama-bench JSON output is not an array")?;

    let mut summaries = Vec::new();

    for entry in arr {
        let n_prompt = entry["n_prompt"].as_u64().unwrap_or(0) as u32;
        let n_gen = entry["n_gen"].as_u64().unwrap_or(0) as u32;
        let avg_ts = entry["avg_ts"].as_f64().unwrap_or(0.0);
        let stddev_ts = entry["stddev_ts"].as_f64().unwrap_or(0.0);

        let test_name = if n_prompt > 0 && n_gen == 0 {
            format!("pp{}", n_prompt)
        } else if n_prompt == 0 && n_gen > 0 {
            format!("tg{}", n_gen)
        } else if n_prompt > 0 && n_gen > 0 {
            format!("pg{}-{}", n_prompt, n_gen)
        } else {
            "unknown".to_string()
        };

        // llama-bench reports a single avg_ts per entry. For PP tests it's
        // prompt processing; for TG (or combined pg) it's generation speed.
        // TTFT and total latency are not measured by llama-bench.
        let (pp_mean, pp_stddev, tg_mean, tg_stddev) = if n_prompt > 0 && n_gen == 0 {
            (avg_ts, stddev_ts, 0.0, 0.0)
        } else if n_prompt == 0 && n_gen > 0 {
            (0.0, 0.0, avg_ts, stddev_ts)
        } else {
            // Combined test — avg_ts is for the generation phase.
            (0.0, 0.0, avg_ts, stddev_ts)
        };

        // Per-run config — surfaces the knob that actually varied in a sweep
        // (e.g. `-d` across three TG rows). Each field is optional because
        // llama-bench may omit them and legacy stored reports don't have them.
        let get_u32 = |key: &str| entry.get(key).and_then(|v| v.as_u64()).map(|v| v as u32);
        let get_str = |key: &str| {
            entry
                .get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        };

        summaries.push(BenchSummary {
            test_name,
            prompt_tokens: n_prompt,
            gen_tokens: n_gen,
            pp_mean,
            pp_stddev,
            tg_mean,
            tg_stddev,
            ttft_mean: 0.0,
            ttft_stddev: 0.0,
            total_mean: 0.0,
            total_stddev: 0.0,
            n_depth: get_u32("n_depth"),
            n_batch: get_u32("n_batch"),
            n_ubatch: get_u32("n_ubatch"),
            type_k: get_str("type_k"),
            type_v: get_str("type_v"),
            flash_attn: entry.get("flash_attn").and_then(|v| v.as_bool()),
            n_threads: get_u32("n_threads"),
            n_gpu_layers: entry
                .get("n_gpu_layers")
                .and_then(|v| v.as_i64())
                .map(|v| v as i32),
        });
    }

    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `parse_bench_json` correctly parses a single PP test entry.
    #[test]
    fn test_parse_bench_json_pp_test() {
        let json = r#"[{
            "n_prompt": 512,
            "n_gen": 0,
            "avg_ts": 5120.5,
            "stddev_ts": 42.3
        }]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.test_name, "pp512");
        assert_eq!(s.prompt_tokens, 512);
        assert_eq!(s.gen_tokens, 0);
        assert!((s.pp_mean - 5120.5).abs() < 0.01);
        assert!((s.pp_stddev - 42.3).abs() < 0.01);
        assert_eq!(s.tg_mean, 0.0);
        assert_eq!(s.ttft_mean, 0.0);
    }

    /// Verifies that `parse_bench_json` correctly parses a single TG test entry.
    #[test]
    fn test_parse_bench_json_tg_test() {
        let json = r#"[{
            "n_prompt": 0,
            "n_gen": 128,
            "avg_ts": 1000.0,
            "stddev_ts": 15.5
        }]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.test_name, "tg128");
        assert_eq!(s.prompt_tokens, 0);
        assert_eq!(s.gen_tokens, 128);
        assert_eq!(s.pp_mean, 0.0);
        assert!((s.tg_mean - 1000.0).abs() < 0.01);
    }

    /// Verifies that `parse_bench_json` correctly parses a combined PP+TG test entry.
    #[test]
    fn test_parse_bench_json_combined_test() {
        let json = r#"[{
            "n_prompt": 512,
            "n_gen": 128,
            "avg_ts": 950.0,
            "stddev_ts": 10.0
        }]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!(s.test_name, "pg512-128");
        assert_eq!(s.prompt_tokens, 512);
        assert_eq!(s.gen_tokens, 128);
    }

    /// Verifies that `parse_bench_json` returns an error for invalid JSON.
    #[test]
    fn test_parse_bench_json_invalid() {
        let result = parse_bench_json("not json");
        assert!(result.is_err());
    }

    /// Verifies that `parse_bench_json` returns an error for non-array JSON.
    #[test]
    fn test_parse_bench_json_not_array() {
        let result = parse_bench_json(r#"{"key": "value"}"#);
        assert!(result.is_err());
    }

    /// Verifies that `parse_bench_json` handles multiple entries correctly.
    #[test]
    fn test_parse_bench_json_multiple_entries() {
        let json = r#"[
            {"n_prompt": 512, "n_gen": 0, "avg_ts": 5000.0, "stddev_ts": 30.0},
            {"n_prompt": 0, "n_gen": 128, "avg_ts": 1000.0, "stddev_ts": 15.0},
            {"n_prompt": 512, "n_gen": 128, "avg_ts": 900.0, "stddev_ts": 20.0}
        ]"#;

        let summaries = parse_bench_json(json).unwrap();

        assert_eq!(summaries.len(), 3);
        assert_eq!(summaries[0].test_name, "pp512");
        assert_eq!(summaries[1].test_name, "tg128");
        assert_eq!(summaries[2].test_name, "pg512-128");
    }

    /// Verifies that `parse_bench_json` handles an empty JSON array.
    #[test]
    fn test_parse_bench_json_empty_array() {
        let json = "[]";
        let summaries = parse_bench_json(json).unwrap();
        assert!(summaries.is_empty());
    }
}
