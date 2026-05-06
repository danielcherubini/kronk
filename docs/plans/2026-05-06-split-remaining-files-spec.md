# Split Remaining Long Files — Spec

**Goal:** Split 3 remaining files > 1,000 lines into focused sub-modules.

---

## 1. `config/resolve/tests/args_building.rs` (1,411 → 5 files)

19 tests for `build_full_args` grouped by feature:

| File | Tests |
|------|-------|
| `mod.rs` | Module declarations + shared imports |
| `basic.rs` | unified, ctx_override, no_sampling, no_quants |
| `dedup.rs` | dedupes, sampling_overrides, dedupes_backend_vs_model, flat_tokens |
| `context_np.rs` | context_multiplied, overflow, no_num_parallel, np_flag, no_np, skips_np |
| `unified_slots.rs` | n_slots, non_unified, default, ctx_override_unified, kv_unified |

## 2. `proxy/tama_handlers/pull/download.rs` (1,041 → 2 files)

- `download.rs` — `start_download_from_queue` (~508 lines)
- `verification.rs` — `run_verification` (~518 lines)

## 3. `api/models/crud/mod.rs` (1,007 → 2 files)

- `mod.rs` — types + helpers (~750 lines)
- `tests.rs` — 21 tests (~250 lines)

---

**Status:** 📋 DRAFT
