//! Bench command handler
//!
//! Handles `koji bench` for benchmarking model inference performance.

use anyhow::{bail, Context, Result};
use koji_core::bench::{self, BenchConfig};
use koji_core::config::Config;
use koji_core::db::OpenResult;

/// Parse comma-separated sizes into a Vec<u32>
pub fn parse_comma_sizes(s: &str) -> Result<Vec<u32>> {
    let parts: Vec<&str> = s
        .split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();

    if parts.is_empty() {
        bail!("At least one size must be specified");
    }

    parts
        .iter()
        .map(|part| {
            part.parse::<u32>()
                .with_context(|| format!("Invalid size '{}': must be a positive integer", part))
        })
        .collect()
}

/// Benchmark command handler
#[allow(clippy::too_many_arguments)]
pub async fn cmd_bench(
    config: &Config,
    name: Option<String>,
    all: bool,
    pp: String,
    tg: String,
    runs: u32,
    warmup: u32,
    ctx: Option<u32>,
) -> Result<()> {
    let pp_sizes = parse_comma_sizes(&pp)?;
    let tg_sizes = parse_comma_sizes(&tg)?;

    let bench_config = BenchConfig {
        pp_sizes,
        tg_sizes,
        runs,
        warmup,
        ctx_override: ctx,
    };

    // Determine which servers to benchmark
    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult { conn, .. } = koji_core::db::open(&db_dir)?;
    let model_configs = koji_core::db::load_model_configs(&conn)?;

    let server_names: Vec<String> = if all {
        // Collect all server names from DB where enabled == true
        let mut servers: Vec<String> = model_configs
            .iter()
            .filter(|(_, server)| server.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        // Sort alphabetically for deterministic order
        servers.sort();

        if servers.is_empty() {
            bail!("No enabled model configs found. Create one with `koji model create`.");
        }

        servers
    } else if let Some(n) = name {
        // Validate the name exists
        config.resolve_server(&model_configs, &n)?;
        vec![n]
    } else {
        bail!("Specify a model config name or use --all to benchmark all enabled configs");
    };

    // Run benchmarks for each server
    for (idx, server_name) in server_names.iter().enumerate() {
        let report = bench::runner::run_benchmark(config, server_name, &bench_config).await?;
        bench::display::print_bench_report(&report);

        // Print blank line separator if there are more servers to benchmark
        if idx < server_names.len() - 1 {
            println!();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_comma_sizes_single() {
        let result = parse_comma_sizes("512");
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v, vec![512]);
    }

    #[test]
    fn test_parse_comma_sizes_multiple() {
        let result = parse_comma_sizes("128,256,512");
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v, vec![128, 256, 512]);
    }

    #[test]
    fn test_parse_comma_sizes_with_spaces() {
        let result = parse_comma_sizes("128, 256, 512");
        assert!(result.is_ok());
        let v = result.unwrap();
        assert_eq!(v, vec![128, 256, 512]);
    }

    #[test]
    fn test_parse_comma_sizes_invalid() {
        let result = parse_comma_sizes("abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_comma_sizes_empty() {
        let result = parse_comma_sizes("");
        assert!(result.is_err());
    }
}
