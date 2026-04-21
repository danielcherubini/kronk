# TTS Backend Plan

**Goal:** Add text-to-speech (TTS) support to Koji with Kokoro and Piper engines, exposed as OpenAI-compatible `/v1/audio/*` endpoints on the existing proxy server.

**Architecture:** Two new crates (`koji-tts` for the engine library, `koji-tts-server` for standalone HTTP serving) plus backend installers in `koji-core`. TTS routes are added directly to the existing proxy router on port 11434 so Open WebUI connects to a single URL. TTS config lives in a new `tts_configs` SQLite table following Koji's established migration pattern. TTS is isolated from LLM backends — never evicts LLMs, managed as a singleton.

**Tech Stack:** Rust, tokio, axum (existing), kokoro-micro or tts-rs (new dependency for Kokoro engine), piper-rs (new dependency for Piper engine), SQLite via rusqlite (existing).

---

### Task 1: Database Foundation — `tts_configs` Table + Queries

**Context:**
Koji stores all persistent configuration in SQLite with a migration system. TTS config needs the same treatment — not just TOML, but a proper table with CRUD operations, case-insensitive lookups, and timestamps. This task creates the foundation that every other TTS task depends on.

The `tts_configs` table mirrors the existing `model_configs` pattern: autoincrement PK, UNIQUE engine column with COLLATE NOCASE, JSON-serializable fields where needed, created_at/updated_at timestamps.

**Files:**
- Create: `crates/koji-core/src/db/migrations.rs` (modify — add migration entry)
- Create: `crates/koji-core/src/db/queries/tts_config_queries.rs` (new file)
- Create: `crates/koji-core/src/db/queries/mod.rs` (modify — export new module)
- Create: `crates/koji-core/src/db/types.rs` (modify — add TtsConfigRecord struct)

**What to implement:**

1. **Migration 11** in `migrations.rs` (after migration 10):
```sql
CREATE TABLE tts_configs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    engine       TEXT NOT NULL UNIQUE COLLATE NOCASE,  -- 'kokoro' or 'piper'
    default_voice TEXT,                                -- e.g., 'af_sky'
    speed        REAL   NOT NULL DEFAULT 1.0,          -- 0.5 to 2.0
    format       TEXT   NOT NULL DEFAULT 'mp3',        -- mp3, wav, ogg
    enabled      INTEGER NOT NULL DEFAULT 1,
    created_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

2. **`TtsConfigRecord` struct** in `db/types.rs`:
```rust
#[derive(Debug, Clone)]
pub struct TtsConfigRecord {
    pub id: i64,
    pub engine: String,
    pub default_voice: Option<String>,
    pub speed: f32,
    pub format: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}
```

3. **Query functions** in `tts_config_queries.rs`:
- `upsert_tts_config(conn, record) -> Result<i64>` — INSERT OR REPLACE on engine (UNIQUE constraint)
- `get_tts_config(conn, engine) -> Result<Option<TtsConfigRecord>>` — SELECT WHERE engine = ? COLLATE NOCASE
- `get_all_tts_configs(conn) -> Result<Vec<TtsConfigRecord>>` — SELECT * ORDER BY engine
- `delete_tts_config(conn, engine) -> Result<()>` — DELETE WHERE engine = ?

4. **Export** the new module in `db/queries/mod.rs`.

**Steps:**
- [ ] Add migration 11 entry to `migrations.rs` with the CREATE TABLE SQL above
- [ ] Add `TtsConfigRecord` struct to `db/types.rs`
- [ ] Create `tts_config_queries.rs` with all four query functions, following the exact pattern from `model_config_queries.rs` (same error handling, same use of rusqlite helpers)
- [ ] Export module in `db/queries/mod.rs`
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-core` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(db): add tts_configs table with CRUD queries"

**Acceptance criteria:**
- [ ] Migration 11 creates the `tts_configs` table with all columns matching the SQL above
- [ ] All four query functions compile and follow existing patterns (upsert, get by engine, get all, delete)
- [ ] `TtsConfigRecord` struct has all fields matching the table schema
- [ ] `cargo build --package koji-core` succeeds with no warnings

---

### Task 2: Backend Type Extension + Registry Support

**Context:**
Koji's backend registry uses a `BackendType` enum to distinguish between llama.cpp, ik_llama, and custom backends. TTS needs two new variants (`TtsKokoro`, `TtsPiper`) so the installer, registry, and lifecycle manager can identify TTS backends separately from LLM backends. This is needed before any TTS installation or inference can happen.

**Files:**
- Modify: `crates/koji-core/src/backends/registry/registry_ops.rs` — add enum variants, update Display/FromStr impls
- Modify: `crates/koji-core/src/backends/mod.rs` — verify no changes needed (re-exports already cover mod)

**What to implement:**

1. Add two new variants to `BackendType` enum in `registry_ops.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackendType {
    LlamaCpp,
    IkLlama,
    TtsKokoro,     // NEW — Kokoro TTS engine
    TtsPiper,      // NEW — Piper TTS engine
    Custom,
}
```

2. Update `Display` impl to add:
```rust
BackendType::TtsKokoro => write!(f, "tts_kokoro"),
BackendType::TtsPiper => write!(f, "tts_piper"),
```

3. Update `FromStr` impl to add:
```rust
"tts_kokoro" | "ttskokoro" => Ok(BackendType::TtsKokoro),
"tts_piper" | "tts Piper" => Ok(BackendType::TtsPiper),
```

4. Add a helper method to `BackendType`:
```rust
impl BackendType {
    pub fn is_tts(&self) -> bool {
        matches!(self, BackendType::TtsKokoro | BackendType::TtsPiper)
    }
}
```
This is used later by the lifecycle manager to filter LLM vs TTS backends.

**Steps:**
- [ ] Add `TtsKokoro` and `TtsPiper` variants to `BackendType` enum
- [ ] Update `Display` impl with new match arms
- [ ] Update `FromStr` impl with new match arms (accept both `tts_kokoro` and `ttskokoro` forms for user-friendliness)
- [ ] Add `is_tts()` helper method to `BackendType` impl block
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test --package koji-core -- backends::registry` — did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-core` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(core): add TtsKokoro and TtsPiper backend types"

**Acceptance criteria:**
- [ ] `BackendType` enum has 5 variants: LlamaCpp, IkLlama, TtsKokoro, TtsPiper, Custom
- [ ] `Display` serializes as `"tts_kokoro"` and `"tts_piper"` respectively
- [ ] `FromStr` parses both kebab-case (`tts_kokoro`) and concatenated (`ttskokoro`) forms
- [ ] `is_tts()` returns `true` only for TtsKokoro and TtsPiper
- [ ] All existing registry tests still pass
- [ ] `cargo build --package koji-core` succeeds with no warnings

---

### Task 3: Kokoro Backend Installer (`tts_kokoro`)

**Context:**
LLM backends in Koji have an installer that downloads prebuilt binaries or builds from source, verifies checksums, and registers the backend in the SQLite registry. TTS backends need the same treatment — but instead of a binary server, they download model files (ONNX) and voice packs.

This task creates the Kokoro-specific installer: downloads the ONNX model (~310MB) and all voice files (~27MB) from HuggingFace, stores them in `~/.config/koji/backends/tts_kokoro/`, and registers a `BackendType::TtsKokoro` entry in the registry.

**Files:**
- Create: `crates/koji-core/src/backends/tts_kokoro/mod.rs` (new file)
- Create: `crates/koji-core/src/backends/tts_kokoro/download.rs` (new file)
- Create: `crates/koji-core/src/backends/tts_kokoro/paths.rs` (new file)
- Modify: `crates/koji-core/src/backends/mod.rs` — add tts_kokoro module, re-export install function
- Modify: `crates/koji-core/Cargo.toml` — ensure reqwest and hf-hub dependencies are present

**What to implement:**

1. **`paths.rs`** — resolve model paths from the installation directory:
```rust
use std::path::{Path, PathBuf};

pub fn models_dir(base: &Path) -> PathBuf { base.join("tts_kokoro") }
pub fn model_file(base: &Path) -> PathBuf { models_dir(base).join("kokoro-82m.onnx") }
pub fn voices_dir(base: &Path) -> PathBuf { models_dir(base).join("voices") }
pub fn voice_file(base: &Path, name: &str) -> PathBuf { voices_dir(base).join(format!("{name}.onnx")) }
```

2. **`download.rs`** — download model and voices from HuggingFace:
- Function `download_kokoro_model(progress: &dyn ProgressSink) -> Result<()>`
  - Downloads `hexgrad/Kokoro-82M` ONNX model file from HF releases or models page
  - Uses same download pattern as existing LLM backends (progress reporting, checksum verification)
  - Saves to `~/.config/koji/backends/tts_kokoro/kokoro-82m.onnx`
- Function `download_kokoro_voices(progress: &dyn ProgressSink) -> Result<()>`
  - Downloads all 26 voice ONNX files to `voices/` subdirectory
  - Uses same download pattern
- Both functions accept a `&dyn ProgressSink` for progress reporting (mirrors existing backend installer)

3. **`mod.rs`** — public API:
```rust
pub mod download;
pub mod paths;

use super::{BackendInfo, BackendRegistry, BackendSource, BackendType, ProgressSink};

/// Install the Kokoro TTS backend: download model + voices, register in registry.
pub async fn install_tts_kokoro(registry: &mut BackendRegistry, progress: Box<dyn ProgressSink>) -> Result<()>;

/// Verify the installed Kokoro backend has all required files.
pub fn verify_tts_kokoro(info: &BackendInfo) -> Result<()>;
```

4. **Update `backends/mod.rs`** — add module declaration and re-export:
```rust
pub mod tts_kokoro;
pub use tts_kokoro::download::install_tts_kokoro as install_tts_backend;  // or similar naming
```

**Download source details:**
- Model: HuggingFace repo `hexgrad/Kokoro-82M`, look for the ONNX model file in releases or models page
- Voices: Same repo, voice files are individual ONNX files (one per voice)
- If the exact HF URL is unknown at implementation time, use a placeholder URL and add a TODO comment

**Steps:**
- [ ] Create `paths.rs` with model/voice path resolution functions
- [ ] Create `download.rs` with download functions for model and voices, following existing backend installer patterns (progress reporting, error handling)
- [ ] Create `mod.rs` with public API (`install_tts_kokoro`, `verify_tts_kokoro`)
- [ ] Update `backends/mod.rs` to declare and re-export the new module
- [ ] **Dependency check:** Before building, verify kokoro-micro is available on crates.io. If not, use a GitHub path dependency in `koji-core/Cargo.toml`. The exact crate choice (kokoro-micro vs kokoro-tiny vs tts-rs) should be resolved by checking crates.io — pick whichever has the most recent update and good docs.
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-core` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(core): add Kokoro TTS backend installer"

**Acceptance criteria:**
- [ ] `paths.rs` provides correct paths for model file, voices directory, and individual voice files
- [ ] `download.rs` has download functions that accept `ProgressSink` and report progress
- [ ] `mod.rs` exports `install_tts_kokoro` and `verify_tts_kokoro`
- [ ] Backend is registered as `BackendType::TtsKokoro` after installation
- [ ] `cargo build --package koji-core` succeeds with no warnings

---

### Task 4: Piper Backend Installer (`tts_piper`)

**Context:**
Mirrors Task 3 but for the Piper TTS engine. Piper uses VITS models — each voice is a separate model file (~50-100MB). The installer downloads one default voice and registers `BackendType::TtsPiper`. Additional voices can be installed later (future enhancement, not in scope here).

**Files:**
- Create: `crates/koji-core/src/backends/tts_piper/mod.rs` (new file)
- Create: `crates/koji-core/src/backends/tts_piper/download.rs` (new file)
- Create: `crates/koji-core/src/backends/tts_piper/paths.rs` (new file)
- Modify: `crates/koji-core/src/backends/mod.rs` — add tts_piper module, re-export

**What to implement:**

1. **`paths.rs`**:
```rust
pub fn models_dir(base: &Path) -> PathBuf { base.join("tts_piper") }
pub fn model_file(base: &Path) -> PathBuf { models_dir(base).join("piper.onnx") }
pub fn config_file(base: &Path) -> PathBuf { models_dir(base).join("piper.json") }
```

2. **`download.rs`**:
- Function `download_piper_model(progress: &dyn ProgressSink) -> Result<()>`
  - Downloads default voice (en_US-lessac-medium) from HuggingFace `rhasspy/piper`
  - Saves ONNX model and JSON config to `~/.config/koji/backends/tts_piper/`

3. **`mod.rs`**:
```rust
pub mod download;
pub mod paths;

pub async fn install_tts_piper(registry: &mut BackendRegistry, progress: Box<dyn ProgressSink>) -> Result<()>;
pub fn verify_tts_piper(info: &BackendInfo) -> Result<()>;
```

4. **Update `backends/mod.rs`** — add tts_piper module and re-export.

**Steps:**
- [ ] Create `paths.rs`, `download.rs`, `mod.rs` following the exact same pattern as Task 3 (tts_kokoro)
- [ ] Update `backends/mod.rs` to declare and re-export the new module
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-core` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(core): add Piper TTS backend installer"

**Acceptance criteria:**
- [ ] `paths.rs`, `download.rs`, `mod.rs` follow the same pattern as tts_kokoro
- [ ] Backend is registered as `BackendType::TtsPiper` after installation
- [ ] Default voice (en_US-lessac-medium) is downloaded from HuggingFace
- [ ] `cargo build --package koji-core` succeeds with no warnings

---

### Task 5: TTS Engine Library (`koji-tts` crate)

**Context:**
This is the core inference engine — the bridge between Rust code and actual audio synthesis. It wraps external TTS libraries (kokoro-micro for Kokoro, piper-rs for Piper) behind a unified `TtsEngine` trait. This trait is what the HTTP handlers call to synthesize speech.

The crate provides:
- A `TtsEngine` trait with `synthesize()` (non-streaming), `synthesize_stream()` (SSE streaming), and `voices()` methods
- Concrete implementations for Kokoro and Piper
- An `Engine` enum that delegates to whichever engine is active
- Audio format handling (mp3, wav, ogg)

**Files:**
- Create: `crates/koji-tts/Cargo.toml` (new file)
- Create: `crates/koji-tts/src/lib.rs` (new file)
- Create: `crates/koji-tts/src/kokoro.rs` (new file)
- Create: `crates/koji-tts/src/piper.rs` (new file)
- Create: `crates/koji-tts/src/config.rs` (new file)
- Create: `crates/koji-tts/src/streaming.rs` (new file)
- Modify: `Cargo.toml` (workspace root) — add koji-tts to workspace members

**What to implement:**

1. **`Cargo.toml`**:
```toml
[package]
name = "koji-tts"
version.workspace = true
edition.workspace = true

[dependencies]
anyhow.workspace = true
tokio.workspace = true
futures-core.workspace = true
# TTS engine backends (feature-gated)
kokoro-micro = { version = "1.0", optional = true }
piper-rs = { version = "0.1", optional = true }
```

2. **`config.rs`**:
```rust
#[derive(Debug, Clone, Default)]
pub struct TtsRequest {
    pub text: String,
    pub voice: String,
    pub speed: f32,       // 0.5 to 2.0, default 1.0
    pub format: AudioFormat,
}

#[derive(Debug, Clone, Default)]
pub enum AudioFormat { Mp3, Wav, Ogg }

#[derive(Debug, Clone)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub data: Vec<u8>,
    pub is_final: bool,
}
```

3. **`lib.rs`** — trait + enum:
```rust
pub mod config;
pub mod kokoro;
pub mod piper;
pub mod streaming;

use config::{AudioChunk, AudioFormat, TtsRequest, VoiceInfo};
use std::pin::Pin;
use futures_core::Stream;
use anyhow::Result;

#[async_trait::async_trait]
pub trait TtsEngine: Send + Sync {
    fn name(&self) -> &str;
    fn voices(&self) -> Vec<VoiceInfo>;
    async fn synthesize(&self, req: &TtsRequest) -> Result<Vec<u8>>;
    async fn synthesize_stream(
        &self,
        req: &TtsRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<AudioChunk>> + Send>>>;
}

#[derive(Debug)]
pub enum Engine { Kokoro(kokoro::KokoroEngine), Piper(piper::PiperEngine) }

#[async_trait::async_trait]
impl TtsEngine for Engine {
    fn name(&self) -> &str { ... }      // delegate
    fn voices(&self) -> Vec<VoiceInfo> { ... }  // delegate
    async fn synthesize(&self, req: &TtsRequest) -> Result<Vec<u8>> { ... }  // delegate
    async fn synthesize_stream(...) -> Result<...> { ... }  // delegate
}

pub enum EngineKind { Kokoro, Piper }
pub async fn load_engine(kind: EngineKind, model_path: &std::path::Path) -> Result<Engine>;
```

4. **`kokoro.rs`** — wraps kokoro-micro:
```rust
use super::{TtsEngine, config::*, streaming::*};

pub struct KokoroEngine {
    model: /* kokoro-micro model handle */,
    voices: Vec<VoiceInfo>,  // 26 voices populated from voice files on disk
}

#[async_trait::async_trait]
impl TtsEngine for KokoroEngine { ... }
```

5. **`piper.rs`** — wraps piper-rs:
```rust
pub struct PiperEngine {
    model: /* piper-rs model handle */,
    voice: VoiceInfo,  // single voice (Piper loads one voice per engine instance)
}

#[async_trait::async_trait]
impl TtsEngine for PiperEngine { ... }
```

6. **`streaming.rs`** — SSE chunk conversion helpers:
- `audio_chunk_to_sse(chunk: &AudioChunk) -> String` — formats as `data: <base64>\n\n` or `event: audio\ndata: ...\n\n`

**Steps:**
- [ ] Create workspace entry in root `Cargo.toml`
- [ ] Create `koji-tts/Cargo.toml` — **first check crates.io** for kokoro-micro and piper-rs. If available, use version constraints. If NOT available, use GitHub path dependencies (see dependency notes at bottom of plan) and add a TODO comment.
- [ ] Create `config.rs` with TtsRequest, AudioFormat, VoiceInfo, AudioChunk structs
- [ ] Create `lib.rs` with TtsEngine trait + Engine enum + load_engine factory
- [ ] Create `kokoro.rs` implementing KokoroEngine wrapping kokoro-micro (or whichever crate is chosen)
- [ ] Create `piper.rs` implementing PiperEngine wrapping piper-rs (or whichever crate is chosen)
- [ ] Create `streaming.rs` with SSE formatting helpers
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-tts --features kokoro` — did it succeed? If it fails due to missing crate, resolve the dependency (crates.io version or GitHub path) and retry.
- [ ] Run `cargo build --workspace` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(core): add koji-tts engine library with Kokoro and Piper support"

**Acceptance criteria:**
- [ ] `TtsEngine` trait has all four methods: name(), voices(), synthesize(), synthesize_stream()
- [ ] `Engine` enum delegates to inner KokoroEngine or PiperEngine
- [ ] `load_engine()` factory creates the correct engine type from a path
- [ ] `config.rs` has all required structs (TtsRequest, AudioFormat, VoiceInfo, AudioChunk)
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 6: HTTP API Handlers — `/v1/audio/*` Routes

**Context:**
This task wires the TTS engine into Koji's existing proxy server. Three new routes are added to the axum router in-process (not a separate server), so everything runs on port 11434. Open WebUI connects to `http://localhost:11434/v1` and finds TTS endpoints alongside chat completions.

The handlers follow the OpenAI API format exactly:
- `POST /v1/audio/speech` — takes `{ model, input, voice, response_format }`, returns binary audio
- `POST /v1/audio/speech/stream` — same but streams audio chunks via SSE
- `GET /v1/audio/voices` — returns JSON array of available voices

**Files:**
- Create: `crates/koji-core/src/proxy/handlers/tts.rs` (new file)
- Modify: `crates/koji-core/src/proxy/handlers/mod.rs` — export tts module
- Modify: `crates/koji-core/src/proxy/server/router.rs` — add three new routes
- Modify: `crates/koji-core/src/proxy/state.rs` — add tts_engine field to ProxyState

**What to implement:**

1. **`tts.rs`** handlers:
```rust
use axum::{extract::State, response::IntoResponse, Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use anyhow::Context;

// Request/Response types matching OpenAI API
#[derive(Debug, Deserialize)]
pub struct AudioRequest {
    pub model: String,
    pub input: String,
    pub voice: Option<String>,
    #[serde(default = "default_response_format")]
    pub response_format: String,
    #[serde(default)]
    pub stream: bool,
}

fn default_response_format() -> String { "mp3".to_string() }

#[derive(Debug, Serialize)]
pub struct VoiceResponse {
    pub id: String,
    pub name: String,
    pub language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gender: Option<String>,
}

// GET /v1/audio/voices
pub async fn handle_audio_voices(
    State(state): State<ProxyState>,
) -> impl IntoResponse {
    let engine = state.tts_engine.read().await;
    if let Some(ref eng) = *engine {
        Json(eng.voices().into_iter().map(|v| VoiceResponse { ... }).collect())
    } else {
        (StatusCode::NOT_FOUND, "TTS not installed").into_response()
    }
}

// POST /v1/audio/speech
pub async fn handle_audio_speech(
    State(state): State<ProxyState>,
    Json(req): Json<AudioRequest>,
) -> impl IntoResponse {
    let engine = state.tts_engine.read().await;
    let eng = engine.as_ref().context("TTS not installed")?;
    let audio = eng.synthesize(&koji_tts::config::TtsRequest { ... }).await?;
    axum::response::Binary(audio).into_response()
}

// POST /v1/audio/speech/stream
pub async fn handle_audio_stream(
    State(state): State<ProxyState>,
    Json(req): Json<AudioRequest>,
) -> impl IntoResponse {
    use axum::response::Sse;
    use futures::StreamExt;

    let engine = state.tts_engine.read().await;
    let eng = engine.as_ref().context("TTS not installed")?;
    let stream = eng.synthesize_stream(&koji_tts::config::TtsRequest { ... }).await?;

    // Convert AudioChunks to SSE events:
    // Each chunk becomes: event: audio\ndata: <base64_encoded_audio>\n\n
    // Final chunk (is_final=true) also sends: event: end\n\n
    let sse_stream = stream.map(|chunk_result| {
        match chunk_result {
            Ok(chunk) => {
                let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &chunk.data);
                if chunk.is_final {
                    format!("event: audio\ndata: {}\n\nevent: end\n\n", encoded)
                } else {
                    format!("event: audio\ndata: {}\n\n", encoded)
                }
            }
            Err(e) => format!("event: error\ndata: {}\n\n", e),
        }
    });

    Sse::new(sse_stream).into_response()
}
```

2. **`router.rs`** — add routes:
```rust
.use_state(state.clone())  // or pass tts_engine separately
.route("/v1/audio/speech", post(tts::handle_audio_speech))
.route("/v1/audio/speech/stream", post(tts::handle_audio_stream))
.route("/v1/audio/voices", get(tts::handle_audio_voices))
```

3. **`state.rs`** — add TTS engine field:
```rust
pub struct ProxyState {
    // ... existing fields ...
    pub tts_engine: RwLock<Option<koji_tts::Engine>>,
}
```

The engine is loaded lazily on first TTS request (or at startup if configured). Loading reads the `tts_configs` DB row, resolves model paths from the backend registry, and calls `load_engine()`.

**Steps:**
- [ ] Create `tts.rs` with three handler functions matching OpenAI API format exactly
- [ ] Update `handlers/mod.rs` to export the tts module
- [ ] Update `router.rs` to add the three new routes
- [ ] Update `state.rs` to add `tts_engine: RwLock<Option<koji_tts::Engine>>` field
- [ ] Ensure koji-core Cargo.toml depends on koji-tts crate
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-core` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(proxy): add TTS API handlers for /v1/audio/* endpoints"

**Acceptance criteria:**
- [ ] Three new routes exist: `/v1/audio/speech`, `/v1/audio/speech/stream`, `/v1/audio/voices`
- [ ] `handle_audio_voices` returns JSON array of voice objects matching OpenAI format
- [ ] `handle_audio_speech` accepts `{ model, input, voice, response_format }` and returns binary audio
- [ ] `handle_audio_stream` returns SSE stream of audio chunks
- [ ] Requests return 404 when TTS engine is not loaded/installed
- [ ] `cargo build --package koji-core` succeeds with no warnings

---

### Task 7: Lifecycle Integration — LLM/TTS Isolation + Engine Loading

**Context:**
This task ensures TTS backends are properly isolated from LLM backends in the lifecycle manager. Two key behaviors:
1. `evict_lru_if_needed()` must only count LLM backends against `max_loaded_models` — TTS engines never trigger LLM eviction and vice versa.
2. TTS engine loading/unloading happens on-demand via the API handlers, not through the model lifecycle system. Loading a new TTS engine unloads the previous TTS engine (singleton behavior) without touching any LLM models.

**Files:**
- Modify: `crates/koji-core/src/proxy/lifecycle.rs` — update `evict_lru_if_needed()` to filter by backend type
- Modify: `crates/koji-core/src/proxy/handlers/tts.rs` — add engine loading logic (or create a separate module)

**What to implement:**

1. **Update `evict_lru_if_needed()` in `lifecycle.rs`:**
```rust
// Before the LRU selection, filter to only count LLM backends:
let llm_count = models.iter()
    .filter(|(_, s)| matches!(s, ModelState::Ready { .. }))
    .filter(|(name, _)| {
        // Check backend type from model config — skip TTS backends
        let config = state.model_configs.blocking_read();
        config.get(name).map_or(true, |mc| mc.backend != "tts_kokoro" && mc.backend != "tts_piper")
    })
    .count();

if llm_count < max as usize {
    return Ok(None);  // Not at capacity for LLMs
}
```

2. **TTS engine loading in `tts.rs` or a new module:**
```rust
async fn load_or_get_engine(state: &ProxyState, engine_kind: EngineKind) -> Result<koji_tts::Engine> {
    // Check if already loaded
    {
        let current = state.tts_engine.read().await;
        if let Some(ref eng) = *current {
            if eng_matches_kind(eng, &engine_kind) {
                return Ok(eng.clone());  // Already the right engine loaded
            }
        }
    }

    // Need to load/switch — find installed backend from registry
    let registry = BackendRegistry::open(Config::base_dir()?.join("koji.db"))?;
    let backend = match engine_kind {
        EngineKind::Kokoro => registry.get("tts_kokoro")?,
        EngineKind::Piper => registry.get("tts_piper")?,
    };
    let info = backend.context("TTS backend not installed. Run: koji backend add tts_<engine>")?;

    // Load the engine from the installed model files
    let engine = load_engine_from_path(engine_kind, &info.path).await?;

    // Replace in state (replaces previous TTS engine if any)
    {
        let mut current = state.tts_engine.write().await;
        *current = Some(engine);
    }

    Ok(state.tts_engine.read().await.as_ref().unwrap().clone())
}
```

**Steps:**
- [ ] Update `evict_lru_if_needed()` to filter out TTS backends when counting against `max_loaded_models`
- [ ] Add engine loading/unloading logic that reads from backend registry and updates `state.tts_engine`
- [ ] Ensure loading a new TTS engine replaces (not adds alongside) the previous one
- [ ] Verify LLM models are never evicted during TTS operations
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test --package koji-core -- proxy::lifecycle` — did all tests pass? If not, fix and re-run before continuing.
- [ ] Run `cargo build --workspace` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(proxy): isolate TTS from LLM lifecycle, add engine loading"

**Acceptance criteria:**
- [ ] `evict_lru_if_needed()` only counts LLM backends (filters out tts_kokoro/tts_piper)
- [ ] Loading a new TTS engine replaces the previous one (singleton behavior)
- [ ] No LLM models are evicted or affected during TTS load/unload
- [ ] `cargo build --workspace` succeeds with no warnings

---

### Task 8: CLI Commands — `koji backend add tts_*`, `koji tts` subcommand

**Context:**
Users need a CLI interface to install, configure, and test TTS backends. This task adds:
1. Backend installation commands that work alongside existing LLM backend commands (reusing the installer infrastructure)
2. A new `koji tts` subcommand group for synthesis, voice listing, and configuration
3. DB-backed config management for TTS settings (reading/writing to `tts_configs`)

**Files:**
- Modify: `crates/koji-cli/src/commands/backend/add.rs` — add tts_kokoro/tts_piper options
- Create: `crates/koji-cli/src/commands/tts.rs` (new file)
- Modify: `crates/koji-cli/src/commands/mod.rs` — export tts module
- Modify: `crates/koji-cli/src/main.rs` or command entry point — register tts subcommand
- Create: `crates/koji-cli/src/commands/tts/config.rs` (new file) — config management

**What to implement:**

1. **Backend add commands** — extend existing backend add logic:
```rust
// In backend/add.rs, add new options:
enum BackendAddKind {
    LlamaCpp,
    IkLlama,
    TtsKokoro,   // NEW
    TtsPiper,    // NEW
}

impl BackendAddKind {
    fn install(self, registry: &mut BackendRegistry, progress: Box<dyn ProgressSink>) -> Result<()> {
        match self {
            Self::LlamaCpp => install_llama_cpp(...),
            Self::IkLlama => install_ik_llama(...),
            Self::TtsKokoro => install_tts_kokoro(registry, progress).await?,  // from Task 3
            Self::TtsPiper => install_tts_piper(registry, progress).await?,    // from Task 4
        }
    }
}
```

2. **`koji tts` subcommand group** in `tts.rs`:
```rust
#[derive(Parser)]
pub struct TtsCmd {
    #[command(subcommand)]
    pub sub: TtsSubCmd,
}

#[derive(Parser)]
pub enum TtsSubCmd {
    /// Synthesize speech from text
    Say {
        #[arg(long, default_value = "kokoro")]
        engine: String,
        #[arg(long)]
        voice: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        #[arg(long, default_value = "mp3")]
        format: String,
        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Text to synthesize (stdin if not provided)
        text: String,
    },
    /// List available voices for an engine
    Voices {
        #[arg(long)]
        engine: String,
    },
}
```

3. **`koji tts config` subcommand** in `tts/config.rs`:
```rust
enum TtsConfigCmd {
    /// Set TTS configuration
    Set {
        #[arg(long)]
        engine: String,
        #[arg(long)]
        voice: Option<String>,
        #[arg(long, default_value_t = 1.0)]
        speed: f32,
        #[arg(long, default_value = "mp3")]
        format: String,
    },
    /// Show current TTS configuration
    Show {
        #[arg(long)]
        engine: Option<String>,
    },
}
```

**Steps:**
- [ ] Extend backend add command to support `tts_kokoro` and `tts_piper` options
- [ ] Create `tts.rs` with `Say`, `Voices` subcommands
- [ ] Create `tts/config.rs` with `Set`, `Show` subcommands that read/write `tts_configs` table
- [ ] Register the tts module and subcommand in CLI entry point
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo build --package koji-cli` — did it succeed? If not, fix and re-run before continuing.
- [ ] Commit with message: "feat(cli): add TTS backend install commands and tts subcommand group"

**Acceptance criteria:**
- [ ] `koji backend add tts_kokoro` installs the Kokoro backend
- [ ] `koji backend add tts_piper` installs the Piper backend
- [ ] `koji tts say "hello"` synthesizes speech to stdout
- [ ] `koji tts voices --engine kokoro` lists available Kokoro voices
- [ ] `koji tts config set --engine kokoro --voice af_sky` writes to DB
- [ ] `koji tts config show --engine kokoro` reads from DB
- [ ] `cargo build --package koji-cli` succeeds with no warnings

---

### Task 9: Integration Tests + End-to-End Verification

**Context:**
The final task adds tests that verify the TTS system works end-to-end. Since actual audio synthesis requires model files (which are large and have external dependencies), tests focus on:
1. Unit tests for the engine trait, config parsing, and SSE formatting
2. Integration tests for the HTTP handlers (mocked engine)
3. DB round-trip tests for tts_configs CRUD
4. CLI command tests for backend add and tts subcommands

**Files:**
- Create: `crates/koji-tts/src/tests.rs` (new file, `#[cfg(test)]` module in lib.rs)
- Modify: `crates/koji-core/src/proxy/handlers/tts.rs` — add `#[cfg(test)]` module
- Modify: `crates/koji-core/src/db/queries/tts_config_queries.rs` — add tests
- Create: `crates/koji-cli/tests/tts_integration.rs` (new file, integration tests)

**What to implement:**

1. **Unit tests in `koji-tts/src/tests.rs`:**
```rust
#[test]
fn test_audio_format_serialization() { ... }

#[test]
fn test_tts_request_defaults() { ... }

#[test]
fn test_sse_chunk_formatting() { ... }
```

2. **Handler tests in `tts.rs`:**
```rust
#[tokio::test]
async fn test_audio_voices_returns_404_when_not_loaded() { ... }

#[tokio::test]
async fn test_audio_speech_returns_binary_data() { ... }  // mocked engine
```

3. **DB tests in `tts_config_queries.rs`:**
```rust
#[test]
fn test_upsert_and_get_tts_config() { ... }

#[test]
fn test_case_insensitive_engine_lookup() { ... }

#[test]
fn test_delete_tts_config() { ... }
```

4. **CLI integration tests:**
```rust
#[test]
fn test_tts_cli_help_shows_subcommands() { ... }

#[test]
fn test_backend_list_filters_by_type() { ... }
```

**Steps:**
- [ ] Add unit tests for audio format, request defaults, and SSE formatting in koji-tts
- [ ] Add handler tests with mocked engine returning test data
- [ ] Add DB CRUD tests for tts_configs table (upsert, get, get_all, delete, case-insensitive)
- [ ] Add CLI integration tests for tts subcommands and backend list filtering
- [ ] Run `cargo fmt --all` — did it succeed? If not, fix and re-run before continuing.
- [ ] Run `cargo test --workspace` — did all tests pass? If not, fix and re-run before continuing.
- [ ] Commit with message: "test: add TTS integration tests for handlers, DB, CLI"

**Acceptance criteria:**
- [ ] All unit tests for koji-tts crate pass
- [ ] Handler tests verify 404 when engine not loaded, binary response when loaded
- [ ] DB tests verify all CRUD operations and case-insensitive lookups
- [ ] CLI tests verify tts subcommands appear in help output
- [ ] `cargo test --workspace` passes with no failures

---

## Dependency Notes

- **kokoro-micro** (crates.io v1.0.0): Minimal Kokoro TTS library, auto-downloads models, supports voice mixing and speed control. MIT license.
- **piper-rs** (crates.io v0.1): Piper TTS model wrapper for Rust, ONNX Runtime based. 

If either crate isn't available on crates.io at implementation time, use path dependencies pointing to the GitHub repos:
- Kokoro: `https://github.com/DavidValin/kokoro-micro` or `https://github.com/8b-is/kokoro-tiny`
- Piper: `https://github.com/thewh1teagle/piper-rs`

## Open Questions (to be resolved during implementation)

1. **Exact HuggingFace URLs** for Kokoro ONNX model and voice files — may need to inspect the HF repo structure at implementation time
2. **Which Kokoro Rust crate to use** — `kokoro-micro`, `kokoro-tiny`, or `tts-rs` — depends on feature requirements (streaming support, voice mixing, etc.)
3. **Piper default voice selection** — en_US-lessac-medium is the most popular, but may want to offer a choice during installation
