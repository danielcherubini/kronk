# TTS Subprocess Architecture

**Goal:** Replace inline ONNX inference for Kokoro TTS with a subprocess-based architecture matching the existing LLM backend pattern, enabling GPU (ROCm) acceleration via PyTorch.

**Architecture:** Kokoro-FastAPI runs as a subprocess on its own port. Tama proxies `/v1/audio/*` requests to it using the same `forward_request()` mechanism used for LLMs. Zero ONNX Runtime code in Rust.

**Tech Stack:** Kokoro-FastAPI (Python + PyTorch ROCm), standard pip in venv, subprocess lifecycle, HTTP proxy forwarding

---

## Design Decisions

- **Kokoro-FastAPI over custom**: [remsky/Kokoro-FastAPI](https://github.com/remsky/Kokoro-FastAPI) has working ROCm builds (`.[rocm]` extras), OpenAI-compatible endpoints, streaming, and handles all PyTorch/ROCm complexity. We clone the repo at `v0.3.0` and install via standard pip in a venv.
- **Same lifecycle as LLMs**: Spawn subprocess → health-check `/health` → store in ModelState → proxy forwards requests → SIGTERM on unload.
- **No more `tama-kokoro` or `tama-tts` crates**: Delete both entirely. No more `ort` dependency for TTS.
- **Installer clones git repo + pip install in venv**: Creates a Python venv, clones Kokoro-FastAPI at a pinned tag, installs via `{venv}/bin/pip install -e ".[rocm]"`, downloads model files via the included download script. No `uv` dependency — standard pip works fine.
- **Entry point**: `{venv}/bin/python -m uvicorn api.src.main:app --host 127.0.0.1 --port {PORT}` with CWD set to repo root and `PYTHONPATH={repo_root}`.
- **Streaming**: Kokoro-FastAPI handles streaming via `stream: true` on `/v1/audio/speech` (not a separate endpoint). The tama handler for `/v1/audio/speech/stream` injects `stream: true` into the body and forwards to `{backend_url}/v1/audio/speech`.
- **Kokoro-FastAPI exposes**: `GET /health`, `GET /v1/audio/voices`, `POST /v1/audio/speech`, `GET /v1/audio/models` — all proxied directly.

---

### Task 1: Rewrite TTS Kokoro Installer for Subprocess Backend

**Context:**
The current `tts_kokoro` installer downloads ONNX model files (`.pth`, voices). We need it to install Kokoro-FastAPI as a git-cloned Python project in a venv, so it can be spawned as a subprocess. This replaces model file management with git clone + pip install.

Kokoro-FastAPI is NOT a PyPI package — it must be cloned from GitHub and installed as an editable package. The ROCm extras use `torch==2.8.0+rocm6.4` via the PyTorch ROCm index (`https://download.pytorch.org/whl/rocm6.4`). Model files (`kokoro-v1_0.pth`, `config.json`) are downloaded from a GitHub release URL by the included `docker/scripts/download_model.py` script.

**Files:**
- Modify: `crates/tama-core/src/backends/tts_kokoro/mod.rs`
- Modify: `crates/tama-core/src/backends/tts_kokoro/download.rs`
- Modify: `crates/tama-core/src/backends/tts_kokoro/paths.rs`

**What to implement:**

1. **`paths.rs`**: New path helpers for the venv + repo layout:
   - `install_dir(base_dir)` → `backends/tts_kokoro/kokoro-fastapi/` (git clone target)
   - `venv_dir(base_dir)` → `backends/tts_kokoro/venv/`
   - `python_bin(base_dir)` → `backends/tts_kokoro/venv/bin/python`
   - `model_dir(base_dir)` → `backends/tts_kokoro/kokoro-fastapi/api/src/models/v1_0/` (where download_model.py places files)
   - Remove old `model_file()` and voice path functions

2. **`download.rs`**: Replace ONNX model download with full install:
   - Step 1: Check disk space — warn if <10GB available (PyTorch ROCm + model ~4-6GB). Don't block, just warn.
   - Step 2: Create Python venv via `python3 -m venv {venv_dir}`
   - Step 3: Clone Kokoro-FastAPI repo at pinned tag `v0.3.0`:
     `git clone --depth 1 --branch v0.3.0 https://github.com/remsky/Kokoro-FastAPI.git {install_dir}`
   - Step 4: Install dependencies with standard pip, CWD set to `{install_dir}`:
     - If ROCm detected (`/opt/rocm` exists): `pip install -e ".[rocm]" --extra-index-url https://download.pytorch.org/whl/rocm6.4`
     - Else (CPU fallback): `pip install -e ".[cpu]"`
   - Step 5: Download model files: `{venv}/bin/python {install_dir}/docker/scripts/download_model.py --output {model_dir}`
   - Progress sink updates for each step
   - On failure at any step: clean up partial install (remove venv + repo clone) to avoid broken state

3. **`mod.rs`**: Update install/verify and BackendInfo:
   - `install_tts_kokoro()`: Orchestrate the 5-step install above with progress reporting
   - `verify_tts_kokoro()`: Check (a) `{install_dir}/api/src/main.py` exists, (b) `.git` directory exists (proves clone worked), (c) `{venv}/bin/python -c "import uvicorn; import kokoro"` succeeds, (d) model file exists at `{model_dir}/kokoro-v1_0.pth`

**Dependency note:** Task 2 must complete `tts_engine` removal BEFORE Task 4 can proceed. The `tama-tts` crate deletion in Task 4 will fail to compile if any `tama_tts` imports remain.
   - BackendInfo `path` = `install_dir` (the repo root). The lifecycle code derives venv and python from sibling dirs.

**Steps:**
- [ ] Rewrite `paths.rs` with new venv + repo path helpers
- [ ] Rewrite `download.rs` to clone git + create venv + pip install + download model, with cleanup on failure
- [ ] Update `mod.rs` install/verify logic
- [ ] Run `cargo build -p tama-core`
- [ ] Commit with message: "refactor(tts): replace ONNX installer with Kokoro-FastAPI subprocess"

**Acceptance criteria:**
- [ ] `install_tts_kokoro()` creates venv, clones repo, installs deps, downloads model
- [ ] `verify_tts_kokoro()` returns Ok when all files are present and imports work
- [ ] No ONNX model files downloaded directly (model comes from Kokoro-FastAPI's download script)

---

### Task 2: Add TTS Subprocess Lifecycle to ProxyState

**Context:**
The LLM lifecycle (`load_model`, `unload_model`) spawns a backend binary with args resolved from config. For TTS, we need a simpler lifecycle that spawns the Kokoro-FastAPI uvicorn server. The subprocess state is tracked in the same `models` HashMap (e.g., key `"tts_kokoro"`), using the existing `ModelState` enum.

The entry point is: `{venv}/bin/python -m uvicorn api.src.main:app --host 127.0.0.1 --port {PORT}` with CWD set to repo root and environment variables `PYTHONPATH={repo_root}`, `MODEL_DIR=src/models`, `VOICES_DIR=src/voices/v1_0`. Kokoro-FastAPI auto-detects GPU via PyTorch's `torch.cuda.is_available()` which works with ROCm — no special env var needed.

**Files:**
- Modify: `crates/tama-core/src/proxy/lifecycle.rs`
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`

**What to implement:**

1. **New method on ProxyState**: `load_tts_backend(&self, backend_name: &str) -> Result<String>`
   - Look up backend in registry by name (e.g., `"tts_kokoro"`)
   - Derive paths from BackendInfo.path (repo root): venv_dir = sibling `venv/`, python_bin = `{venv}/bin/python`
   - Find a free port via `TcpListener::bind("127.0.0.1:0")`
   - Spawn subprocess with:
     - Command: `{python} -m uvicorn api.src.main:app --host 127.0.0.1 --port {port}`
     - CWD: `{install_dir}` (repo root)
     - Env: `PYTHONPATH={install_dir}`, `MODEL_DIR=api/src/models`, `VOICES_DIR=api/src/voices/v1_0`
       (These paths are joined with api_dir internally, so they must include the `api/` prefix)
   - Health-check: poll `GET http://127.0.0.1:{port}/health` every 2s, timeout 60s (longer than LLM default because model loads into GPU memory on startup)
   - On success: store in `models` HashMap with key `"tts_kokoro"` and `ModelState::Ready { backend_url: "http://127.0.0.1:{port}", backend_pid, ... }`
   - Return the server name (key)

2. **New method on ProxyState**: `unload_tts_backend(&self, backend_name: &str) -> Result<()>`
   - Look up ModelState by key, get PID
   - SIGTERM the process
   - Wait up to 5s for exit, then SIGKILL
   - Remove from `models` HashMap

3. **New method on ProxyState**: `get_tts_server(&self, backend_name: &str) -> Option<String>`
   - Check if `models` has a ready entry for the given TTS backend name
   - Return the key if found (same pattern as `get_available_server_for_model` but by backend name)

4. **Remove `tts_engine` field** from ProxyState:
   - Delete `pub tts_engine: Arc<tokio::sync::RwLock<Option<tama_tts::Engine>>>` from `types.rs`
   - Delete initialization in `state.rs` (`tts_engine: Arc::new(tokio::sync::RwLock::new(None))`)

**Steps:**
- [ ] Implement `load_tts_backend()` on ProxyState (spawn uvicorn subprocess, health-check)
- [ ] Implement `unload_tts_backend()` on ProxyState (SIGTERM + cleanup)
- [ ] Implement `get_tts_server()` on ProxyState (lookup ready TTS backend)
- [ ] Remove `tts_engine` field from ProxyState (types.rs + state.rs)
- [ ] Run `cargo build -p tama-core`
- [ ] Commit with message: "feat(tts): add subprocess lifecycle for TTS backends"

**Acceptance criteria:**
- [ ] `load_tts_backend("tts_kokoro")` spawns uvicorn on a free port and health-checks `/health`
- [ ] ModelState stored in `models` HashMap with correct backend_url and PID
- [ ] `unload_tts_backend("tts_kokoro")` kills the process and cleans up state
- [ ] No references to `tama_tts` or `tts_engine` remain

---

### Task 3: Rewrite TTS Handlers as HTTP Proxy Forwarders

**Context:**
The current `tts.rs` handler does inline synthesis via `tama_tts::Engine`. Replace all four handlers with proxy forwarders that route to the Kokoro-FastAPI subprocess. The key difference from LLM forwarding: TTS backends are identified by backend name (`kokoro`, `piper`) rather than model config.

Kokoro-FastAPI endpoints (all proxied directly):
- `GET /health` — health check (used by lifecycle, not exposed to clients)
- `GET /v1/audio/voices` — list voices (returns `{ "voices": [...] }`)
- `POST /v1/audio/speech` — synthesize speech (handles both streaming and non-streaming via `stream` boolean in body)
- `GET /v1/audio/models` — list models (returns `{ "data": [{ "id": "kokoro", ... }] }`)

**Files:**
- Modify: `crates/tama-core/src/proxy/handlers/tts.rs`
- Modify: `crates/tama-core/Cargo.toml` (remove `tama-tts` dependency)

**What to implement:**

1. **Rewrite `handle_audio_voices`**:
   - Auto-load TTS backend if not running: `state.load_tts_backend("tts_kokoro").await`
   - Forward GET to `/v1/audio/voices` using `forward_request()` with the TTS server name (e.g., `"tts_kokoro"`). The function internally looks up the URL from stored ModelState.
   - Return 404 if no TTS backend installed in registry

2. **Rewrite `handle_audio_models`**:
   - Auto-load TTS backend if not running
   - Forward GET to `/v1/audio/models` via `forward_request()` (Kokoro-FastAPI exposes this)
   - If no backend loaded, return static list based on registry: `[{"id": "kokoro", ...}]`

3. **Rewrite `handle_audio_speech`** (non-streaming):
   - Parse body to extract `model` field (e.g., `"kokoro"`)
   - Auto-load TTS backend for that model
   - Forward POST to `/v1/audio/speech` via `forward_request()` with the TTS server name
   - Pass through body as-is (Kokoro-FastAPI handles all fields)

4. **Rewrite `handle_audio_stream`** (streaming):
   - Parse JSON body, inject `"stream": true` into the body
   - Forward POST to `/v1/audio/speech` via `forward_request()` with the TTS server name (same endpoint — Kokoro-FastAPI detects `stream` field)
   - Response is raw binary audio chunks forwarded as-is via axum StreamingResponse

5. **Remove `tama-tts` from `tama-core/Cargo.toml`** dependencies.

**Steps:**
- [ ] Rewrite all four TTS handlers as proxy forwarders with auto-load logic
- [ ] Handle streaming handler to inject `stream: true` into forwarded body
- [ ] Remove `tama-tts` from `tama-core/Cargo.toml`
- [ ] Run `cargo build -p tama-core`
- [ ] Update existing TTS handler tests for proxy behavior
- [ ] Commit with message: "refactor(tts): replace inline synthesis with HTTP proxy forwarding"

**Acceptance criteria:**
- [ ] All `/v1/audio/*` endpoints forward to the Kokoro-FastAPI subprocess
- [ ] TTS backend auto-starts on first request (lazy load)
- [ ] Streaming works via `stream: true` in body, raw binary forwarded as-is
- [ ] No references to `tama_tts` remain in tama-core

---

### Task 4: Delete ONNX Crates and Clean Up Dependencies

**Context:**
With TTS now handled as a subprocess, the `tama-kokoro` (ONNX fork) and `tama-tts` (inline synthesis wrapper) crates are dead code. Remove them entirely. Note: `tama-tts` depends on `tama-kokoro`, so removing both from workspace members handles the dependency chain.

**Files:**
- Delete: `crates/tama-kokoro/` (entire directory)
- Delete: `crates/tama-tts/` (entire directory)
- Modify: `Cargo.toml` (workspace members)
- Modify: `crates/tama-core/Cargo.toml` (remove tama-tts dep if still present)

**What to implement:**

1. Remove `"crates/tama-kokoro"` and `"crates/tama-tts"` from `[workspace] members` in root `Cargo.toml`
2. Remove any remaining dependencies on these crates from `tama-core/Cargo.toml` (should already be removed in Task 3)
3. Delete both crate directories
4. Verify no other source files reference `tama_kokoro` or `tama_tts`

**Steps:**
- [ ] Remove crates from workspace members in root `Cargo.toml`
- [ ] Double-check `tama-core/Cargo.toml` has no remaining tama-tts/tama-kokoro deps
- [ ] Delete `crates/tama-kokoro/` and `crates/tama-tts/` directories
- [ ] Run `grep -rn "tama_kokoro\|tama_tts" crates/` to verify no references remain
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Run `cargo test --workspace`
- [ ] Commit with message: "chore(tts): remove tama-kokoro and tama-tts crates"

**Acceptance criteria:**
- [ ] `cargo build --workspace` succeeds without these crates
- [ ] `cargo clippy --workspace -- -D warnings` passes
- [ ] No references to `tama_kokoro` or `tama_tts` in any source file

---

### Task 5: Update CLI and Verify End-to-End

**Context:**
Ensure the CLI `tama backend add tts_kokoro` command works with the new installer. Verify the full flow from install to synthesis. Ensure `safe_remove_installation` handles the venv + repo directories correctly (the old bug that wiped `backends/` was caused by directory paths).

**Files:**
- Modify: `crates/tama-cli/src/commands/backend/` (if TTS-specific code exists)

**What to implement:**

1. Verify CLI backend add/uninstall commands work with the new venv + repo layout
2. Verify `safe_remove_installation` handles the install_dir correctly (it's a directory tree, not a single file — ensure it removes the whole `backends/tts_kokoro/` dir without affecting siblings)
3. End-to-end test: install → serve → voices → speech

**Steps:**
- [ ] Test `tama backend add tts_kokoro` installs Kokoro-FastAPI correctly (on remote with ROCm)
- [ ] Test `tama serve` starts the proxy
- [ ] Test `curl http://localhost:11444/v1/audio/voices` returns voice list (auto-starts subprocess)
- [ ] Test `curl -X POST http://localhost:11444/v1/audio/speech ...` returns audio
- [ ] Verify GPU usage with `rocm-smi` during synthesis
- [ ] Test `tama backend remove tts_kokoro` cleans up venv + repo without wiping other backends
- [ ] Commit with message: "fix(tts): verify CLI and end-to-end subprocess flow"

**Acceptance criteria:**
- [ ] Full install → serve → voices → speech flow works
- [ ] GPU is used for inference (ROCm activity visible)
- [ ] Uninstall cleans up without wiping other backends

---

## End-to-End Test Steps

After all tasks are complete:

```bash
# Install TTS backend (clones repo, creates venv, installs deps, downloads model)
tama backend add tts_kokoro

# Start the proxy
tama serve

# List voices (auto-starts Kokoro-FastAPI subprocess)
curl http://localhost:11444/v1/audio/voices

# Generate speech
curl -X POST http://localhost:11444/v1/audio/speech \
  -H 'Content-Type: application/json' \
  -d '{"model":"kokoro","input":"Hello world","voice":"af_nicole"}' \
  -o /tmp/test.wav

# Check GPU usage (should show ROCm activity during synthesis)
rocm-smi

# Uninstall
tama backend remove tts_kokoro
```
