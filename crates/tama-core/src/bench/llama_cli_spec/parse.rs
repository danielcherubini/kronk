//! Output parsing for llama-cli benchmark runs.
//!
//! Extracts timing information from the stderr output produced by `llama-cli`
//! after a generation run completes.

use anyhow::{bail, Result};

/// Parse the token-generation speed (tokens/s) from llama-cli output.
///
/// llama-cli prints a line like:
/// ```text
///    total eval time = 2345.67 ms / 256 tokens ( 9.16 ms per token, 109.14 tokens per second)
/// ```
///
/// This function extracts the "tokens per second" value from that line.
///
/// # Arguments
/// - `output`: the full stderr (or stdout) output from llama-cli.
///
/// # Returns
/// The token generation speed as `f64` (tokens per second).
pub(super) fn parse_timing(output: &str) -> Result<f64> {
    let re = regex::Regex::new(
        r#"total eval time =\s*([\d.]+)\s*ms\s*/\s*(\d+)\s*tokens\s*\(\s*[\d.]+\s*ms per token,\s*([\d.]+)\s*tokens per second\)"#,
    )
    .expect("regex is valid");

    // Search for the timing substring within each line to handle prefixes like
    // "llama_print_timings:" and leading whitespace.
    for line in output.lines() {
        if let Some(pos) = line.find("total eval time =") {
            let trimmed = &line[pos..];
            if let Some(caps) = re.captures(trimmed) {
                // Group 3 is tokens/s (group 1 = eval time ms, group 2 = token count)
                let ts_str = caps.get(3).expect("capture group 3 exists").as_str();
                return ts_str.parse::<f64>().map_err(|e| {
                    anyhow::anyhow!("Failed to parse tokens/s value '{}': {}", ts_str, e)
                });
            }
        }
    }

    bail!(
        "No timing line found in output. Expected: 'total eval time = ...'. Raw output:\n{}",
        output
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `parse_timing` correctly extracts tokens/s from a normal llama-cli output line.
    #[test]
    fn test_parse_timing_normal() {
        let output = "llama_print_timings:        load time =   123.45 ms
llama_print_timings:      sample time =     5.67 ms / 256 runs (    0.02 ms per token, 45149.38 tokens per second)
llama_print_timings:        prompt eval time =   345.67 ms / 512 tokens (    0.68 ms per token, 1481.23 tokens per second)
llama_print_timings:           total eval time =  2345.67 ms / 256 tokens (    9.16 ms per token, 109.14 tokens per second)";

        let result = parse_timing(output).unwrap();
        assert!((result - 109.14).abs() < 0.01);
    }

    /// Verifies that `parse_timing` handles slightly different whitespace and values.
    #[test]
    fn test_parse_timing_varied_values() {
        let output = "some other output line
   total eval time = 1234.56 ms / 128 tokens ( 9.65 ms per token, 103.20 tokens per second)
more output";

        let result = parse_timing(output).unwrap();
        assert!((result - 103.20).abs() < 0.01);
    }

    /// Verifies that `parse_timing` returns an error with "No timing line" message when the output
    /// does not contain a matching line.
    #[test]
    fn test_parse_timing_malformed() {
        let output = "some random output
no timing info here
nothing useful";

        let result = parse_timing(output);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No timing line"));
    }

    /// Verifies that `parse_timing` returns an error for completely empty output.
    #[test]
    fn test_parse_timing_empty() {
        let result = parse_timing("");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("No timing line"));
    }

    /// Verifies that `parse_timing` handles a realistic multi-line output with the timing line
    /// buried among other output.
    #[test]
    fn test_parse_timing_buried_line() {
        let output = r#"llama.cpp built with support for:
- CUDA
Loading model...
Model loaded successfully.
Generating 256 tokens...
Done.
llama_print_timings:        load time =   500.00 ms
llama_print_timings:      sample time =    10.00 ms / 256 runs (    0.04 ms per token, 25600.00 tokens per second)
llama_print_timings:        prompt eval time =   800.00 ms / 512 tokens (    1.56 ms per token, 640.00 tokens per second)
llama_print_timings:           total eval time =  3200.00 ms / 256 tokens (   12.50 ms per token,  80.00 tokens per second)"#;

        let result = parse_timing(output).unwrap();
        assert!((result - 80.00).abs() < 0.01);
    }
}
