# Plan: Add n-min/n-max support for ngram-mod speculative decoding

## Intent

Extend tama's speculative decoding benchmark system to support ngram-mod's full 3-parameter interface (`n-match`, `n-min`, `n-max`), fix dead code in the CLI arg builders, and correct the misleading docstring. Currently ngram-mod only supports `n-match` (mapped via `spec_ngram_n`), silently dropping `n-min` and `n-max` — which could produce benchmark results with llama.cpp's defaults instead of user-specified values.

## Scope

**In scope:**
- Add `ngram_min_values` / `ngram_max_values` to `SpecBenchConfig` and `SweepConfig`
- Wire through `ServerArgs` → `to_args()` for nmin/nmax flag emission
- Add `spec_ngram_min` / `spec_ngram_max` to API request DTO
- UI: expose n-min/n-max inputs, show only for ngram-mod
- Update sweep matrix builder for 3D n-gram-mod sweeps
- Fix `args.rs` docstring claim about `--spec-type` removal
- Dead code cleanup in `llama_cli_spec/args.rs` and `llama_bench/args.rs`

**Out of scope:**
- Changes to the llama-bench (non-spec) benchmark path
- Database schema changes
- New CLI binary flags (tama's spec bench is UI/API-driven only)

## Action items

### Phase 1: Data model — extend SpecBenchConfig and SweepConfig

**File:** `crates/tama-core/src/bench/llama_cli_spec/mod.rs`

**Prime:** Read the existing `SpecBenchConfig`, `SweepConfig`, `build_sweep_matrix()`, and `run_spec_type_group()` in `mod.rs`. Note that `spec_ngram_flags()` already returns empty strings for n-gram-mod's missing flags — but tama has no fields to hold nmin/nmax values.

**Test:** Add unit tests:
- `test_sweep_matrix_ngram_mod_3d`: n-gram-mod with 2 draft_max × 3 n-match × 2 n-min × 2 n-max = 24 entries
- `test_sweep_matrix_ngram_mod_requires_min_max`: error when nmin/nmax values empty for n-gram-mod

**Implement:**
1. Add `ngram_min_values: Vec<u32>` and `ngram_max_values: Vec<u32>` to `SpecBenchConfig` (after `ngram_m_values`, with serde defaults of empty vec)
2. Add `ngram_min: Option<u32>` and `ngram_max: Option<u32>` to `SweepConfig` struct
3. Update `build_sweep_matrix()`:
   - Add `needs_minmax` check: true if any spec_type is NgramMod
   - Bail if nmin/nmax values empty when needed
   - In the NgramMod arm: add nested loops over nmin and nmax, pushing SweepConfig with all three set
4. Update `run_spec_type_group()`: extract `spec_ngram_min` and `spec_ngram_max` from configs (first config's value)

**Validate:** `cargo test --lib bench::llama_cli_spec::tests` — all existing tests still pass, new tests pass.

---

### Phase 2: ServerArgs — wire nmin/nmax through to_args()

**File:** `crates/tama-core/src/bench/llama_cli_spec/server.rs`

**Prime:** Read `ServerArgs` struct and `to_args()` method. Note the pattern: `spec_ngram_flags()` returns `(size_n_flag, size_m_flag, min_hits_flag)` as empty strings for flags that don't apply. For n-gram-mod, we need a different approach since the flags are `--spec-ngram-mod-n-match`, `--spec-ngram-mod-n-min`, `--spec-ngram-mod-n-max` — not size-m/min-hits.

**Test:** Add test for `to_args()` with n-gram-mod including nmin/nmax:
- Create `ServerArgs` with NgramMod, spec_ngram_n=12, spec_ngram_min=3, spec_ngram_max=48
- Verify args contain `--spec-ngram-mod-n-match 12`, `--spec-ngram-mod-n-min 3`, `--spec-ngram-mod-n-max 48`

**Implement:**
1. Add `spec_ngram_min: Option<u32>` and `spec_ngram_max: Option<u32>` to `ServerArgs`
2. In `to_args()`, after the generic n-gram flags block, add a special case for NgramMod:
   ```rust
   if let Some(spec_type) = &self.spec_type {
       // ... existing n-gram flags code ...
       // Type-specific draft flags.
       if let Some(dm) = self.draft_max { /* ... */ }
       if let Some(dmin) = self.draft_min { /* ... */ }
       
       // Ngram-mod needs its own n-min and n-max flags (not covered by spec_ngram_flags).
       if matches!(spec_type, SpecType::NgramMod) {
           if let Some(nmin) = self.spec_ngram_min {
               args.push("--spec-ngram-mod-n-min".to_string());
               args.push(nmin.to_string());
           }
           if let Some(nmax) = self.spec_ngram_max {
               args.push("--spec-ngram-mod-n-max".to_string());
               args.push(nmax.to_string());
           }
       }
   }
   ```
3. Update all `ServerArgs` construction sites (in `mod.rs`'s `run_spec_type_group()` and baseline setup) to pass the new fields

**Validate:** `cargo test --lib bench::llama_cli_spec::server` + full build.

---

### Phase 3: API layer — accept nmin/nmax in request DTO

**File:** `crates/tama-web/src/api/benchmarks.rs`

**Prime:** Read `SpecBenchmarkRunRequest` struct and `run_spec_benchmark()` / `run_spec_benchmark_inner()`.

**Test:** No new tests needed (API is integration-tested via the web UI). Verify manually that the DTO deserializes with the new fields.

**Implement:**
1. Add `#[serde(default)] pub ngram_min_values: Vec<u32>` and `#[serde(default)] pub ngram_max_values: Vec<u32>` to `SpecBenchmarkRunRequest`
2. In `run_spec_benchmark()`, pass these through to `validation_config`
3. In `run_spec_benchmark_inner()`, pass these through to the final `SpecBenchConfig`

**Validate:** Build compiles. No runtime test needed — the existing SSE-based integration covers this.

---

### Phase 4: UI — expose n-min/n-max inputs for ngram-mod

**File:** `crates/tama-web/src/pages/benchmarks/spec_bench.rs`

**Prime:** Read the full `SpecBench` component. Note the existing knob fields (draft_max, ngram_n, ngram_m) and how they're bound via signals. Also read the presets in `SpecPreset::all()`.

**Test:** No unit tests for Leptos UI. Validate visually with `cargo run -p tama-web` or equivalent dev server.

**Implement:**
1. Add two new signals:
   ```rust
   let ngram_min_str = RwSignal::new("3,5".to_string());
   let ngram_max_str = RwSignal::new("48,64".to_string());
   ```
2. In the "Knob Configuration" section, add two new input fields (conditionally shown when ngram-mod is selected):
   - "N-gram min" — text input, helper: "Minimum n-gram matches (ngram-mod only)"
   - "N-gram max" — text input, helper: "Maximum n-gram matches (ngram-mod only)"
3. Use a computed signal or conditional rendering to show/hide these fields based on whether any selected spec_type is `ngram-mod`
4. In `submit_benchmark`, add `ngram_min_values` and `ngram_max_values` to the JSON body
5. Update `apply_preset` to handle nmin/nmax:
   - Add `ngram_min: &'static str` and `ngram_max: &'static str` to `SpecPreset` struct
   - Update all preset definitions with appropriate defaults
6. Split signals for the new fields

**Validate:** Start dev server, open spec bench page, verify n-min/n-max inputs appear/disappear correctly when toggling ngram-mod checkbox.

---

### Phase 5: Cleanup — dead code and docstring fixes

**File:** `crates/tama-core/src/bench/llama_cli_spec/args.rs`

**Prime:** Read the module-level docstring and `build_args()` function. Note that `build_args()` accepts `_spec_type`, `_ngram_n`, `_ngram_m` as unused parameters — this is intentional dead code for API compatibility (the comment says so), but it's misleading.

**Implement:**
1. Delete the entire file `crates/tama-core/src/bench/llama_cli_spec/args.rs` — it is dead code with zero callers (verified: no imports of `llama_cli_spec::args`, `build_args`, or `build_baseline_args` exist anywhere in the codebase).
2. Remove the `mod args;` line from `crates/tama-core/src/bench/llama_cli_spec/mod.rs`.
3. Delete the corresponding test module in the same file (it tests dead functions).

**File:** `crates/tama-core/src/bench/llama_bench/args.rs`

**Prime:** This file is for llama-bench (non-spec), so the dead code here is different. Verify that `LlamaBenchConfig` fields are all actually used in `build_args()`.

**Implement:** No changes needed — this file is clean. The reviewer's concern about dead code applies only to `llama_cli_spec/args.rs`.

**File:** `crates/tama-core/src/bench/llama_cli_spec/mod.rs` — module docstring

**Implement:** Update the module-level docstring from:
```
Spawns a `llama-server` process with the appropriate speculative decoding
flags (`--spec-type`, `--spec-draft-n-max`, `--spec-draft-n-min`, and
type-specific `--spec-ngram-*-size-n/m`)
```
to reflect the current reality (no `--spec-type` flag, uses `--spec-default` + type-specific n-gram flags).

**Validate:** `cargo build` with no warnings. Run `cargo clippy` to verify no new dead-code warnings.

---

### Phase 6: Integration — end-to-end validation

**Validate:**
1. Full test suite: `cargo test --package tama-core`
2. Web UI build: `cargo build -p tama-web` (or `cargo build` for the workspace)
3. Manual smoke test: run a spec benchmark with n-gram-mod selected, verify n-min/n-max flags appear in server logs

## Open questions

1. **What are sensible defaults for n-min/n-max?** llama.cpp's n-gram-mod defaults are typically n-min=3, n-max=48. Should we set these as defaults in the UI or require explicit user input?
2. **Should nmin/nmax be swept independently or always together?** The plan assumes a full 3D Cartesian product (n-match × n-min × n-max), which could explode the matrix size. Consider adding a "sweep depth" guard (e.g., cap total entries at 100).
3. **Resolved:** `llama_cli_spec/args.rs` is confirmed dead code — deleted in Phase 5.
