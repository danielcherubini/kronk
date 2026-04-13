# Integrate hf-hub for Authenticated Parallel Downloads Plan

**Goal:** Use `hf-hub` to provide an authenticated `reqwest::Client`, then pass that client to our existing parallel `download_chunked` logic.
**Architecture:** Refactor the download module to accept an existing `reqwest::Client`. Update the pull handlers to retrieve the authenticated client from the `hf-hub` API singleton.
**Tech Stack:** Rust, `reqwest`, `hf-hub`, `tokio`

---

### Task 1: Refactor `download_chunked` and clean up auth parameters

**Context:**
Currently, `download_chunked` builds its own client and takes an `auth_header`. This makes it impossible to use a pre-authenticated client (like the one from `hf-hub`). We need to refactor the entire download pipeline to accept a client and remove the redundant `auth_header` parameters that are no longer needed once the client is configured.

**Files:**
- Modify: `crates/koji-core/src/models/download/mod.rs`
- Modify: `crates/koji-core/src/models/download/single.rs`
- Modify: `crates/koji-core/src/models/download/parallel.rs`
- Modify: `crates/koji-core/src/models/pull.rs`
- Modify: `crates/koji-cli/src/commands/model.rs`

**What to implement:**
1.  **In `crates/koji-core/src/models/download/mod.rs`**:
    - Change `download_chunked` signature to: `pub async fn download_chunked(client: &Client, url: &str, dest: &Path, connections: usize) -> Result<u64>`
    - Remove the `auth_header` parameter and the logic that builds a new client inside it.
    - Update the calls to `single::download_single` and `parallel::download_parallel` to pass the `client` instead of `None`.
2.  **In `crates/koji-core/src/models/download/single.rs`**:
    - Update `download_single` signature to accept `client: &Client` and remove `auth_header`.
3.  **In `crates/koji-core/src/models/download/parallel.rs`**:
    - Update `download_parallel` and `download_chunk_with_retry` signatures to accept `client: &Client` and remove `auth_header`.
4.  **In `crates/koji-core/src/models/pull.rs`**:
    - Update `download_gguf` signature to accept `client: &Client` and remove `auth_header`.
    - Update the call to `download_chunked` inside `download_gguf`.
5.  **In `crates/koji-cli/src/commands/model.rs`**:
    - Update calls to `download_gguf` (specifically around lines 242 and 872) to create/provide a client. *Note: Since this is a CLI, we can just create a standard `reqwest::Client::new()` here.*

**Steps:**
- [ ] Run `cargo check` to find all call sites.
- [ ] Implement signature changes in `mod.rs`, `single.rs`, and `parallel.rs`.
- [ ] Update internal calls within `mod.rs`.
- [ ] Update `download_gguf` in `pull.rs`.
- [ ] Update `koji-cli` callers in `crates/koji-cli/src/commands/model.rs`.
- [ ] Update unit tests in `crates/koji-core/src/models/download/mod.rs` to pass a `Client`.
- [ ] Run `cargo test --package koji-core`
- [ ] Run `cargo fmt`
- [ ] Commit with message: "refactor: make download_chunked accept an existing reqwest::Client and remove redundant auth headers"

**Acceptance criteria:**
- [ ] All download functions (`download_chunked`, `download_single`, `download_parallel`, `download_gguf`) accept a `&Client`.
- [ ] The `auth_header` parameter is completely removed from the download module.
- [ ] `koji-cli` still works (using a standard client).
- [ ] All tests pass.

---

### Task 2: Update Pull Handler to use Authenticated `hf-hub` Client

**Context:**
The `spawn_download_job` function in the pull handler needs to use the `HF_TOKEN` for gated/private repos. We will now use the existing `hf-hub` singleton to get a client that already has the authentication headers configured.

**Files:**
- Modify: `crates/koji-core/src/proxy/koji_handlers/pull.rs`

**What to implement:**
1.  In `spawn_download_job`:
    - Retrieve the authenticated client using `crate::models::pull::hf_api().await?.client()`.
    - Pass this authenticated client to the call to `crate::models::download::download_chunked`.
    - Ensure the URL construction remains the same (the existing `https://huggingface.co/{repo}/resolve/main/{file}` is correct).

**Steps:**
- [ ] Implement the authenticated client retrieval in `spawn_download_job`.
- [ ] Update the `download_chunked` call to pass the client.
- [ ] Run `cargo test --package koji-core`
- [ ] Run `cargo fmt`
- [ ] Commit with message: "feat(proxy): use authenticated hf-hub client for model pulls"

**Acceptance criteria:**
- [ ] The pull job now uses the `hf-hub` client.
- [ ] Authentication works correctly for gated/private repositories.
- [ ] All tests pass.
