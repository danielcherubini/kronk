# Grouped Args Format + Backend↔Model Dedup Plan

**Goal:** Store args as one logical flag per array entry (e.g. `"-fa 1"`, `"--mlock"`) for readable `config.toml`, and ensure any flag in `model.args` completely replaces the same flag in `backend.default_args`.

**Status:** ✅ COMPLETED - See git commits `5c8fac1` ("feat(config): add shlex and grouped-args helper functions with tests"), `3fbf27b` ("feat(config): wire merge_args into build_args and grouped SamplingParams::to_args"), `ae67a0b` ("refactor(config): rework args_helpers to use shlex and align with grouped-args plan")

**Architecture:** Add the `shlex` crate for quote-aware splitting. Introduce a `merge_args` helper that operates on grouped form and de-duplicates by flag name. Flatten back to tokens at the `Command::args` boundary. Auto-migrate legacy flat configs on load and write the migrated form back to disk.

**Tech Stack:** Rust, `toml` crate, `shlex` crate (zero deps), Leptos (web frontend, built via `trunk`).

---

## Important Cross-Cutting Invariants

These invariants are referenced by multiple tasks; read them before starting any task.

1. **`build_full_args` MUST always return flat tokens** (one CLI token per `Vec` element). This is its existing contract. `proxy/lifecycle.rs`, `bench/runner.rs`, `proxy/process.rs::override_arg`, and `bench/runner.rs::_override_arg` all depend on this. The very last operation in `build_full_args` must therefore be `flatten_args(&grouped)`.

2. **`build_args` returns flat tokens** for consistency with `build_full_args`. (Currently has no in-workspace callers but is public API and may be used by external consumers of `koji-core`. The two new tests added in Task 2a are the primary regression guard.)

3. **Helper visibility**: `koji-cli` (a separate crate) imports things via `use koji_core::config::...`. So the helpers must be `pub use`-d from `crates/koji-core/src/config/mod.rs`, not just declared as a private/`pub(crate)` submodule.

4. **`shlex::try_quote` is the modern API**. `shlex::quote` is deprecated (since 1.3.0). Use `try_quote` and `?`/`unwrap_or_else` on its `Result<Cow<'_, str>, QuoteError>` return type.

5. **Migrating on load must persist to disk** if anything changed, otherwise the verification checklist's "inspect config.toml after running koji status" step will silently fail. Use a `(migrated_args, did_change)` return tuple from the helper so we can detect changes without a separate compare pass.

---

## Task 1: Add `shlex` dependency and implement helper functions with full unit tests

**Context:**
This task sets up the foundation. We add the `shlex` crate, create a new `args_helpers` submodule of `config`, implement five pure helper functions, expose them via `pub use` from `config/mod.rs` so other crates can reach them, and pin every helper with unit tests covering its edge cases. No production-code call sites are touched in this task — it is purely additive. After this commit, the new helpers exist but nothing in the build pipeline uses them yet.

**Files:**
- Modify: `Cargo.toml` (workspace root) — add `shlex` to `[workspace.dependencies]`
- Modify: `crates/koji-core/Cargo.toml` — add `shlex = { workspace = true }` under `[dependencies]`
- Create: `crates/koji-core/src/config/args_helpers.rs` (new file, ~280 lines including tests)
- Modify: `crates/koji-core/src/config/mod.rs` — add `mod args_helpers;` and `pub use args_helpers::{...};`

**What to implement:**

### 1.1 — `Cargo.toml` (workspace root)

Add to the existing `[workspace.dependencies]` table (alphabetical order):

```toml
shlex = "1.3"
```

### 1.2 — `crates/koji-core/Cargo.toml`

Add under `[dependencies]` (alphabetical order):

```toml
shlex = { workspace = true }
```

### 1.3 — `crates/koji-core/src/config/args_helpers.rs` (new file)

Create the file with this exact content:

```rust
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
pub fn flatten_args(grouped: &[String]) -> Vec<String> {
    grouped.iter().flat_map(|e| split_arg_entry(e)).collect()
}

/// Quote a value for inclusion in a grouped entry. Wraps `shlex::try_quote`
/// and falls back to a hand-rolled double-quoted form if quoting fails
/// (e.g. value contains a NUL byte). The result is safe to round-trip
/// through `split_arg_entry`.
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
pub fn merge_args(base: &[String], overrides: &[String]) -> Vec<String> {
    use std::collections::HashSet;
    let override_flags: HashSet<&str> = overrides.iter().filter_map(|e| flag_name(e)).collect();

    let mut out: Vec<String> = base
        .iter()
        .filter(|e| flag_name(e).map_or(true, |f| !override_flags.contains(f)))
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
        let next_is_value = match next {
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
        let grouped = vec!["-fa 1".to_string(), "--mlock".to_string(), "-b 4096".to_string()];
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
            vec!["-ub 512".to_string(), "--mlock".to_string(), "-b 4096".to_string()]
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
        let result = merge_args(
            &[],
            &["-b 1024".to_string(), "-b 4096".to_string()],
        );
        assert_eq!(result, vec!["-b 1024".to_string(), "-b 4096".to_string()]);
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
        let flat = vec![
            "-m".to_string(),
            "C:/path with space/m.gguf".to_string(),
        ];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(out.len(), 1);
        // Re-split should round-trip losslessly back to the original two tokens.
        let re_flat = split_arg_entry(&out[0]);
        assert_eq!(re_flat, vec!["-m", "C:/path with space/m.gguf"]);
        assert!(changed);
    }

    #[test]
    fn group_inline_equals_passes_through() {
        let flat = vec!["--port=8080".to_string(), "-fa".to_string(), "1".to_string()];
        let (out, changed) = group_legacy_flat_args(&flat);
        assert_eq!(
            out,
            vec!["--port=8080".to_string(), "-fa 1".to_string()]
        );
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
}
```

### 1.4 — `crates/koji-core/src/config/mod.rs`

Replace the existing file contents with:

```rust
mod args_helpers;
mod defaults;
mod loader;
mod migrate;
mod rename_legacy;
mod resolve;
mod types;

pub use args_helpers::{
    flag_name, flatten_args, group_legacy_flat_args, merge_args, quote_value, split_arg_entry,
};
pub use migrate::migrate_cards_to_unified_config;
pub use rename_legacy::{migrate_legacy_data_dir, Migration};
pub use types::{
    BackendConfig, Config, General, HealthCheck, ModelConfig, ProxyConfig, QuantEntry, Supervisor,
    DEFAULT_PROXY_PORT, MAX_REQUEST_BODY_SIZE,
};
```

**Steps:**
- [ ] Read `Cargo.toml` (workspace root) to confirm the `[workspace.dependencies]` section exists.
- [ ] Add `shlex = "1.3"` under `[workspace.dependencies]` in `Cargo.toml`.
- [ ] Run `cargo metadata --format-version 1 > /dev/null`
  - Did it succeed (exit 0)? If not, the workspace dependency syntax is wrong; fix before continuing.
- [ ] Read `crates/koji-core/Cargo.toml` to find the `[dependencies]` section.
- [ ] Add `shlex = { workspace = true }` under `[dependencies]` in `crates/koji-core/Cargo.toml`.
- [ ] Run `cargo check -p koji-core`
  - Did it succeed? If not, check the dependency syntax and re-run.
- [ ] Create `crates/koji-core/src/config/args_helpers.rs` with the exact content from section 1.3 above.
- [ ] Replace `crates/koji-core/src/config/mod.rs` with the exact content from section 1.4 above.
- [ ] Run `cargo check -p koji-core`
  - Did it succeed? If not, fix module declarations or syntax errors.
- [ ] Run `cargo test -p koji-core --lib -- config::args_helpers::tests`
  - Did all 42 tests in the `args_helpers::tests` module pass? If not, fix the implementations to match the test expectations.
- [ ] Run `cargo clippy -p koji-core -- -D warnings`
  - Did it succeed? Fix any warnings.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo fmt --all -- --check`
  - Did it succeed? If not, run `cargo fmt --all` again.
- [ ] Verify external import works by running `cargo check -p koji-cli` (smoke test that the `pub use` makes the helpers reachable from another crate).
- [ ] Commit with message: `feat(config): add shlex and grouped-args helper functions with tests`

**Acceptance criteria:**
- [ ] `crates/koji-core/Cargo.toml` declares `shlex` as a dependency.
- [ ] `crates/koji-core/src/config/args_helpers.rs` exists and contains all five public helpers (`split_arg_entry`, `flag_name`, `flatten_args`, `quote_value`, `merge_args`, `group_legacy_flat_args`) plus the private `is_flag_token`.
- [ ] All 42 unit tests in `args_helpers::tests` pass (10 `flag_name`, 5 `split_arg_entry`, 2 `flatten_args`, 12 `merge_args`, 13 `group_legacy_flat_args` — including `group_user_real_world_example` (canonical mix of standalone booleans + flag/value pairs: `--no-mmap`, `--cpu-moe`, `-b 4096`, `-ub 4096`, `-ctk q4_0`, `-ctv q4_0`), `group_long_flag_with_zero_value` and `group_long_flag_with_zero_value_followed_by_boolean` (regression for `--ctx-checkpoints 0` — a long flag whose explicit value happens to be `0`), `merge_long_flag_with_zero_value_overridden`, and `merge_overrides_with_internal_duplicates_preserved`).
- [ ] `pub use args_helpers::{...}` is in `config/mod.rs` so external crates can import via `koji_core::config::merge_args` etc.
- [ ] `cargo clippy -p koji-core -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes.

---

## Task 2a: Wire `merge_args` + `flatten_args` into `build_args` and update `SamplingParams::to_args`

**Context:**
With the helpers in place, this task wires them into `Config::build_args` (the simpler of the two arg builders) and converts `SamplingParams::to_args` to emit grouped form so it composes with `merge_args`. `Config::build_full_args` is intentionally left for Task 2b — it has more complex injection logic for `-m`/`-c`/`-ngl` and deserves its own commit so the diff is reviewable.

**The four existing `SamplingParams::to_args` callers** that this task will affect:
- `crates/koji-core/src/config/resolve.rs::build_args` (rewritten in this task)
- `crates/koji-core/src/config/resolve.rs::build_full_args` (will be rewritten in Task 2b — for now, the existing skip_next dedup logic still works because it consumes flat tokens; we leave it untouched)
- `crates/koji-core/src/profiles.rs` test `test_to_args_coding` (line 217) — assertion needs updating
- (no other callers — verified via grep for `to_args(`)

**The existing test that needs an updated assertion** is `test_to_args_coding` at `crates/koji-core/src/profiles.rs:217`. Its current assertion is:
```rust
assert_eq!(args, vec!["--temp", "0.30", "--top-k", "50"]);
```
After this task it becomes:
```rust
assert_eq!(args, vec!["--temp 0.30", "--top-k 50"]);
```

`test_to_args_empty` at line 228 still passes unchanged (empty vec).

**Files:**
- Modify: `crates/koji-core/src/profiles.rs` — rewrite `SamplingParams::to_args` and update `test_to_args_coding`
- Modify: `crates/koji-core/src/config/resolve.rs` — rewrite `Config::build_args`

**What to implement:**

### 2a.1 — `crates/koji-core/src/profiles.rs`

Replace `SamplingParams::to_args` (currently lines 106–139) with:

```rust
    /// Convert to grouped CLI args for llama.cpp backend.
    /// Each returned `String` is one logical flag, e.g. `"--temp 0.30"`.
    /// Run `flatten_args` on the result to get the flat token list that
    /// `Command::args` expects.
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(v) = self.temperature {
            args.push(format!("--temp {:.2}", v));
        }
        if let Some(v) = self.top_k {
            args.push(format!("--top-k {}", v));
        }
        if let Some(v) = self.top_p {
            args.push(format!("--top-p {:.2}", v));
        }
        if let Some(v) = self.min_p {
            args.push(format!("--min-p {:.2}", v));
        }
        if let Some(v) = self.presence_penalty {
            args.push(format!("--presence-penalty {:.2}", v));
        }
        if let Some(v) = self.frequency_penalty {
            args.push(format!("--frequency-penalty {:.2}", v));
        }
        if let Some(v) = self.repeat_penalty {
            args.push(format!("--repeat-penalty {:.2}", v));
        }
        args
    }
```

Update `test_to_args_coding` (line 217):
```rust
    #[test]
    fn test_to_args_coding() {
        let params = SamplingParams {
            temperature: Some(0.3),
            top_k: Some(50),
            ..Default::default()
        };
        let args = params.to_args();
        assert_eq!(args, vec!["--temp 0.30", "--top-k 50"]);
    }
```

### 2a.2 — `crates/koji-core/src/config/resolve.rs`

Replace `Config::build_args` (currently lines 144–181) with:

```rust
    /// Build the merged arg list for a server, returning **flat tokens**
    /// suitable for `Command::args`.
    ///
    /// Merging order: `backend.default_args` → `server.args` →
    /// `server.sampling.to_args()`. Each later layer's flags fully replace
    /// the same flag in the earlier layers via `merge_args`.
    pub fn build_args(&self, server: &ModelConfig, backend: &BackendConfig) -> Vec<String> {
        let mut grouped =
            crate::config::merge_args(&backend.default_args, &server.args);
        if let Some(sampling) = &server.sampling {
            if !sampling.is_empty() {
                grouped = crate::config::merge_args(&grouped, &sampling.to_args());
            }
        }
        crate::config::flatten_args(&grouped)
    }
```

Add this test to the existing `mod tests` block at the bottom of `resolve.rs`:

```rust
    #[test]
    fn build_args_dedupes_backend_vs_model_flags() {
        let mut config = Config::default();
        config.backends.insert(
            "test_backend".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![
                    "-b 2048".to_string(),
                    "-ub 512".to_string(),
                    "-t 14".to_string(),
                ],
                health_check_url: None,
                version: None,
            },
        );

        let server = ModelConfig {
            backend: "test_backend".to_string(),
            args: vec!["-b 4096".to_string(), "-ub 4096".to_string()],
            sampling: None,
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
        };

        let backend = config.backends.get("test_backend").unwrap().clone();
        let flat = config.build_args(&server, &backend);

        // -t 14 from base must survive
        assert!(flat.windows(2).any(|w| w == ["-t", "14"]));
        // -b appears exactly once with value 4096
        let b_count = flat.iter().filter(|t| *t == "-b").count();
        assert_eq!(b_count, 1, "expected exactly one -b token, got {:?}", flat);
        assert!(flat.windows(2).any(|w| w == ["-b", "4096"]));
        // -ub appears exactly once with value 4096
        let ub_count = flat.iter().filter(|t| *t == "-ub").count();
        assert_eq!(ub_count, 1, "expected exactly one -ub token, got {:?}", flat);
        assert!(flat.windows(2).any(|w| w == ["-ub", "4096"]));
        // 2048 and 512 must NOT appear
        assert!(!flat.iter().any(|t| t == "2048"));
        assert!(!flat.iter().any(|t| t == "512"));
    }

    #[test]
    fn build_args_sampling_overrides_inline_temp_in_args() {
        // Requires SamplingParams::to_args to already be in grouped form
        // (done earlier in this same task, section 2a.1). If this test
        // fails with a flat-token mismatch instead of a dedup failure,
        // the to_args rewrite was skipped.
        let mut config = Config::default();
        config.backends.insert(
            "test_backend".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![],
                health_check_url: None,
                version: None,
            },
        );

        let server = ModelConfig {
            backend: "test_backend".to_string(),
            // inline --temp in args should be overridden by sampling.temperature
            args: vec!["--temp 0.10".to_string()],
            sampling: Some(crate::profiles::SamplingParams {
                temperature: Some(0.5),
                ..Default::default()
            }),
            model: None,
            quant: None,
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants: std::collections::BTreeMap::new(),
        };

        let backend = config.backends.get("test_backend").unwrap().clone();
        let flat = config.build_args(&server, &backend);

        // --temp appears exactly once with value 0.50
        let temp_count = flat.iter().filter(|t| *t == "--temp").count();
        assert_eq!(temp_count, 1, "expected exactly one --temp token, got {:?}", flat);
        assert!(flat.windows(2).any(|w| w == ["--temp", "0.50"]));
        assert!(!flat.iter().any(|t| t == "0.10"));
    }
```

**Steps:**
- [ ] Read `crates/koji-core/src/profiles.rs` lines 106–140 to confirm the current `to_args` shape.
- [ ] Update `test_to_args_coding` in `profiles.rs` to assert `vec!["--temp 0.30", "--top-k 50"]` (this is the failing-test step — the current implementation would still produce `vec!["--temp", "0.30", "--top-k", "50"]`).
- [ ] Run `cargo test -p koji-core --lib -- profiles::tests::test_to_args_coding`
  - Did it fail with an assertion mismatch? If not, stop and investigate why.
- [ ] Replace `SamplingParams::to_args` in `profiles.rs` with the grouped-form implementation from section 2a.1.
- [ ] Run `cargo test -p koji-core --lib -- profiles::tests`
  - Did all `profiles::tests` pass (including `test_to_args_coding`, `test_to_args_empty`, and the preset_label tests which don't touch `to_args`)? If not, fix.
- [ ] Read `crates/koji-core/src/config/resolve.rs` lines 144–181 to confirm the current `build_args` shape.
- [ ] Add the two new tests `build_args_dedupes_backend_vs_model_flags` and `build_args_sampling_overrides_inline_temp_in_args` to `resolve.rs`'s existing `mod tests`.
- [ ] Run `cargo test -p koji-core --lib -- config::resolve::tests::build_args_dedupes_backend_vs_model_flags`
  - Did it fail (the current `build_args` does naive `extend` and won't dedup)? If not, stop and investigate.
- [ ] Replace `Config::build_args` in `resolve.rs` with the implementation from section 2a.2.
- [ ] Run `cargo test -p koji-core --lib -- config::resolve::tests`
  - Did all `resolve::tests` pass, including the two new ones? If not, fix.
- [ ] Run `cargo test -p koji-core --lib`
  - Did the full koji-core test suite pass? If not, investigate any other tests that depended on the old `to_args` flat format.
- [ ] Run `cargo clippy -p koji-core -- -D warnings`
  - Did it succeed? Fix any warnings.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo fmt --all -- --check`
  - Did it succeed?
- [ ] Commit with message: `feat(config): wire merge_args into build_args and grouped SamplingParams::to_args`

**Acceptance criteria:**
- [ ] `SamplingParams::to_args` returns one entry per logical flag (grouped form).
- [ ] `test_to_args_coding` asserts the grouped form and passes.
- [ ] `Config::build_args` returns flat tokens via `flatten_args`.
- [ ] `build_args_dedupes_backend_vs_model_flags` passes: backend `[-b 2048, -ub 512, -t 14]` + model `[-b 4096, -ub 4096]` → flat output contains exactly one `-b 4096`, one `-ub 4096`, preserves `-t 14`, and contains no `2048` or `512` tokens.
- [ ] `build_args_sampling_overrides_inline_temp_in_args` passes: inline `--temp 0.10` in args is fully replaced by `sampling.temperature = 0.5`, producing exactly one `--temp 0.50`.
- [ ] `cargo test -p koji-core --lib` passes overall.
- [ ] `cargo clippy -p koji-core -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes.

---

## Task 2b: Refactor `Config::build_full_args` to grouped form with dedup

**Context:**
This is the main bug-fix task. `Config::build_full_args` currently does naive `default_args.clone() + server.args.clone()` (lines 192–193) and only de-duplicates a hardcoded subset of flags. After this task, it operates on grouped form internally, uses `merge_args` to dedup backend↔model flags, uses `flag_name`-aware existence checks for the `-m`/`-c`/`-ngl` injection, properly shlex-quotes the model path so it round-trips through `flatten_args`, and calls `flatten_args` exactly once at the end. The function's external contract — returning flat tokens — is preserved.

**Why the contract preservation matters:** `proxy/lifecycle.rs:76-77` and `bench/runner.rs:106-107` call `override_arg`/`_override_arg` on the result of `build_full_args`, and those helpers operate on flat tokens. If `build_full_args` ever started returning grouped form, those callers would silently break. We add a `debug_assert!` at the end of `build_full_args` pinning the invariant.

**Files:**
- Modify: `crates/koji-core/src/config/resolve.rs` — rewrite `Config::build_full_args`
- Modify: `crates/koji-core/src/config/resolve.rs` — update existing tests if needed (`test_build_full_args_unified`, `test_build_full_args_ctx_override`, `test_build_full_args_no_sampling`, `test_build_full_args_no_quants`)

**What to implement:**

### 2b.1 — Rewrite `Config::build_full_args`

Replace `Config::build_full_args` (currently lines 186–270 of `resolve.rs`) with:

```rust
    /// Build the full argument list for a model, including model config args
    /// (`-m`, `-c`, `-ngl`) and sampling. Returns **flat tokens** suitable for
    /// `Command::args`.
    ///
    /// Merging order:
    /// 1. `backend.default_args`
    /// 2. `server.args`     (replaces same-flag entries from #1)
    /// 3. Injected `-m`/`-c`/`-ngl` (only if not already present after #1+#2)
    /// 4. `server.sampling.to_args()` (replaces same-flag entries from #1+#2+#3)
    ///
    /// **Invariant:** the returned `Vec<String>` is always flat (one token
    /// per element). Callers like `proxy/lifecycle.rs::override_arg` and
    /// `bench/runner.rs::_override_arg` depend on this. The final
    /// `flatten_args` call enforces it; the `debug_assert!` makes accidental
    /// regressions visible in test/debug builds.
    pub fn build_full_args(
        &self,
        server: &ModelConfig,
        backend: &BackendConfig,
        ctx_override: Option<u32>,
    ) -> Result<Vec<String>> {
        let mut grouped = crate::config::merge_args(&backend.default_args, &server.args);

        // Inject -m from model card, only if not already present.
        if let (Some(ref model_id), Some(ref quant_name)) = (&server.model, &server.quant) {
            if let Some(quant_entry) = server.quants.get(quant_name.as_str()) {
                let models_dir = self.models_dir()?;
                let model_path = models_dir.join(model_id).join(&quant_entry.file);
                let already_has_m = grouped.iter().any(|e| {
                    matches!(crate::config::flag_name(e), Some("-m") | Some("--model"))
                });
                if !already_has_m {
                    let path_str = model_path.to_string_lossy();
                    let quoted = crate::config::quote_value(&path_str);
                    grouped.push(format!("-m {}", quoted));
                }
            } else {
                tracing::warn!(
                    "Quant '{}' not found in ModelConfig for model '{}'",
                    quant_name,
                    model_id
                );
            }
        }

        // Inject -c (context length) only if not already present.
        let ctx = ctx_override.or(server.context_length).or_else(|| {
            server
                .quant
                .as_ref()
                .and_then(|q| server.quants.get(q).and_then(|qe| qe.context_length))
        });
        if let Some(ctx) = ctx {
            let already_has_c = grouped.iter().any(|e| {
                matches!(crate::config::flag_name(e), Some("-c") | Some("--ctx-size"))
            });
            if !already_has_c {
                grouped.push(format!("-c {}", ctx));
            }
        }

        // Inject -ngl only if not already present.
        if let Some(ngl) = server.gpu_layers {
            let already_has_ngl = grouped.iter().any(|e| {
                matches!(
                    crate::config::flag_name(e),
                    Some("-ngl") | Some("--n-gpu-layers")
                )
            });
            if !already_has_ngl {
                grouped.push(format!("-ngl {}", ngl));
            }
        }

        // Sampling: each sampling flag fully replaces the same flag in
        // anything injected so far.
        if let Some(sampling) = &server.sampling {
            if !sampling.is_empty() {
                grouped = crate::config::merge_args(&grouped, &sampling.to_args());
            }
        }

        let flat = crate::config::flatten_args(&grouped);
        // INVARIANT: build_full_args returns flat tokens. Callers like
        // proxy/lifecycle.rs::override_arg depend on this. The check
        // catches the failure mode where a *grouped* entry (e.g.
        // "-b 4096") leaks through unflattened: such an element starts
        // with '-' AND contains whitespace. Legitimate value-side tokens
        // like "system: hi" or "/path with space/m.gguf" contain
        // whitespace but do NOT start with '-', so they pass.
        debug_assert!(
            flat.iter().all(|t| !(t.starts_with('-') && t.contains(char::is_whitespace))),
            "build_full_args invariant violated: element looks like a grouped entry (flag + space + value): {:?}",
            flat
        );
        Ok(flat)
    }
```

### 2b.2 — Update existing tests in `resolve.rs`

Find the existing tests `test_build_full_args_unified`, `test_build_full_args_ctx_override`, `test_build_full_args_no_sampling`, and `test_build_full_args_no_quants` (currently at lines 543–774). Their current assertions look like:
```rust
assert!(args.contains(&"-c".to_string()));
assert!(args.contains(&"4096".to_string()));
```
These should **continue to pass unchanged** because `flatten_args` produces the same flat tokens. Run the tests after the rewrite and only change assertions if they actually fail.

The one place where the existing assertions reference sampling is `test_build_full_args_unified`:
```rust
assert!(args.contains(&"--temp".to_string()));
assert!(args.contains(&"0.30".to_string()));
```
This should also continue to pass because sampling args go through `merge_args` then `flatten_args`, which yields the same flat tokens.

### 2b.3 — Add a regression test for backend↔model dedup in `build_full_args`

Add this test alongside the existing `build_full_args` tests in `resolve.rs`:

```rust
    #[test]
    fn build_full_args_dedupes_backend_vs_model_flags() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model-Q4_K_M.gguf");
        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy gguf content").expect("Failed to write model file");

        let mut quants = std::collections::BTreeMap::new();
        quants.insert(
            "Q4_K_M".to_string(),
            crate::config::types::QuantEntry {
                file: "model-Q4_K_M.gguf".to_string(),
                size_bytes: None,
                context_length: None,
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec!["-b 4096".to_string(), "-ub 4096".to_string()],
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4_K_M".to_string()),
            port: None,
            health_check: None,
            enabled: true,
            context_length: Some(4096),
            profile: None,
            display_name: None,
            gpu_layers: Some(99),
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![
                "-b 2048".to_string(),
                "-ub 512".to_string(),
                "-t 14".to_string(),
            ],
            health_check_url: None,
            version: None,
        };

        let args = config
            .build_full_args(&server, &backend, None)
            .expect("build_full_args failed");

        // -t 14 must survive from backend defaults
        assert!(
            args.windows(2).any(|w| w == ["-t", "14"]),
            "expected -t 14 in args, got {:?}",
            args
        );
        // -b appears exactly once with value 4096
        let b_count = args.iter().filter(|t| *t == "-b").count();
        assert_eq!(b_count, 1, "expected exactly one -b token, got {:?}", args);
        assert!(args.windows(2).any(|w| w == ["-b", "4096"]));
        // -ub appears exactly once with value 4096
        let ub_count = args.iter().filter(|t| *t == "-ub").count();
        assert_eq!(ub_count, 1, "expected exactly one -ub token, got {:?}", args);
        assert!(args.windows(2).any(|w| w == ["-ub", "4096"]));
        // No 2048 or 512 anywhere
        assert!(!args.iter().any(|t| t == "2048"));
        assert!(!args.iter().any(|t| t == "512"));
    }

    #[test]
    fn build_full_args_returns_flat_tokens_with_quoted_path() {
        // Path with spaces must round-trip through grouped → flat correctly.
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let models_dir = temp_dir.path().join("models with space");
        let org_dir = models_dir.join("org").join("repo");
        let quant_file = org_dir.join("model.gguf");
        std::fs::create_dir_all(&org_dir).expect("Failed to create model dir");
        std::fs::write(&quant_file, b"dummy").expect("Failed to write model file");

        let mut quants = std::collections::BTreeMap::new();
        quants.insert(
            "Q4".to_string(),
            crate::config::types::QuantEntry {
                file: "model.gguf".to_string(),
                size_bytes: None,
                context_length: None,
            },
        );

        let mut config = Config::default();
        config.general.models_dir = Some(models_dir.to_string_lossy().to_string());
        config.loaded_from = Some(temp_dir.path().to_path_buf());

        let server = ModelConfig {
            backend: "llama_cpp".to_string(),
            args: vec![],
            sampling: None,
            model: Some("org/repo".to_string()),
            quant: Some("Q4".to_string()),
            port: None,
            health_check: None,
            enabled: true,
            context_length: None,
            profile: None,
            display_name: None,
            gpu_layers: None,
            quants,
        };

        let backend = BackendConfig {
            path: None,
            default_args: vec![],
            health_check_url: None,
            version: None,
        };

        let args = config
            .build_full_args(&server, &backend, None)
            .expect("build_full_args failed");

        // -m and the path must appear as adjacent flat tokens, with the
        // space-containing path preserved as a single token.
        let m_pos = args.iter().position(|t| t == "-m").expect("-m not found");
        let path_token = &args[m_pos + 1];
        assert!(
            path_token.contains("models with space"),
            "expected path with spaces preserved as a single token, got {:?}",
            path_token
        );
        assert!(path_token.ends_with("model.gguf"));
    }
```

**Steps:**
- [ ] Read `crates/koji-core/src/config/resolve.rs` lines 186–270 to understand the current `build_full_args`.
- [ ] Add the two new tests `build_full_args_dedupes_backend_vs_model_flags` and `build_full_args_returns_flat_tokens_with_quoted_path` to the existing `mod tests` block in `resolve.rs`.
- [ ] Run `cargo test -p koji-core --lib -- config::resolve::tests::build_full_args_dedupes_backend_vs_model_flags`
  - Did it fail (the current implementation has duplicate `-b` tokens)? If not, stop and investigate.
- [ ] Replace `Config::build_full_args` in `resolve.rs` with the implementation from section 2b.1.
- [ ] Run `cargo test -p koji-core --lib -- config::resolve::tests`
  - Did all `resolve::tests` pass, including the two new ones AND the existing `test_build_full_args_unified`, `test_build_full_args_ctx_override`, `test_build_full_args_no_sampling`, `test_build_full_args_no_quants`? If any of the existing tests fail, investigate whether the failure is due to `flatten_args` ordering or sampling format and fix accordingly.
- [ ] Run `cargo test -p koji-core --lib`
  - Did the full koji-core suite pass?
- [ ] Run `cargo test --workspace`
  - Did the full workspace test suite pass? (This catches any incidental regressions in `bench/runner.rs` or `proxy/lifecycle.rs` that depend on `build_full_args`.)
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Did it succeed? Fix any warnings.
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo fmt --all -- --check`
- [ ] Commit with message: `fix(config): dedupe backend↔model flags in build_full_args`

**Acceptance criteria:**
- [ ] `Config::build_full_args` uses `merge_args` for backend↔model dedup.
- [ ] `Config::build_full_args` injects `-m`/`-c`/`-ngl` using `flag_name`-based existence checks (not raw string equality).
- [ ] `Config::build_full_args` uses `quote_value` for the model path so paths with spaces round-trip.
- [ ] `Config::build_full_args` returns flat tokens (verified by the `debug_assert!` and by `build_full_args_returns_flat_tokens_with_quoted_path`).
- [ ] `build_full_args_dedupes_backend_vs_model_flags` passes: backend `[-b 2048, -ub 512, -t 14]` + model `[-b 4096, -ub 4096]` → flat output has exactly one `-b 4096`, one `-ub 4096`, `-t 14` survives, no `2048` or `512` tokens.
- [ ] `build_full_args_returns_flat_tokens_with_quoted_path` passes: a `models_dir` containing a space produces a single flat token for the path.
- [ ] All existing `test_build_full_args_*` tests still pass.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` passes.

---

## Task 3: Auto-migrate legacy flat args on load and persist to disk

**Context:**
Existing user configs have flat args like `[-fa, 1, -b, 2048, ...]`. After this task, the loader detects flat form, converts it to grouped form via `group_legacy_flat_args`, logs a one-time info message, and writes the migrated config back to disk **only if** something actually changed (so we don't churn already-grouped files). The `Config::default()` impl in `loader.rs` is also updated to use grouped form so freshly-created configs are correct from the start. A `Config::default()` round-trip test pins the invariant that defaults pass through `normalize_grouped_args` unchanged.

**Files:**
- Modify: `crates/koji-core/src/config/loader.rs` — add `normalize_grouped_args` helper, call it in `load_from`, save on change, update the `Default` impl
- Add tests: `crates/koji-core/src/config/loader.rs` (new `mod tests` if absent, or extend if present)

**What to implement:**

### 3.1 — Add `normalize_grouped_args` helper in `loader.rs`

Add this private function near the top of `loader.rs` (just after the imports):

```rust
/// Normalize all `default_args` and `args` lists in the config from
/// legacy flat form to grouped form. Returns `true` if anything changed.
fn normalize_grouped_args(config: &mut Config) -> bool {
    use crate::config::group_legacy_flat_args;
    let mut changed = false;
    for backend in config.backends.values_mut() {
        let (migrated, did) = group_legacy_flat_args(&backend.default_args);
        if did {
            backend.default_args = migrated;
            changed = true;
        }
    }
    for model in config.models.values_mut() {
        let (migrated, did) = group_legacy_flat_args(&model.args);
        if did {
            model.args = migrated;
            changed = true;
        }
    }
    changed
}
```

### 3.2 — Call it in `Config::load_from`

In `loader.rs`, find `Config::load_from` (currently around lines 58–83). After the `let mut config = ...` block but before `migrate_cards_to_unified_config(&mut config)?;`, insert:

```rust
        // Migrate legacy flat args to grouped form. If anything changed,
        // persist the migrated config back to disk so the next load is a
        // no-op and `koji status` shows the new format.
        let args_migrated = normalize_grouped_args(&mut config);
        if args_migrated {
            tracing::info!(
                "Migrated legacy flat args to grouped form in {}",
                config_path.display()
            );
        }
```

Then, **after** `migrate_cards_to_unified_config(&mut config)?;` and the `config.loaded_from = Some(...)` assignment, add:

```rust
        if args_migrated {
            // Best-effort save; if it fails (e.g. read-only filesystem),
            // log a warning but do not fail the load.
            if let Err(e) = config.save_to(config_dir) {
                tracing::warn!("Failed to persist migrated args to {}: {}", config_path.display(), e);
            }
        }
```

(Note: read the actual current order of operations in `load_from` and place these blocks at the right point. The principle is: migrate before any other transformation, then save once at the end if migration changed something.)

### 3.3 — Update the `Default` impl in `loader.rs`

Find the `Default for Config` impl (lines 131–252 of `loader.rs`). Locate the `models.insert("default".to_string(), ModelConfig { ... args: vec!["--host", "0.0.0.0", "-m", "path/to/model.gguf", "-ngl", "999", "-fa", "1", "-c", "8192"].into_iter().map(String::from).collect(), ... })` and replace the `args` field with grouped form:

```rust
                args: vec![
                    "--host 0.0.0.0",
                    "-m path/to/model.gguf",
                    "-ngl 999",
                    "-fa 1",
                    "-c 8192",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
```

### 3.4 — Tests

Add a test module to `loader.rs` (or extend an existing one). Add these tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn normalize_migrates_flat_backend_default_args() {
        let mut config = Config::default();
        config.backends.insert(
            "flat_backend".to_string(),
            BackendConfig {
                path: None,
                default_args: vec![
                    "-fa".to_string(),
                    "1".to_string(),
                    "-b".to_string(),
                    "2048".to_string(),
                    "--mlock".to_string(),
                ],
                health_check_url: None,
                version: None,
            },
        );

        let changed = normalize_grouped_args(&mut config);
        assert!(changed);

        let migrated = &config.backends["flat_backend"].default_args;
        assert_eq!(
            migrated,
            &vec!["-fa 1".to_string(), "-b 2048".to_string(), "--mlock".to_string()]
        );
    }

    #[test]
    fn normalize_migrates_flat_model_args() {
        let mut config = Config::default();
        config.models.insert(
            "flat_model".to_string(),
            ModelConfig {
                backend: "llama_cpp".to_string(),
                args: vec![
                    "-ngl".to_string(),
                    "999".to_string(),
                    "-c".to_string(),
                    "8192".to_string(),
                ],
                sampling: None,
                model: None,
                quant: None,
                port: None,
                health_check: None,
                enabled: true,
                context_length: None,
                profile: None,
                display_name: None,
                gpu_layers: None,
                quants: BTreeMap::new(),
            },
        );

        let changed = normalize_grouped_args(&mut config);
        assert!(changed);

        let migrated = &config.models["flat_model"].args;
        assert_eq!(migrated, &vec!["-ngl 999".to_string(), "-c 8192".to_string()]);
    }

    #[test]
    fn normalize_default_config_is_noop() {
        // Config::default() must already be in grouped form, so calling
        // normalize_grouped_args on it should not change anything. We
        // compare only the args/default_args fields rather than whole
        // structs because BackendConfig/ModelConfig/QuantEntry don't
        // currently derive PartialEq, and adding those derives is
        // out-of-scope for this PR.
        let mut config = Config::default();

        // Snapshot the args fields before normalization. Use BTreeMap to
        // get deterministic ordering for the comparison.
        let before_backend_args: std::collections::BTreeMap<String, Vec<String>> = config
            .backends
            .iter()
            .map(|(k, b)| (k.clone(), b.default_args.clone()))
            .collect();
        let before_model_args: std::collections::BTreeMap<String, Vec<String>> = config
            .models
            .iter()
            .map(|(k, m)| (k.clone(), m.args.clone()))
            .collect();

        let changed = normalize_grouped_args(&mut config);
        assert!(!changed, "Config::default() must already be in grouped form");

        let after_backend_args: std::collections::BTreeMap<String, Vec<String>> = config
            .backends
            .iter()
            .map(|(k, b)| (k.clone(), b.default_args.clone()))
            .collect();
        let after_model_args: std::collections::BTreeMap<String, Vec<String>> = config
            .models
            .iter()
            .map(|(k, m)| (k.clone(), m.args.clone()))
            .collect();

        assert_eq!(
            before_backend_args, after_backend_args,
            "default backend default_args drifted"
        );
        assert_eq!(
            before_model_args, after_model_args,
            "default model args drifted"
        );
    }

    #[test]
    fn normalize_already_grouped_is_noop() {
        let mut config = Config::default();
        config.backends.insert(
            "grouped".to_string(),
            BackendConfig {
                path: None,
                default_args: vec!["-fa 1".to_string(), "-b 2048".to_string()],
                health_check_url: None,
                version: None,
            },
        );

        let changed = normalize_grouped_args(&mut config);
        assert!(!changed);
    }
}
```

Note: `Config::default()` already has `models["default"]` populated with the args field. After 3.3, that field is in grouped form, so `normalize_default_config_is_noop` becomes meaningful. The test deliberately compares only `args` / `default_args` `Vec<String>`s (via `BTreeMap` snapshots) rather than whole `ModelConfig`/`BackendConfig` structs, because those structs don't derive `PartialEq` and adding derives is out-of-scope for this PR.

**Steps:**
- [ ] Read `crates/koji-core/src/config/loader.rs` to find the current structure of `load_from` and `Default for Config`.
- [ ] Add the `normalize_grouped_args` helper function from section 3.1 to `loader.rs`.
- [ ] Add the four tests from section 3.4 to a `#[cfg(test)] mod tests` block in `loader.rs`.
- [ ] Run `cargo test -p koji-core --lib -- config::loader::tests::normalize_default_config_is_noop`
  - Did it fail? It should fail because `Default` still uses flat form. If it passes, investigate.
- [ ] Update the `Default for Config` impl in `loader.rs` (section 3.3) to use grouped form.
- [ ] Run `cargo test -p koji-core --lib -- config::loader::tests`
  - Did all four tests pass? If not, fix.
- [ ] Modify `Config::load_from` in `loader.rs` to call `normalize_grouped_args` and save-on-change (section 3.2).
- [ ] Run `cargo test -p koji-core --lib`
  - Did the full koji-core suite pass? If any other test depends on the old default, update its assertions.
- [ ] Manually verify the save-on-migrate path: create a temp file with flat args, call `Config::load_from` on its parent dir, then read the file again and confirm it now contains grouped form. (You can write this as an integration-style test inside `loader.rs` using `tempfile::tempdir`.) Add this test:

```rust
    #[test]
    fn load_from_persists_migration_to_disk() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        let legacy_toml = r#"
[general]
log_level = "info"

[backends.llama_cpp]
default_args = ["-fa", "1", "-b", "2048"]

[models.test]
backend = "llama_cpp"
args = ["-ngl", "999"]
enabled = true
"#;
        std::fs::write(&config_path, legacy_toml).expect("write");

        let _config = Config::load_from(temp_dir.path()).expect("load");

        let after = std::fs::read_to_string(&config_path).expect("read after");
        // After load, the file on disk must contain grouped form.
        assert!(after.contains("\"-fa 1\""), "expected grouped -fa 1 in {}", after);
        assert!(after.contains("\"-b 2048\""), "expected grouped -b 2048 in {}", after);
        assert!(after.contains("\"-ngl 999\""), "expected grouped -ngl 999 in {}", after);
        // The flat tokens must NOT remain.
        assert!(!after.contains("\"-fa\","), "flat -fa, leaked: {}", after);
    }

    #[test]
    fn load_from_already_grouped_does_not_rewrite() {
        // Pin the "don't churn already-grouped configs" invariant: if the
        // file on disk is already in grouped form, Config::load_from must
        // NOT rewrite it. We verify by snapshotting the byte content
        // before and after.
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_path = temp_dir.path().join("config.toml");
        let grouped_toml = r#"
[general]
log_level = "info"

[backends.llama_cpp]
default_args = ["-fa 1", "-b 2048"]

[models.test]
backend = "llama_cpp"
args = ["-ngl 999"]
enabled = true
"#;
        std::fs::write(&config_path, grouped_toml).expect("write");
        let before = std::fs::read_to_string(&config_path).expect("read before");

        let _config = Config::load_from(temp_dir.path()).expect("load");

        let after = std::fs::read_to_string(&config_path).expect("read after");
        assert_eq!(
            before, after,
            "already-grouped config was rewritten unnecessarily.\nBefore:\n{}\nAfter:\n{}",
            before, after
        );
    }
```
- [ ] Run `cargo test -p koji-core --lib -- config::loader::tests::load_from_persists_migration_to_disk`
  - Did it pass? If not, the save path is wrong; fix.
- [ ] Run `cargo test -p koji-core --lib -- config::loader::tests::load_from_already_grouped_does_not_rewrite`
  - Did it pass? If not, the save-on-change guard is incorrectly triggering on no-op migrations; check `normalize_grouped_args` returns `false` for already-grouped input.
- [ ] Run `cargo test --workspace`
  - Did the full workspace test suite pass?
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo fmt --all -- --check`
- [ ] Commit with message: `feat(config): auto-migrate legacy flat args to grouped form on load and persist`

**Acceptance criteria:**
- [ ] `normalize_grouped_args` exists in `loader.rs` and migrates both `backends.*.default_args` and `models.*.args`.
- [ ] `Config::load_from` calls `normalize_grouped_args` and writes the migrated config back to disk via `save_to` if anything changed.
- [ ] When migration occurs, a `tracing::info!` is emitted naming the file path.
- [ ] `Config::default()` produces grouped-form args.
- [ ] `normalize_default_config_is_noop` passes — proves `Default` is in grouped form.
- [ ] `normalize_migrates_flat_backend_default_args` and `normalize_migrates_flat_model_args` pass.
- [ ] `load_from_persists_migration_to_disk` passes — proves the on-disk file is rewritten when migration is needed.
- [ ] `load_from_already_grouped_does_not_rewrite` passes — proves already-grouped configs are NOT churned (byte-equality before/after).
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --workspace -- -D warnings` passes.

---

## Task 4: Update `service.rs` fallback, web UI hint and placeholder

**Context:**
The remaining loose ends are:
1. `crates/koji-cli/src/service.rs` lines 307–314 has a fallback path that does naive `args.extend(srv.args.clone())` if `build_full_args` fails. This fallback must use the new `merge_args` + `flatten_args` so it produces a deduped flat list consistent with the success path. It also needs to import the helpers via `koji_core::config::{merge_args, flatten_args}` (which Task 1's `pub use` makes available).
2. `crates/koji-web/src/pages/model_editor.rs` line 1186 (`placeholder`) and line 1190 (`form-hint`) currently disagree with each other and with the new grouped format. Both must be updated to describe one-flag-per-line.

This task includes a tiny compile-only test for the service.rs fallback to make sure the import paths work.

**Files:**
- Modify: `crates/koji-cli/src/service.rs` (lines 307–314)
- Modify: `crates/koji-web/src/pages/model_editor.rs` (lines 1186 and 1190)

**What to implement:**

### 4.1 — `crates/koji-cli/src/service.rs`

Find the `unwrap_or_else` block at lines 307–314:

```rust
            let args = config
                .build_full_args(srv, backend, ctx)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to build model args: {}", e);
                    let mut args = backend.default_args.clone();
                    args.extend(srv.args.clone());
                    args
                });
```

Replace with:

```rust
            let args = config
                .build_full_args(srv, backend, ctx)
                .unwrap_or_else(|e| {
                    tracing::warn!("Failed to build model args: {}", e);
                    koji_core::config::flatten_args(&koji_core::config::merge_args(
                        &backend.default_args,
                        &srv.args,
                    ))
                });
```

(Use the fully-qualified `koji_core::config::...` path. `service.rs` already has `use koji_core::config::Config;` at line 26, so the helpers can also be imported as `use koji_core::config::{flatten_args, merge_args};` at the top of the file if you prefer — either approach is fine, but the FQN is more obviously correct on first read.)

### 4.2 — `crates/koji-web/src/pages/model_editor.rs`

Find line 1186 and 1190:

```rust
                                placeholder="One flag per line, e.g.:\n-ctk\nq4_0"
                                ...
                            <span class="form-hint">"One argument per line (same as TOML args array)"</span>
```

Replace with:

```rust
                                placeholder="One flag per line, e.g.:\n-fa 1\n-b 4096\n--mlock"
                                ...
                            <span class="form-hint">"One flag per line, e.g. -fa 1, --mlock, or -b 4096. Quote values containing spaces: -m \"path with space/m.gguf\""</span>
```

**Steps:**
- [ ] Read `crates/koji-cli/src/service.rs` lines 295–320 to confirm the current fallback shape and surrounding context (binding name `srv` vs `server`, etc.).
- [ ] Apply the replacement from section 4.1. Use the exact binding names (`srv`, `backend`) as they appear in the current code.
- [ ] Run `cargo check -p koji-cli`
  - Did it succeed? If not, the import path or function name is wrong; fix and re-check.
- [ ] Run `cargo build -p koji-cli`
  - Did it succeed?
- [ ] Read `crates/koji-web/src/pages/model_editor.rs` lines 1180–1195 to confirm the current placeholder and hint.
- [ ] Apply the replacement from section 4.2.
- [ ] Run `make build-frontend-dev` (this runs `trunk build` which is the correct Leptos/wasm build command).
  - Did it succeed? If not, fix any syntax errors.
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo clippy --package koji-web --features ssr -- -D warnings`
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo fmt --all -- --check`
- [ ] Run `cargo test --workspace` as a final smoke test that nothing else broke.
- [ ] Commit with message: `chore(cli,web): use grouped-args helpers in service fallback and update model editor hint`

**Acceptance criteria:**
- [ ] `service.rs`'s `build_full_args` fallback uses `koji_core::config::flatten_args` + `koji_core::config::merge_args`.
- [ ] `cargo check -p koji-cli` succeeds (proves the `pub use` from Task 1 makes the helpers reachable).
- [ ] `model_editor.rs` placeholder and hint both describe one-flag-per-line and mention quoting for paths with spaces.
- [ ] `make build-frontend-dev` (i.e. `trunk build`) succeeds.
- [ ] `cargo clippy --workspace -- -D warnings` passes.
- [ ] `cargo clippy --package koji-web --features ssr -- -D warnings` passes.

---

## Final Verification Checklist (after all four tasks)

Run each step manually after Task 4 commits:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo clippy --package koji-web --features ssr -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `make build-frontend-dev` (verifies the Leptos frontend still builds)

**Manual smoke test (requires a real koji install):**

1. Backup your existing `~/.config/koji/config.toml` (or `%APPDATA%\koji\config.toml` on Windows).
2. Edit it so a backend has `default_args = ["-fa", "1", "-b", "2048"]` (flat) and a model has `args = ["-ngl", "999"]` (flat).
3. Run any command that calls `Config::load`, e.g. `koji status`.
4. Inspect the file: it should now contain `default_args = ["-fa 1", "-b 2048"]` and `args = ["-ngl 999"]`.
5. Check the logs: a `tracing::info!` line about migration should appear.
6. Set the same backend's `default_args = ["-b 2048", "-ub 512"]` and a model's `args = ["-b 4096", "-ub 4096"]`.
7. Run `koji run <model>` and inspect the `Executing backend:` log line: it must contain exactly one `-b 4096`, one `-ub 4096`, and no `2048`/`512`.

**Windows path-with-spaces verification:**

1. In `models_dir = "C:/Users/Test/koji models"`, install a model.
2. Run `koji run` against it and confirm the spawned command shows `-m "C:/Users/Test/koji models/.../model.gguf"` (or equivalently, `koji models/.../model.gguf` as a single argv element).
3. The backend should successfully load the model (no "file not found" because the path was split mid-token).

---

## Summary of Changes

| Component | Before | After |
|---|---|---|
| On-disk format | `["-fa", "1", "-b", "2048"]` (flat, one token per line) | `["-fa 1", "-b 2048"]` (grouped, one flag per line) |
| Backend↔model dedup | None — duplicates passed to backend | Model flags fully replace backend flags |
| Sampling args | Flat `["--temp", "0.30", ...]` from `to_args()` | Grouped `["--temp 0.30", ...]` |
| Path quoting | Paths-with-spaces worked accidentally (each token preserved by Vec) | Paths shlex-quoted in grouped form, round-trip via `flatten_args` |
| Migration | N/A | Auto-migrate on load + persist to disk + log info |
| `build_full_args` contract | Flat tokens (informal) | Flat tokens (enforced by `debug_assert!`) |
| Web hint | "One argument per line" (lying — was actually one token per line) | "One flag per line, e.g. `-fa 1` …" |

## Out of scope

- Changing the on-disk *type* of `args` from `Vec<String>` to a typed enum. Not done because the migration is sticky and the string-based form is sufficient.
- Aliasing short and long flag forms (`-c` ↔ `--ctx-size`). Users typically pick one form; aliasing would couple koji to llama.cpp's flag taxonomy.
- Touching `proxy/process.rs::override_arg`, `bench/runner.rs::_override_arg`, `proxy/lifecycle.rs`, `bench/runner.rs` callers — they continue to work because `build_full_args` still returns flat tokens (invariant pinned by `debug_assert!` in Task 2b).
- Backwards-compatibility fallback if a user downgrades koji after migration. Document this in the PR description as a known one-way migration.
