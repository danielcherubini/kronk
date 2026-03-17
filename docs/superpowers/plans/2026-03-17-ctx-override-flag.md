# `--ctx` Context Size Override Flag Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `--ctx` CLI flag to `kronk run` (and `kronk service-run`) that overrides the context size passed to the backend, taking priority over the model card value.

**Architecture:** Add an `Option<u32>` `--ctx` argument to the `Run` and `ServiceRun` clap variants. Thread it through `cmd_run` into `build_full_args`. In `build_full_args`, if `ctx_override` is `Some`, force `-c <value>` regardless of what the model card says (replacing any existing `-c`/`--ctx-size` in the args). This is a pure CLI-layer change — no core library modifications needed.

**Tech Stack:** Rust, clap (derive), existing `build_full_args` in `crates/kronk-cli/src/main.rs`

---

## File Structure

| File | Responsibility | Changes |
|------|---------------|---------|
| `crates/kronk-cli/src/main.rs` | CLI entry point, arg parsing, `build_full_args`, `cmd_run` | Add `--ctx` to `Run`/`ServiceRun` variants, thread through `cmd_run`, apply override in `build_full_args` |

Only one file changes. The override is purely a CLI concern — the core library's `ModelCard::context_length_for` remains unchanged since the override happens at arg-assembly time.

---

## Task 1: Add `--ctx` Flag and Override Logic

### Step-by-step

- [ ] **Step 1: Add `ctx` field to the `Run` variant**

In `crates/kronk-cli/src/main.rs`, add the `--ctx` argument to the `Run` enum variant (line 27-30):

```rust
    /// Pull the lever! Run a profile in the foreground
    Run {
        #[arg(short, long, default_value = "default")]
        profile: String,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
    },
```

Also add it to the `ServiceRun` variant (line 36-41):

```rust
    /// Internal: called by Windows SCM (do not use directly)
    #[command(hide = true)]
    ServiceRun {
        #[arg(short, long)]
        profile: String,
        /// Override context size (e.g. 8192, 16384). Takes priority over model card value.
        #[arg(long)]
        ctx: Option<u32>,
    },
```

- [ ] **Step 2: Update the match arms to pass `ctx` through**

In `crates/kronk-cli/src/main.rs` at the dispatch match (line 278-281), update both arms:

```rust
            Commands::Run { profile, ctx } => cmd_run(&config, &profile, ctx).await,
            // ...
            Commands::ServiceRun { profile, ctx } => cmd_run(&config, &profile, ctx).await,
```

- [ ] **Step 3: Update `cmd_run` signature to accept `ctx_override`**

In `crates/kronk-cli/src/main.rs`, update `cmd_run` (line 625):

```rust
async fn cmd_run(config: &Config, profile_name: &str, ctx_override: Option<u32>) -> Result<()> {
```

And update the call to `build_full_args` (line 628):

```rust
    let args = build_full_args(config, profile, backend, ctx_override)?;
```

Also add it to the status banner (after line 633, before the health check line):

```rust
    if let Some(ctx) = ctx_override {
        println!("  Context:  {}", ctx);
    }
```

- [ ] **Step 4: Update other `build_full_args` call sites to pass `None`**

There are two other call sites for `build_full_args` that don't have a ctx override. Update them to pass `None`:

At line 495 (Windows service main):

```rust
        let args = build_full_args(&config, prof, backend, None).unwrap_or_else(|e| {
```

At line 705 (Linux service install):

```rust
                let args = build_full_args(config, prof, backend, None)?;
```

- [ ] **Step 5: Update `build_full_args` to accept and apply `ctx_override`**

In `crates/kronk-cli/src/main.rs`, update `build_full_args` signature (line 574):

```rust
fn build_full_args(
    config: &Config,
    profile: &kronk_core::config::ProfileConfig,
    backend: &kronk_core::config::BackendConfig,
    ctx_override: Option<u32>,
) -> Result<Vec<String>> {
```

Replace the context-length injection block (lines 593-597) with logic that respects the override:

```rust
            // Context size: CLI override > model card
            let ctx = ctx_override.or_else(|| installed.card.context_length_for(quant_name));
            if let Some(ctx) = ctx {
                // Remove any existing -c / --ctx-size from args before injecting
                let mut filtered = Vec::with_capacity(args.len());
                let mut skip_next = false;
                for arg in &args {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if arg == "-c" || arg == "--ctx-size" {
                        skip_next = true;
                        continue;
                    }
                    filtered.push(arg.clone());
                }
                args = filtered;
                args.push("-c".to_string());
                args.push(ctx.to_string());
            }
```

Also handle the no-model-card path. After the early return `Ok(args)` at line 613, before the "No model card" comment (line 617), add context override handling for profiles without model cards:

```rust
    // No model card — still apply ctx override if given
    if let Some(ctx) = ctx_override {
        // Remove any existing -c / --ctx-size from args
        let mut filtered = Vec::with_capacity(args.len());
        let mut skip_next = false;
        for arg in &args {
            if skip_next {
                skip_next = false;
                continue;
            }
            if arg == "-c" || arg == "--ctx-size" {
                skip_next = true;
                continue;
            }
            filtered.push(arg.clone());
        }
        args = filtered;
        args.push("-c".to_string());
        args.push(ctx.to_string());
    }
```

- [ ] **Step 6: Build to verify it compiles**

Run: `cargo build -p kronk-cli 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 7: Run existing tests to check nothing is broken**

Run: `cargo test --workspace 2>&1`
Expected: All existing tests pass. The `test_build_args_includes_sampling` test in `kronk-core` calls `config.build_args()` (not `build_full_args`), so it is unaffected.

- [ ] **Step 8: Commit**

```bash
git add crates/kronk-cli/src/main.rs
git commit -m "feat: add --ctx flag to kronk run for context size override"
```

---

## Task 2: Refactor `-c` Removal Into a Helper (DRY)

The "remove existing `-c`/`--ctx-size` and inject new value" logic appears twice in Task 1 (model-card path and no-model-card path). Extract it.

- [ ] **Step 1: Extract helper function**

Add this function above `build_full_args` in `crates/kronk-cli/src/main.rs`:

```rust
/// Replace or inject `-c <value>` in the argument list.
/// Removes any existing `-c` / `--ctx-size` and appends the new value.
fn inject_context_size(args: &mut Vec<String>, ctx: u32) {
    let mut filtered = Vec::with_capacity(args.len());
    let mut skip_next = false;
    for arg in args.iter() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "-c" || arg == "--ctx-size" {
            skip_next = true;
            continue;
        }
        filtered.push(arg.clone());
    }
    *args = filtered;
    args.push("-c".to_string());
    args.push(ctx.to_string());
}
```

- [ ] **Step 2: Use the helper in both code paths**

In the model-card path inside `build_full_args`, replace the inline removal logic with:

```rust
            // Context size: CLI override > model card
            let ctx = ctx_override.or_else(|| installed.card.context_length_for(quant_name));
            if let Some(ctx) = ctx {
                inject_context_size(&mut args, ctx);
            }
```

In the no-model-card path, replace the inline removal logic with:

```rust
    // No model card — still apply ctx override if given
    if let Some(ctx) = ctx_override {
        inject_context_size(&mut args, ctx);
    }
```

- [ ] **Step 3: Build to verify it compiles**

Run: `cargo build -p kronk-cli 2>&1`
Expected: Compiles with no errors.

- [ ] **Step 4: Run existing tests**

Run: `cargo test --workspace 2>&1`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-cli/src/main.rs
git commit -m "refactor: extract inject_context_size helper for DRY"
```

---

## Task 3: Add Tests

The `build_full_args` function lives in `kronk-cli` (not `kronk-core`), and currently has no unit tests. We'll add tests for the new override behavior. Since `build_full_args` requires a `Config` and model resolution, we'll test through the public `inject_context_size` helper and verify the `build_full_args` behavior via integration-style assertions.

- [ ] **Step 1: Write test for `inject_context_size` — replaces existing value**

Add to the bottom of `crates/kronk-cli/src/main.rs` (or in a `#[cfg(test)] mod tests` block if one exists — if not, create one):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inject_context_size_replaces_existing() {
        let mut args = vec![
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "-c".to_string(),
            "8192".to_string(),
            "-ngl".to_string(),
            "999".to_string(),
        ];
        inject_context_size(&mut args, 32768);
        assert!(!args.contains(&"8192".to_string()));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"32768".to_string()));
        // Other args preserved
        assert!(args.contains(&"--host".to_string()));
        assert!(args.contains(&"-ngl".to_string()));
    }

    #[test]
    fn test_inject_context_size_adds_when_missing() {
        let mut args = vec!["--host".to_string(), "0.0.0.0".to_string()];
        inject_context_size(&mut args, 16384);
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"16384".to_string()));
    }

    #[test]
    fn test_inject_context_size_replaces_long_form() {
        let mut args = vec![
            "--ctx-size".to_string(),
            "4096".to_string(),
            "-m".to_string(),
            "model.gguf".to_string(),
        ];
        inject_context_size(&mut args, 65536);
        assert!(!args.contains(&"--ctx-size".to_string()));
        assert!(!args.contains(&"4096".to_string()));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"65536".to_string()));
        // Model arg preserved
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"model.gguf".to_string()));
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test -p kronk-cli 2>&1`
Expected: All 3 new tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/kronk-cli/src/main.rs
git commit -m "test: add unit tests for inject_context_size helper"
```

---

## Task 4: Verify End-to-End CLI Parsing

- [ ] **Step 1: Build and check `--help` output**

Run: `cargo run -p kronk-cli -- run --help 2>&1`
Expected: Output includes `--ctx <CTX>` with the description "Override context size".

- [ ] **Step 2: Run the full test suite one final time**

Run: `cargo test --workspace 2>&1`
Expected: All tests pass, zero warnings related to our changes.

- [ ] **Step 3: Final commit if any cleanup was needed, otherwise done**

No commit needed if everything is clean.
