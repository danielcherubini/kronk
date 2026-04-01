# Fix Download Progress Bar Plan

**Goal:** Fix the download progress bar showing "0 B/0 B" and the completion message appearing broken during `kronk model pull`.
**Status:** DONE

**Architecture:** The root cause is a known reqwest bug (#843, open since 2020) where `Response::content_length()` returns `Some(0)` for HEAD requests despite the raw `Content-Length` header being correct. The fix parses the header manually, adds a testable helper, cleans up progress bar completion in both download paths, and simplifies the control flow.

**Tech Stack:** Rust, reqwest, indicatif

---

### Task 1: Extract and fix Content-Length parsing for HEAD requests

**Files:**
- Modify: `crates/kronk-core/src/models/download/mod.rs`

**Steps:**
- [ ] Extract a helper function `parse_content_length(headers: &reqwest::header::HeaderMap) -> Option<u64>` that manually parses the `Content-Length` header from raw headers, bypassing `reqwest::Response::content_length()` which returns `Some(0)` or `None` for HEAD responses (known bug: https://github.com/seanmonstar/reqwest/issues/843)
- [ ] Write unit tests for the helper:
  - Header present with valid value → returns `Some(value)`
  - Header missing → returns `None`
  - Header present with non-numeric value → returns `None`
  - Header present with `"0"` → returns `Some(0)`
- [ ] Run tests, verify they pass
- [ ] Replace `head.content_length()` (lines 63-65) with the new helper:
  ```rust
  let total_size = parse_content_length(head.headers())
      .context("Server did not return a valid Content-Length")?;
  ```
- [ ] Add a zero-size guard after the extraction:
  ```rust
  if total_size == 0 {
      anyhow::bail!("Server reported Content-Length of 0 for {}", url);
  }
  ```
- [ ] Run tests (`cargo test --workspace`), verify they pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit

---

### Task 2: Fix progress bar completion in model download

**Files:**
- Modify: `crates/kronk-core/src/models/download/mod.rs`

**Steps:**
- [ ] Observe that `pb.finish_with_message("done")` (line 112) sets a `{msg}` value that isn't rendered because the progress bar template has no `{msg}` placeholder — the bar just freezes showing the last state
- [ ] Simplify the match block (lines 110-119). Both arms should call `pb.finish_and_clear()`, so hoist it above the match:
  ```rust
  pb.finish_and_clear();
  result?;
  Ok(total_size)
  ```
  The CLI already prints "Downloaded: ..." after this returns, so the progress bar should cleanly disappear.
- [ ] Run tests (`cargo test --workspace`), verify they pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit

---

### Task 3: Fix progress bar completion in backend installer download

**Files:**
- Modify: `crates/kronk-core/src/backends/installer/download.rs`

**Steps:**
- [ ] Observe that `pb.finish_with_message("Download complete")` (line 54) has the same `{msg}` template mismatch — the template on line 35 has no `{msg}` placeholder
- [ ] Replace `pb.finish_with_message("Download complete")` with `pb.finish_and_clear()`. The calling code in `prebuilt.rs` already prints status messages after download.
- [ ] Also replace `response.content_length().unwrap_or(0)` (line 30) with the `parse_content_length` helper from Task 1 for consistency. Since this is a GET request the reqwest bug doesn't apply, but using the helper is more robust and consistent. Use `unwrap_or(0)` on the helper result since this path should gracefully degrade to an indeterminate progress bar when content length is unknown (chunked transfer encoding).
- [ ] Run tests (`cargo test --workspace`), verify they pass
- [ ] Run `cargo clippy --workspace -- -D warnings`
- [ ] Commit
