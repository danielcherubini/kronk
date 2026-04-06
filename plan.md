# Implementation Plan

## Goal
Add a unit test `test_collect_model_statuses_reports_idle_when_no_runtime_entry` to `crates/koji-core/src/proxy/status.rs` that verifies `ProxyState::collect_model_statuses()` returns one entry per configured model, each marked `loaded: false`, sorted by `id`, with the configured `backend` field, when `state.models` is empty.

## Relevant Code (read first)

- `crates/koji-core/src/proxy/status.rs` — target file. Currently has **no** `#[cfg(test)]` module; we must add one from scratch. The `collect_model_statuses` impl (lines 9–36) iterates `config.models`, uses `resolve_servers_for_model(model_id)` to find server entries, then checks `runtime.get(&server_name)` for `is_ready()`. When `runtime` is empty, `.any(...)` evaluates to `false`, so each entry is pushed with `loaded: false`. Finally the vector is sorted by `id` ascending. This is the exact branch we're testing.
- `crates/koji-core/src/proxy/state.rs` — defines `ProxyState::new(config, db_dir)` (lines 7–33). Creates `models` as an empty `RwLock<HashMap<String, ModelState>>`. Also contains the only existing test in the neighboring file: `test_proxy_state_new_creates_metrics_channel` (good reference for how to set up a `ProxyState` in tests).
- `crates/koji-core/src/proxy/types.rs` — defines `ProxyState` fields. `models: Arc<RwLock<HashMap<String, ModelState>>>` — we can assert emptiness via `state.models.read().await.is_empty()`.
- `crates/koji-core/src/config/types.rs` — `Config` is `Default`, `ModelConfig` is **NOT** `Default`; all fields must be provided. Re-exported at `crate::config::{Config, ModelConfig}` via `crates/koji-core/src/config/mod.rs`.
- `crates/koji-core/src/proxy/mod.rs` (lines 105–125) — canonical example of constructing a `ModelConfig` literal in tests. Copy this shape verbatim.
- `crates/koji-core/src/gpu.rs` (lines 80–86) — `ModelStatus { id: String, backend: String, loaded: bool }` is the returned struct.
- `crates/koji-core/src/config/resolve.rs` (lines 28–49) — `resolve_servers_for_model` **skips disabled models** and **skips models whose `backend` key isn't in `config.backends`**. For this test both conditions are fine because:
  - We want `loaded == false`, which is what happens when `resolve_servers_for_model` returns an empty list (or when the entries it returns have no matching runtime state). Either way the assertion holds.
  - The `backend` field on `ModelStatus` comes from `model_cfg.backend.clone()` **inside the outer loop**, not from `resolve_servers_for_model`, so it is populated even if no backends map is configured. No need to populate `config.backends`.

## Tasks

1. **Add test module with the new test** to `crates/koji-core/src/proxy/status.rs`.
   - File: `crates/koji-core/src/proxy/status.rs`
   - Changes: Append a new `#[cfg(test)] mod tests { ... }` block at the bottom of the file. Inside:
     - `use super::*;`
     - `use crate::config::{Config, ModelConfig};`
     - `use std::collections::BTreeMap;`
     - Small helper `fn make_model_config(backend: &str) -> ModelConfig` that returns a `ModelConfig` with all fields explicit (`enabled: true`, everything else `None`/empty) — mirror the literal in `proxy/mod.rs` lines 107–121.
     - The test `#[tokio::test] async fn test_collect_model_statuses_reports_idle_when_no_runtime_entry()`:
       1. Build `let mut config = Config::default();`
       2. Insert **three** models (satisfies "at least two" and makes the sort assertion meaningful because HashMap iteration order is unspecified):
          - `"zebra"` → `make_model_config("llama_cpp")`
          - `"alpha"` → `make_model_config("mlx")`
          - `"mango"` → `make_model_config("vllm")`
          Each uses a distinct backend name so the "backend field matches the configured backend name" assertion is non-trivial.
       3. `let state = ProxyState::new(config, None);`
       4. Sanity-check that `state.models` is empty: `assert!(state.models.read().await.is_empty());`
       5. `let statuses = state.collect_model_statuses().await;`
       6. Assertions:
          - Length equals configured count: `assert_eq!(statuses.len(), 3);`
          - Every entry `loaded == false`: `assert!(statuses.iter().all(|s| !s.loaded));`
          - Sorted by id ascending:
            ```rust
            let ids: Vec<&str> = statuses.iter().map(|s| s.id.as_str()).collect();
            assert_eq!(ids, vec!["alpha", "mango", "zebra"]);
            ```
          - Backend matches configured backend per entry:
            ```rust
            assert_eq!(statuses[0].backend, "mlx");       // alpha
            assert_eq!(statuses[1].backend, "vllm");      // mango
            assert_eq!(statuses[2].backend, "llama_cpp"); // zebra
            ```
     - Add a `///` doc comment on the test explaining the scenario (per AGENTS.md conventions).
   - Acceptance: `cargo test --package koji-core proxy::status::tests::test_collect_model_statuses_reports_idle_when_no_runtime_entry` passes.

2. **TDD sanity check**: Before adding the test, briefly eyeball the current `collect_model_statuses` (lines 9–36) to confirm it already handles the empty-runtime case — it does (`.any(...)` on empty iterator returns `false`). So the test should pass on first run. If it doesn't, stop and diagnose; do **not** modify `collect_model_statuses` as part of this task.
   - Acceptance: Test passes without touching the implementation.

3. **Format, build, test** — each step independently, waiting for completion:
   1. `cargo fmt --all` — format the new code.
   2. `cargo build --package koji-core` — ensure it compiles. (Workspace build is also fine.)
   3. `cargo test --package koji-core proxy::status::tests::test_collect_model_statuses_reports_idle_when_no_runtime_entry -- --nocapture` — run the new test specifically.
   4. `cargo test --package koji-core` — run the full koji-core suite to make sure nothing else regressed.
   - Acceptance: all four steps exit cleanly.

4. **Commit** with message:
   `test(proxy): cover collect_model_statuses idle case when state.models is empty`
   - Files: `crates/koji-core/src/proxy/status.rs`
   - Acceptance: `git log -1` shows the new commit touching only `status.rs`.

## Files to Modify
- `crates/koji-core/src/proxy/status.rs` — append a `#[cfg(test)] mod tests { ... }` block with one helper and one `#[tokio::test]`.

## New Files
None.

## Dependencies
- Task 2 and 3 depend on Task 1 (test must exist before building/running).
- Task 4 depends on Task 3 succeeding.

## Risks

- **`ModelConfig` has no `Default` impl.** All 14 fields must be specified in the literal. Use the existing canonical example in `crates/koji-core/src/proxy/mod.rs` lines 107–121 as the source of truth. If new fields are added to `ModelConfig` later, this literal will fail to compile and must be updated — acceptable and explicit.
- **HashMap iteration order.** `config.models` is a `HashMap`, so insertion order is *not* iteration order. This is actually why we use three models with non-alphabetical insertion — it forces the test to rely on the sort step in `collect_model_statuses`, rather than accidentally passing because the HashMap happened to yield them in order.
- **Backend resolution side effect.** `resolve_servers_for_model` filters by `config.backends` membership. We deliberately leave `config.backends` empty — that's fine because:
  - `loaded` becomes `false` either via empty resolution or via absent runtime entry; both paths yield the expected result.
  - The `backend` field on `ModelStatus` is set from `model_cfg.backend.clone()` in the outer loop, independent of `resolve_servers_for_model`.
  If we wanted to also exercise the "backend present + server resolvable" path, we would need to populate `config.backends` — but that's out of scope for Task 1, which only asks about the "no runtime entry" branch.
- **Test module placement.** `status.rs` currently has no `#[cfg(test)]` block; we add the first one. Place it at the very bottom of the file, after the final `}` of `impl ProxyState`, matching the convention used in `state.rs`.
- **Async test runtime.** Use `#[tokio::test]` (not `#[test]`) because `collect_model_statuses` is `async` and reads `tokio::sync::RwLock`. `tokio` with the `macros` + `rt` features is already a dev-dep of `koji-core` (other async tests like `test_rename_model_success` use `#[tokio::test]`), so no `Cargo.toml` changes needed.
- **Do not modify `collect_model_statuses`.** The task is test-only. If the test fails, fix the test — not the implementation — unless a genuine bug is found, in which case stop and report.
