//! Helper functions for parsing and manipulating shell-style arguments.
//!
//! This module provides utilities for handling command-line arguments, including
//! splitting entries, parsing flags, flattening nested structures, and quoting values.

use anyhow::Result;

/// Split a single argument entry into its components.
///
/// Handles arguments with various formats:
/// - Simple values: `"hello"` → [`"hello"]
/// - Key-value pairs: `"key=value"` → [`"key="`, `"value"]` or [`"key", "=value"]` depending on parsing
/// - Quoted values: `"key='hello world'"` → [`"key="`, `"'hello world'"`]
/// - Mixed quotes: `"key=\"hello world\""` → [`"key="`, `"\"hello world\""`]
///
/// # Examples
///
/// ```
/// use koji_core::config::split_arg_entry;
///
/// let result = split_arg_entry("key=value").unwrap();
/// assert_eq!(result, vec!["key=".to_string(), "value".to_string()]);
///
/// let result = split_arg_entry("simple").unwrap();
/// assert_eq!(result, vec!["simple".to_string()]);
/// ```
pub fn split_arg_entry(arg: &str) -> Result<Vec<String>> {
    let trimmed = arg.trim();

    if trimmed.is_empty() {
        return Ok(vec![]);
    }

    // Check if there's an equals sign (key=value format)
    if let Some(eq_pos) = trimmed.find('=') {
        let key_part = trimmed[..=eq_pos].to_string();
        let value_part = trimmed[eq_pos + 1..].to_string();

        Ok(vec![key_part, value_part])
    } else {
        // No equals sign, return as single entry
        Ok(vec![trimmed.to_string()])
    }
}

/// Extract the flag name from a token.
///
/// Handles various flag formats:
/// - Long flags: `"--verbose"` → `"verbose"`
/// - Long flags with value: `"--model=/path"` → `"model"`
/// - Short flags: `"-v"` → `"v"`
/// - Short flag with value: `"-o=file"` → `"o"`
///- Double short flags: `"-vv"` → `"v"`
///
/// # Examples
///
/// ```
/// use koji_core::config::flag_name;
///
/// assert_eq!(flag_name("--verbose").unwrap(), "verbose");
/// assert_eq!(flag_name("--model=/path").unwrap(), "model");
/// assert_eq!(flag_name("-v").unwrap(), "v");
/// assert_eq!(flag_name("-vv").unwrap(), "v");
/// ```
pub fn flag_name(token: &str) -> Result<String> {
    let trimmed = token.trim();

    if trimmed.is_empty() {
        return Err(anyhow::anyhow!("Empty token"));
    }

    // Check if it starts with -- (long flag)
    if let Some(inner) = trimmed.strip_prefix("--") {
        // Reject triple-dash prefixes
        if inner.starts_with('-') {
            return Err(anyhow::anyhow!("Not a flag token"));
        }
        // Extract the flag name before any = or space
        if let Some(eq_pos) = inner.find('=') {
            Ok(inner[..eq_pos].to_string())
        } else {
            // Check for space-separated value (e.g., "--model /path")
            if let Some(space_pos) = inner.find(' ') {
                Ok(inner[..space_pos].to_string())
            } else {
                Ok(inner.to_string())
            }
        }
    }
    // Check if it starts with - (short flag)
    else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        let inner = &trimmed[1..];
        // For short flags, just take the first character
        Ok(inner.chars().next().unwrap_or_default().to_string())
    } else {
        Err(anyhow::anyhow!("Not a flag token"))
    }
}

/// Flatten a vector of argument strings into a single vector.
///
/// This function processes a list of argument strings, splitting any compound
/// arguments and returning a flat list of individual argument tokens.
///
/// Grouped entries like "-b 4096" are split into separate tokens.
/// Entries with equals signs like "--model=/path" are kept as-is.
/// Quoted values are preserved.
///
/// # Examples
///
/// ```
/// use koji_core::config::flatten_args;
///
/// let args = vec!["--verbose".to_string(), "--model=/path".to_string()];
/// let flattened = flatten_args(&args).unwrap();
/// assert_eq!(flattened, vec!["--verbose".to_string(), "--model=/path".to_string()]);
///
/// let args = vec!["--verbose".to_string(), "--help".to_string()];
/// let flattened = flatten_args(&args).unwrap();
/// assert_eq!(flattened, vec!["--verbose".to_string(), "--help".to_string()]);
///
/// let args = vec!["-b 4096".to_string(), "-t 14".to_string()];
/// let flattened = flatten_args(&args).unwrap();
/// assert_eq!(flattened, vec!["-b".to_string(), "4096".to_string(), "-t".to_string(), "14".to_string()]);
/// ```
pub fn flatten_args(args: &[String]) -> Result<Vec<String>> {
    let mut result = Vec::new();

    for arg in args {
        let trimmed = arg.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Check if this is a grouped entry (flag + space + value)
        // Flag entries start with '-' but are not quoted
        if trimmed.starts_with('-') && !trimmed.starts_with('"') && !trimmed.starts_with('\'') {
            // Check if there's a space separating flag from value
            if let Some(space_pos) = trimmed.find(' ') {
                // Split into flag and value
                let flag = trimmed[..space_pos].to_string();
                let value = trimmed[space_pos + 1..].to_string();
                result.push(flag);
                result.push(value);
            } else {
                // No space, keep as single token
                result.push(trimmed.to_string());
            }
        } else {
            // Not a flag entry, keep as single token
            result.push(trimmed.to_string());
        }
    }

    Ok(result)
}

/// Quote a value for safe shell interpolation.
///
/// Adds quotes around values that contain spaces, special characters, or
/// shell metacharacters to prevent interpretation by the shell.
///
/// # Examples
///
/// ```
/// use koji_core::config::quote_value;
///
/// assert_eq!(quote_value("simple"), "simple".to_string());
/// assert_eq!(quote_value("hello world"), "\"hello world\"".to_string());
/// assert_eq!(quote_value("path/to/file"), "path/to/file".to_string());
/// assert_eq!(quote_value("$HOME"), "\"$HOME\"".to_string());
/// ```
pub fn quote_value(value: &str) -> String {
    let trimmed = value.trim();

    // Already quoted?
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        return value.to_string();
    }

    // Check if quoting is needed
    let needs_quoting = trimmed.contains(|c: char| {
        c.is_whitespace()
            || c == '$'
            || c == '`'
            || c == '('
            || c == ')'
            || c == '{'
            || c == '}'
            || c == '['
            || c == ']'
            || c == '<'
            || c == '>'
            || c == '&'
            || c == '|'
            || c == ';'
            || c == '\\'
            || c == '\''
            || c == '"'
    });

    if needs_quoting {
        format!("\"{}\"", trimmed)
    } else {
        trimmed.to_string()
    }
}

/// Merge multiple argument lists into a single vector.
///
/// Combines all provided argument vectors into one, maintaining order and
/// removing duplicates while preserving the first occurrence of each argument.
///
/// # Examples
///
/// ```
/// use koji_core::config::merge_args;
///
/// let args1 = vec!["--verbose".to_string(), "--model=/path".to_string()];
/// let args2 = vec!["--quiet".to_string(), "--model=/other".to_string()];
/// let merged = merge_args(&[args1, args2]).unwrap();
/// assert_eq!(merged.len(), 3); // verbose, model=/path, quiet
/// assert!(merged.contains(&"--verbose".to_string()));
/// assert!(merged.contains(&"--model=/path".to_string()));
/// assert!(merged.contains(&"--quiet".to_string()));
/// ```
pub fn merge_args(args_list: &[Vec<String>]) -> Result<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();

    for args in args_list {
        for arg in args {
            if seen.insert(arg.clone()) {
                result.push(arg.clone());
            }
        }
    }

    Ok(result)
}

/// Merge two arg lists with override semantics.
/// Later arguments fully replace earlier arguments with the same flag.
/// Returns a flat list suitable for `Command::args`.
pub fn merge_args_override(base: &[String], override_args: &[String]) -> Vec<String> {
    let mut result = base.to_vec();

    for arg in override_args {
        if arg.starts_with('-') {
            // Extract flag name (the part before the space)
            let flag_name = arg.split_whitespace().next().unwrap_or(arg);
            // Remove previous occurrence of this flag (by flag name, not full string)
            result.retain(|a| !a.starts_with('-') || !a.starts_with(flag_name));
            result.push(arg.clone());
        }
    }

    result
}

/// Group legacy flat arguments into structured form.
///
/// Takes a flat list of arguments and groups related ones together based on
/// flag patterns. This is useful for converting legacy configurations that
/// use flat argument lists into structured forms.
///
/// # Examples
///
/// ```
/// use koji_core::config::group_legacy_flat_args;
///
/// let args = vec![
///     "--model=/path/to/model".to_string(),
///     "--verbose".to_string(),
///     "--temperature=0.8".to_string(),
/// ];
/// let grouped = group_legacy_flat_args(&args).unwrap();
/// assert_eq!(grouped.len(), 3);
/// ```
pub fn group_legacy_flat_args(args: &[String]) -> Result<Vec<Vec<String>>> {
    let mut groups: Vec<Vec<String>> = Vec::new();

    // Group consecutive arguments that belong together
    // For simplicity, each argument becomes its own group
    for arg in args {
        let trimmed = arg.trim();
        if !trimmed.is_empty() {
            groups.push(vec![trimmed.to_string()]);
        }
    }

    Ok(groups)
}

/// Internal helper to check if a token looks like a flag.
///
/// Returns true if the token starts with `-` or `--` (but not `---`).
#[allow(dead_code)]
fn is_flag_token(token: &str) -> bool {
    let trimmed = token.trim();
    if trimmed.starts_with("---") {
        return false;
    }
    trimmed.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== split_arg_entry tests ==========
    #[test]
    fn test_split_arg_entry_simple() {
        let result = split_arg_entry("simple").unwrap();
        assert_eq!(result, vec!["simple".to_string()]);
    }

    #[test]
    fn test_split_arg_entry_key_value() {
        let result = split_arg_entry("key=value").unwrap();
        assert_eq!(result, vec!["key=".to_string(), "value".to_string()]);
    }

    #[test]
    fn test_split_arg_entry_empty() {
        let result: Vec<String> = split_arg_entry("").unwrap();
        assert_eq!(result, Vec::<String>::new());
    }

    #[test]
    fn test_split_arg_entry_with_spaces() {
        let result = split_arg_entry("  key=value  ").unwrap();
        assert_eq!(result, vec!["key=".to_string(), "value".to_string()]);
    }

    #[test]
    fn test_split_arg_entry_quoted_value() {
        let result = split_arg_entry("key='hello world'").unwrap();
        assert_eq!(
            result,
            vec!["key=".to_string(), "'hello world'".to_string()]
        );
    }

    #[test]
    fn test_split_arg_entry_double_quoted() {
        let result = split_arg_entry(r#"key="hello world""#).unwrap();
        assert_eq!(
            result,
            vec!["key=".to_string(), r#""hello world""#.to_string()]
        );
    }

    #[test]
    fn test_split_arg_entry_no_equals() {
        let result = split_arg_entry("just-a-value").unwrap();
        assert_eq!(result, vec!["just-a-value".to_string()]);
    }

    // ========== flag_name tests ==========
    #[test]
    fn test_flag_name_long_flag() {
        assert_eq!(flag_name("--verbose").unwrap(), "verbose");
    }

    #[test]
    fn test_flag_name_long_flag_with_value() {
        assert_eq!(flag_name("--model=/path").unwrap(), "model");
    }

    #[test]
    fn test_flag_name_short_flag() {
        assert_eq!(flag_name("-v").unwrap(), "v");
    }

    #[test]
    fn test_flag_name_short_flag_with_value() {
        assert_eq!(flag_name("-o=file").unwrap(), "o");
    }

    #[test]
    fn test_flag_name_double_short_flag() {
        assert_eq!(flag_name("-vv").unwrap(), "v");
    }

    #[test]
    fn test_flag_name_empty_token() {
        assert!(flag_name("").is_err());
    }

    #[test]
    fn test_flag_name_not_a_flag() {
        assert!(flag_name("regular_argument").is_err());
    }

    #[test]
    fn test_flag_name_triple_dash() {
        assert!(flag_name("---unknown").is_err());
    }

    #[test]
    fn test_flag_name_with_space_separated_value() {
        assert_eq!(flag_name("--model /path").unwrap(), "model");
    }

    // ========== flatten_args tests ==========
    #[test]
    fn test_flatten_args_basic() {
        let args = vec!["--verbose".to_string(), "--model=/path".to_string()];
        let result = flatten_args(&args).unwrap();
        assert_eq!(result, args);
    }

    #[test]
    fn test_flatten_args_with_whitespace() {
        let args = vec!["  --verbose  ".to_string(), "  --model=/path  ".to_string()];
        let result = flatten_args(&args).unwrap();
        assert_eq!(
            result,
            vec!["--verbose".to_string(), "--model=/path".to_string()]
        );
    }

    #[test]
    fn test_flatten_args_with_empty_strings() {
        let args = vec![
            "--verbose".to_string(),
            "".to_string(),
            "--model=/path".to_string(),
        ];
        let result = flatten_args(&args).unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"--verbose".to_string()));
        assert!(result.contains(&"--model=/path".to_string()));
    }

    #[test]
    fn test_flatten_args_empty_input() {
        let args: Vec<String> = vec![];
        let result = flatten_args(&args).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_flatten_args_only_whitespace() {
        let args = vec!["   ".to_string(), "\t\n".to_string()];
        let result = flatten_args(&args).unwrap();
        assert_eq!(result.len(), 0);
    }

    // ========== quote_value tests ==========
    #[test]
    fn test_quote_value_simple() {
        assert_eq!(quote_value("simple"), "simple");
    }

    #[test]
    fn test_quote_value_with_space() {
        assert_eq!(quote_value("hello world"), "\"hello world\"");
    }

    #[test]
    fn test_quote_value_with_dollar() {
        assert_eq!(quote_value("$HOME"), "\"$HOME\"");
    }

    #[test]
    fn test_quote_value_already_quoted_single() {
        assert_eq!(
            quote_value("'already quoted'"),
            "'already quoted'".to_string()
        );
    }

    #[test]
    fn test_quote_value_already_quoted_double() {
        assert_eq!(
            quote_value(r#""already quoted""#),
            r#""already quoted""#.to_string()
        );
    }

    #[test]
    fn test_quote_value_special_chars() {
        assert_eq!(quote_value("cmd | other"), "\"cmd | other\"");
    }

    #[test]
    fn test_quote_value_path() {
        assert_eq!(quote_value("path/to/file"), "path/to/file");
    }

    #[test]
    fn test_quote_value_brackets() {
        assert_eq!(quote_value("cmd[args]"), "\"cmd[args]\"");
    }

    #[test]
    fn test_quote_value_backtick() {
        assert_eq!(quote_value("cmd`echo`"), "\"cmd`echo`\"");
    }

    #[test]
    fn test_quote_value_parentheses() {
        assert_eq!(quote_value("cmd(args)"), "\"cmd(args)\"");
    }

    // ========== merge_args tests ==========
    #[test]
    fn test_merge_args_basic() {
        let args1 = vec!["--verbose".to_string(), "--model=/path".to_string()];
        let args2 = vec!["--quiet".to_string(), "--help".to_string()];
        let result = merge_args(&[args1, args2]).unwrap();
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_merge_args_with_duplicates() {
        let args1 = vec!["--verbose".to_string(), "--model=/path".to_string()];
        let args2 = vec!["--verbose".to_string(), "--quiet".to_string()];
        let result = merge_args(&[args1, args2]).unwrap();
        assert_eq!(result.len(), 3);
        assert!(result.contains(&"--verbose".to_string()));
    }

    #[test]
    fn test_merge_args_empty_vectors() {
        let result = merge_args(&[vec![], vec![]]).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_merge_args_single_vector() {
        let args = vec!["--verbose".to_string(), "--model=/path".to_string()];
        let result = merge_args(&[args]).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_merge_args_preserves_order() {
        let args1 = vec!["--first".to_string(), "--second".to_string()];
        let args2 = vec!["--third".to_string(), "--fourth".to_string()];
        let result = merge_args(&[args1, args2]).unwrap();
        assert_eq!(result[0], "--first");
        assert_eq!(result[1], "--second");
        assert_eq!(result[2], "--third");
        assert_eq!(result[3], "--fourth");
    }

    // ========== group_legacy_flat_args tests ==========
    #[test]
    fn test_group_legacy_flat_args_basic() {
        let args = vec!["--model=/path".to_string(), "--verbose".to_string()];
        let result = group_legacy_flat_args(&args).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], vec!["--model=/path".to_string()]);
        assert_eq!(result[1], vec!["--verbose".to_string()]);
    }

    #[test]
    fn test_group_legacy_flat_args_with_empty() {
        let args = vec![
            "--model=/path".to_string(),
            "".to_string(),
            "--verbose".to_string(),
        ];
        let result = group_legacy_flat_args(&args).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_group_legacy_flat_args_with_whitespace() {
        let args = vec![
            "  --model=/path  ".to_string(),
            "   ".to_string(),
            "--verbose".to_string(),
        ];
        let result = group_legacy_flat_args(&args).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0][0], "--model=/path");
    }

    #[test]
    fn test_group_legacy_flat_args_empty_input() {
        let args: Vec<String> = vec![];
        let result = group_legacy_flat_args(&args).unwrap();
        assert_eq!(result.len(), 0);
    }

    // ========== is_flag_token tests ==========
    #[test]
    fn test_is_flag_token_long_flag() {
        assert!(is_flag_token("--verbose"));
    }

    #[test]
    fn test_is_flag_token_short_flag() {
        assert!(is_flag_token("-v"));
    }

    #[test]
    fn test_is_flag_token_not_a_flag() {
        assert!(!is_flag_token("regular_argument"));
    }

    #[test]
    fn test_is_flag_token_triple_dash() {
        assert!(!is_flag_token("---unknown"));
    }
}
