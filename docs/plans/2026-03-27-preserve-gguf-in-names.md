# Stop Stripping -GGUF from Model Names

**Goal:** Preserve `-GGUF` / `-gguf` in model IDs and directory paths instead of stripping it during `koji model pull`.
**Status:** ✅ COMPLETED - See git commits `c102bd0` ("fix: preserve -GGUF in model IDs and directory paths (#17)"), `58ad0b4` ("fix: preserve -GGUF in model IDs and directory paths")

**Architecture:** Remove the `clean_parts` stripping logic in the pull command so that `repo_id` is used directly for directory creation, `model_id`, and config filenames. The community card lookup fallback (which tries stripped names as URL candidates) is kept as-is since it's a read-only lookup, not a name mutation.

---

### Task 1: Remove -GGUF stripping from model pull path

**Files:**
- Modify: `crates/koji-cli/src/commands/model.rs` (lines 96-110)

**Steps:**
- [ ] Replace the `clean_parts` logic (lines 98-110) with direct use of `repo_id`:
  - `let model_id = repo_id.to_string();`
  - Build `model_dir` by splitting `repo_id` on `/` and pushing each part onto `models_dir_pathbuf`
- [ ] Remove the comments about stripping -GGUF suffix
- [ ] Run `cargo build --workspace` — verify it compiles
- [ ] Run `cargo test --workspace` — verify all tests pass  
- [ ] Run `cargo clippy --workspace -- -D warnings` — no warnings
- [ ] Commit: `fix: preserve -GGUF in model IDs and directory paths`

**Note:** `fetch_community_card()` in `crates/koji-core/src/models/pull.rs` is NOT changed — it tries the exact repo name first, then falls back to stripped variants for URL lookup. This is correct behavior since community card files on GitHub may be named without `-GGUF`.
