//! Helpers for the grouped args format.
//!
//! On disk, both `BackendConfig.default_args` and `ModelConfig.args` are
//! `Vec<String>` where each element is **one logical flag** in shell-like
//! form, e.g. `"-fa 1"`, `"--mlock"`, `"-b 4096"`, or
//! `"--chat-template \"system: hi\""`.
//!
//! At runtime, immediately before invoking the child process, this
//! representation is flattened into the flat token list that
//! `std::process::Command::args` expects, e.g. `["-fa", "1", "-b", "4096"]`.
//!
//! Two invariants govern this module:
//!
//! 1. `merge_args` operates on **grouped** entries.
//! 2. `flatten_args` is the **only** way grouped entries become flat tokens
//!    that get handed to a child process. Any flag in a model's `args`
//!    completely replaces the same flag in the parent backend's
//!    `default_args` via `merge_args` before the flatten step.

use std::borrow::Cow;

/// Quote-aware split of a single grouped arg entry into runtime tokens.
///
/// Examples:
/// - `"-b 4096"` → `["-b", "4096"]`
/// - `"--mlock"` → `["--mlock"]`
/// - `"--chat-template \"system: hi\""` → `["--chat-template", "system: hi"]`
///
/// On parse failure (e.g. unbalanced quotes), logs a warning and falls back
/// to plain whitespace split. The fallback may corrupt values containing
/// spaces — that's why the warning is emitted.
///
/// # Examples
///
/// ```
/// use koji_core::config::split_arg_entry;
///
/// assert_eq!(split_arg_entry("-b 4096"), vec!["-b", "4096"]);
/// assert_eq!(split_arg_entry("--mlock"), vec!["--mlock"]);
/// ```
pub fn split_arg_entry(entry: &str) -> Vec<String> {
    match shlex::split(entry) {
        Some(tokens) => tokens,
        None => {
            tracing::warn!(
                "shlex failed to parse arg entry {:?} (likely unbalanced quotes); \
                 falling back to whitespace split which may corrupt values containing spaces",
                entry
            );
            entry.split_whitespace().map(String::from).collect()
        }
    }
}

/// Return the flag name (first whitespace-delimited token, with any
/// `=value` suffix stripped) of a grouped entry, only if the first token
/// looks like a flag (`-x`, `--xyz`, `--foo=bar`) and is not a negative
/// number.
///
/// Returns `None` for entries whose first token is a value, the POSIX
/// `--` end-of-options marker, a single `-` (stdin placeholder), or a
/// negative number like `-1` / `-0.5`.
///
/// # Examples
///
/// ```
/// use koji_core::config::flag_name;
///
/// assert_eq!(flag_name("--port 8080"), Some("--port"));
/// assert_eq!(flag_name("-b 4096"), Some("-b"));
/// assert_eq!(flag_name("--mlock"), Some("--mlock"));
/// assert_eq!(flag_name("-1"), None);
/// ```
pub fn flag_name(entry: &str) -> Option<&str> {
    let first = entry.split_whitespace().next()?;
    if !first.starts_with('-') {
        return None;
    }
    // Reject single "-" (stdin placeholder) and bare "--" (end-of-options).
    if first == "-" || first == "--" {
        return None;
    }
    // Reject negative numbers like "-1", "-0.5", "-.5"
    let after = first.trim_start_matches('-');
    if let Some(c) = after.chars().next() {
        if c.is_ascii_digit() || c == '.' {
            return None;
        }
    } else {
        // After trimming all dashes, nothing remains. Already handled above
        // by the "-"/"--" guards, but be defensive.
        return None;
    }
    // Strip inline `=value` form: `--port=8080` → flag name `--port`
    Some(first.split('=').next().unwrap_or(first))
}

/// Flatten grouped entries into the flat token list that
/// `Command::args` expects.
///
/// # Examples
///
/// ```
/// use koji_core::config::flatten_args;
///
/// let grouped = vec!["-fa 1".to_string(), "--mlock".to_string(), "-b 4096".to_string()];
/// assert_eq!(
///     flatten_args(&grouped),
///     vec!["-fa", "1", "--mlock", "-b", "4096"]
/// );
/// ```
pub fn flatten_args(grouped: &[String]) -> Vec<String> {
    grouped.iter().flat_map(|e| split_arg_entry(e)).collect()
}

/// Quote a value for inclusion in a grouped entry. Wraps `shlex::try_quote`
/// and falls back to a hand-rolled double-quoted form if quoting fails
/// (e.g. value contains a NUL byte). The result is safe to round-trip
/// through `split_arg_entry`.
///
/// # Examples
///
/// ```
/// use koji_core::config::quote_value;
///
/// assert_eq!(quote_value("simple").as_ref(), "simple");
/// // Values with spaces get quoted by shlex (single quotes).
/// let quoted = quote_value("hello world");
/// assert!(quoted.contains("hello world"));
/// ```
pub fn quote_value(value: &str) -> Cow<'_, str> {
    match shlex::try_quote(value) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(
                "shlex::try_quote failed for value {:?}: {} — falling back to naive quoting",
                value,
                e
            );
            // Naive fallback: strip NULs and double-quote.
            let cleaned: String = value.chars().filter(|c| *c != '\0').collect();
            Cow::Owned(format!("\"{}\"", cleaned.replace('"', "\\\"")))
        }
    }
}

/// Merge a base list with overrides. Each entry is one logical flag.
///
/// For every flag name appearing in `overrides`, drop **all** entries in
/// `base` with the same flag name; preserve `base` order for the kept
/// entries; then append `overrides` verbatim at the end.
///
/// `--port=8080` and `--port 9090` are recognised as the same flag.
///
/// **Note:** Duplicates *within* `overrides` are intentionally preserved.
/// We only de-duplicate *across* layers (backend vs model). If a user
/// writes `-b` twice in a single `args = [...]`, they get `-b` twice on
/// the command line. See `merge_overrides_with_internal_duplicates_preserved`.
///
/// Non-flag entries in `overrides` (e.g. positional/orphan tokens) are
/// passed through and appended at the end alongside flag entries.
///
/// # Examples
///
/// ```
/// use koji_core::config::merge_args;
///
/// let base = vec!["-b 2048".to_string(), "-t 14".to_string()];
/// let overrides = vec!["-b 4096".to_string()];
/// let merged = merge_args(&base, &overrides);
/// assert_eq!(merged, vec!["-t 14".to_string(), "-b 4096".to_string()]);
/// ```
pub fn merge_args(base: &[String], overrides: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let override_flags: HashSet<&str> = overrides.iter().filter_map(|e| flag_name(e)).collect();

    let mut out: Vec<String> = base
        .iter()
        .filter(|e| flag_name(e).is_none_or(|f| !override_flags.contains(f)))
        .cloned()
        .collect();
    out.extend(overrides.iter().cloned());
    out
}

/// One-shot legacy migration: convert flat token lists to the grouped
/// form. Values containing whitespace are shlex-quoted via `quote_value`
/// so that the round-trip through `split_arg_entry` is lossless.
///
/// Idempotent: already-grouped entries (those containing whitespace) and
/// orphan tokens pass through unchanged.
///
/// Returns `(migrated, did_change)` where `did_change` is `true` iff the
/// output differs from the input. Callers use this to decide whether to
/// rewrite the file on disk and to log a one-time migration message.
///
/// # Examples
///
/// ```
/// use koji_core::config::group_legacy_flat_args;
///
/// let flat = vec!["-fa".to_string(), "1".to_string(), "-b".to_string(), "2048".to_string()];
/// let (out, changed) = group_legacy_flat_args(&flat);
/// assert_eq!(out, vec!["-fa 1".to_string(), "-b 2048".to_string()]);
/// assert!(changed);
/// ```
pub fn group_legacy_flat_args(flat: &[String]) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    while i < flat.len() {
        let cur = &flat[i];
        // If this entry already contains whitespace, treat it as already grouped.
        if cur.contains(char::is_whitespace) {
            out.push(cur.clone());
            i += 1;
            continue;
        }
        // Standalone non-flag token (orphan value) — keep verbatim.
        let cur_is_flag = is_flag_token(cur);
        if !cur_is_flag {
            out.push(cur.clone());
            i += 1;
            continue;
        }
        // It's a flag. Look at next token: if it's a value (not a flag,
        // or is a negative number), join the two.
        let next = flat.get(i + 1);
        // Skip joining if current token already has inline =value form.
        let next_is_value = match next {
            Some(_n) if cur.contains('=') => false,
            Some(n) => !is_flag_token(n),
            None => false,
        };
        if next_is_value {
            let n = next.unwrap();
            // Quote values that contain whitespace so the grouped entry
            // can round-trip through split_arg_entry.
            let joined = if n.contains(char::is_whitespace) {
                format!("{} {}", cur, quote_value(n))
            } else {
                format!("{} {}", cur, n)
            };
            out.push(joined);
            i += 2;
        } else {
            out.push(cur.clone());
            i += 1;
        }
    }
    let did_change = out != flat;
    (out, did_change)
}

/// Internal: does this token look like a flag? Mirrors `flag_name`'s
/// criteria but operates on a raw token rather than a grouped entry.
fn is_flag_token(tok: &str) -> bool {
    if !tok.starts_with('-') || tok == "-" || tok == "--" {
        return false;
    }
    let after = tok.trim_start_matches('-');
    match after.chars().next() {
        Some(c) => !c.is_ascii_digit() && c != '.',
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- flag_name ----------

    #[test]
    fn flag_name_long_with_value() {
        assert_eq!(flag_name("--port 8080"), Some("--port"));
    }

    #[test]
    fn flag_name_long_inline_equals() {
        assert_eq!(flag_name("--port=8080"), Some("--port"));
    }

    #[test]
    fn flag_name_short_with_value() {
        assert_eq!(flag_name("-b 4096"), Some("-b"));
    }

    #[test]
    fn flag_name_boolean_long() {
        assert_eq!(flag_name("--mlock"), Some("--mlock"));
    }

    #[test]
    fn flag_name_negative_int_is_not_flag() {
        assert_eq!(flag_name("-1"), None);
    }

    #[test]
    fn flag_name_negative_float_is_not_flag() {
        assert_eq!(flag_name("-0.5"), None);
        assert_eq!(flag_name("-.5"), None);
    }

    #[test]
    fn flag_name_non_flag_value() {
        assert_eq!(flag_name("model.gguf"), None);
    }

    #[test]
    fn flag_name_empty_returns_none() {
        assert_eq!(flag_name(""), None);
    }

    #[test]
    fn flag_name_single_dash_is_not_flag() {
        // POSIX stdin placeholder
        assert_eq!(flag_name("-"), None);
    }

    #[test]
    fn flag_name_double_dash_is_not_flag() {
        // POSIX end-of-options marker
        assert_eq!(flag_name("--"), None);
    }

    // ---------- split_arg_entry ----------

    #[test]
    fn split_simple_pair() {
        assert_eq!(split_arg_entry("-b 4096"), vec!["-b", "4096"]);
    }

    #[test]
    fn split_boolean_flag() {
        assert_eq!(split_arg_entry("--mlock"), vec!["--mlock"]);
    }

    #[test]
    fn split_quoted_value_with_space() {
        assert_eq!(
            split_arg_entry("--chat-template \"system: hi\""),
            vec!["--chat-template", "system: hi"]
        );
    }

    #[test]
    fn split_quoted_path_with_spaces() {
        assert_eq!(
            split_arg_entry("-m \"C:/Path With Spaces/model.gguf\""),
            vec!["-m", "C:/Path With Spaces/model.gguf"]
        );
    }

    #[test]
    fn split_unbalanced_quotes_falls_back() {
        // Falls back to whitespace split. The fallback is lossy but should
        // not panic.
        let result = split_arg_entry("--bad \"unterminated");
        assert!(!result.is_empty());
    }

    // ---------- flatten_args ----------

    #[test]
    fn flatten_basic() {
        let grouped = vec![
            "-fa 1".to_string(),
            "--mlock".to_string(),
            "-b 4096".to_string(),
        ];
        let flat = flatten_args(&grouped);
        assert_eq!(flat, vec!["-fa", "1", "--mlock", "-b", "4096"]);
    }

    #[test]
    fn flatten_preserves_quoted_paths() {
        let grouped = vec!["-m \"C:/Path With Spaces/m.gguf\"".to_string()];
        let flat = flatten_args(&grouped);
        assert_eq!(flat, vec!["-m", "C:/Path With Spaces/m.gguf"]);
    }

    // ---------- merge_args ----------

    #[test]
    fn merge_overrides_short_flag() {
        let base = vec![
            "-b 2048".to_string(),
            "-ub 512".to_string(),
            "--mlock".to_string(),
        ];
        let overrides = vec!["-b 4096".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(
            result,
            vec![
                "-ub 512".to_string(),
                "--mlock".to_string(),
                "-b 4096".to_string()
            ]
        );
    }

    #[test]
    fn merge_overrides_drop_all_base_occurrences() {
        // Multiple base entries with the same flag name → all dropped.
        let base = vec![
            "-b 2048".to_string(),
            "-b 1024".to_string(),
            "-ub 512".to_string(),
        ];
        let overrides = vec!["-b 4096".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(result, vec!["-ub 512".to_string(), "-b 4096".to_string()]);
    }

    #[test]
    fn merge_inline_equals_in_base_overridden_by_space_form() {
        let base = vec!["--port=8080".to_string()];
        let overrides = vec!["--port 9090".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(result, vec!["--port 9090".to_string()]);
    }

    #[test]
    fn merge_space_form_in_base_overridden_by_inline_equals() {
        let base = vec!["--port 8080".to_string()];
        let overrides = vec!["--port=9090".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(result, vec!["--port=9090".to_string()]);
    }

    #[test]
    fn merge_boolean_flag_dedupes() {
        let base = vec!["--mlock".to_string(), "-fa 1".to_string()];
        let overrides = vec!["--mlock".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(result, vec!["-fa 1".to_string(), "--mlock".to_string()]);
    }

    #[test]
    fn merge_disjoint_flags_concatenate() {
        let base = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let overrides = vec!["--mlock".to_string(), "-t 14".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(
            result,
            vec![
                "-fa 1".to_string(),
                "-b 2048".to_string(),
                "--mlock".to_string(),
                "-t 14".to_string(),
            ]
        );
    }

    #[test]
    fn merge_empty_overrides_returns_base_clone() {
        let base = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let result = merge_args(&base, &[]);
        assert_eq!(result, base);
    }

    #[test]
    fn merge_empty_base_returns_overrides_clone() {
        let overrides = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let result = merge_args(&[], &overrides);
        assert_eq!(result, overrides);
    }

    #[test]
    fn merge_preserves_base_order_for_kept_entries() {
        let base = vec![
            "-fa 1".to_string(),
            "-b 2048".to_string(),
            "-ub 512".to_string(),
            "-t 14".to_string(),
        ];
        let overrides = vec!["-b 4096".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(
            result,
            vec![
                "-fa 1".to_string(),
                "-ub 512".to_string(),
                "-t 14".to_string(),
                "-b 4096".to_string(),
            ]
        );
    }

    #[test]
    fn merge_negative_value_not_treated_as_flag() {
        // -ngl -1 is a flag with a negative-int value. Override should
        // replace the entire entry.
        let base = vec!["-ngl -1".to_string()];
        let overrides = vec!["-ngl 999".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(result, vec!["-ngl 999".to_string()]);
    }

    #[test]
    fn merge_long_flag_with_zero_value_overridden() {
        // --ctx-checkpoints 8 in base, overridden by --ctx-checkpoints 0
        // in model args. The override value of 0 must win and the base
        // entry must be dropped.
        let base = vec!["--ctx-checkpoints 8".to_string(), "-fa 1".to_string()];
        let overrides = vec!["--ctx-checkpoints 0".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(
            result,
            vec!["-fa 1".to_string(), "--ctx-checkpoints 0".to_string()]
        );
    }

    #[test]
    fn merge_overrides_with_internal_duplicates_preserved() {
        // Intentional behavior: duplicates *within* overrides are
        // preserved. We trust user input. If someone writes -b twice in
        // a single args = [...], they get -b twice on the command line.
        // The plan only deduplicates *across* layers (backend vs model),
        // not within a single layer.
        let result = merge_args(&[], &["-b 1024".to_string(), "-b 4096".to_string()]);
        assert_eq!(result, vec!["-b 1024".to_string(), "-b 4096".to_string()]);
    }

    #[test]
    fn merge_non_flag_override_passes_through() {
        // Non-flag entries in overrides (e.g. orphan/positional tokens)
        // must be appended verbatim, not silently dropped.
        let base = vec!["-fa 1".to_string()];
        let overrides = vec!["positional".to_string(), "-b 4096".to_string()];
        let result = merge_args(&base, &overrides);
        assert_eq!(
            result,
            vec![
                "-fa 1".to_string(),
                "positional".to_string(),
                "-b 4096".to_string(),
            ]
        );
    }

    // ---------- group_legacy_flat_args ----------

    #[test]
    fn group_basic_pairs() {
        let flat = vec![
            "-fa".to_string(),
            "1".to_string(),
            "-b".to_string(),
            "2048".to_string(),
        ];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["-fa 1".to_string(), "-b 2048".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_idempotent_on_already_grouped() {
        let grouped = vec!["-fa 1".to_string(), "-b 2048".to_string()];
        let (out, changed) = group_legacy_flat_args(&grouped);
        assert_eq!(out, grouped);
        assert!(!changed);
    }

    #[test]
    fn group_consecutive_boolean_flags() {
        let flat = vec!["--mlock".to_string(), "--no-mmap".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, flat);
        assert!(!changed);
    }

    #[test]
    fn group_mixed_boolean_and_pair() {
        let flat = vec!["-fa".to_string(), "1".to_string(), "--mlock".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["-fa 1".to_string(), "--mlock".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_negative_int_value() {
        let flat = vec!["-ngl".to_string(), "-1".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["-ngl -1".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_negative_float_value() {
        let flat = vec!["--min-p".to_string(), "-0.05".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["--min-p -0.05".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_path_with_spaces_quoted() {
        let flat = vec!["-m".to_string(), "C:/path with space/m.gguf".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out.len(), 1);
        // Re-split should round-trip losslessly back to the original two tokens.
        let re_flat = split_arg_entry(&out[0]);
        assert_eq!(re_flat, vec!["-m", "C:/path with space/m.gguf"]);
        assert!(changed);
    }

    #[test]
    fn group_inline_equals_passes_through() {
        let flat = vec![
            "--port=8080".to_string(),
            "-fa".to_string(),
            "1".to_string(),
        ];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["--port=8080".to_string(), "-fa 1".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_orphan_value_passes_through() {
        let flat = vec!["orphan".to_string(), "-fa".to_string(), "1".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["orphan".to_string(), "-fa 1".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_empty_input_no_change() {
        let flat: Vec<String> = vec![];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert!(out.is_empty());
        assert!(!changed);
    }

    #[test]
    fn group_user_real_world_example() {
        // This is the canonical example from the user: a mix of standalone
        // boolean flags (`--no-mmap`, `--cpu-moe`) interleaved with
        // flag+value pairs. Standalone flags must remain as their own
        // entries; pairs must be joined.
        let flat = vec![
            "--no-mmap".to_string(),
            "--cpu-moe".to_string(),
            "-b".to_string(),
            "4096".to_string(),
            "-ub".to_string(),
            "4096".to_string(),
            "-ctk".to_string(),
            "q4_0".to_string(),
            "-ctv".to_string(),
            "q4_0".to_string(),
        ];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(
            out,
            vec![
                "--no-mmap".to_string(),
                "--cpu-moe".to_string(),
                "-b 4096".to_string(),
                "-ub 4096".to_string(),
                "-ctk q4_0".to_string(),
                "-ctv q4_0".to_string(),
            ]
        );
        assert!(changed);
    }

    #[test]
    fn group_long_flag_with_zero_value() {
        // Regression: `--ctx-checkpoints 0` looks like a boolean-style long
        // flag but actually takes an explicit value (0). The "0" is a digit
        // so `is_flag_token` returns false for it, and the lookahead must
        // therefore JOIN it with the preceding `--ctx-checkpoints` rather
        // than treat it as an orphan or as a separate boolean flag.
        let flat = vec!["--ctx-checkpoints".to_string(), "0".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["--ctx-checkpoints 0".to_string()]);
        assert!(changed);
    }

    #[test]
    fn group_long_flag_with_zero_value_followed_by_boolean() {
        // Same as above, but followed by a standalone boolean flag, to
        // make sure the lookahead doesn't accidentally consume it.
        let flat = vec![
            "--ctx-checkpoints".to_string(),
            "0".to_string(),
            "--no-mmap".to_string(),
        ];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(
            out,
            vec!["--ctx-checkpoints 0".to_string(), "--no-mmap".to_string()]
        );
        assert!(changed);
    }

    #[test]
    fn group_inline_equals_flag_not_joined() {
        // Regression: `--port=8080` should be left alone, not joined with
        // the next token.
        let flat = vec![
            "--port=8080".to_string(),
            "-fa".to_string(),
            "1".to_string(),
        ];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out, vec!["--port=8080".to_string(), "-fa 1".to_string()]);
        assert!(changed);
    }
}
