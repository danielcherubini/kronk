# Unified Server Plan

**Goal:** Merge the proxy server (port 11434) and web UI server (port 11435) into a single server on one port.

**Architecture:** The proxy server (`tama-core`) becomes the single entry point. When built with the `web-ui` feature, it merges the web UI's API routes and static file serving into its own axum router. The `tama-web` crate's handlers are refactored to accept `Arc<ProxyState>` instead of `Arc<AppState>`, eliminating the inter-process HTTP proxying between the two servers.

**Tech Stack:** Rust, axum, Leptos (frontend unchanged), tokio

---

## Route Map (After Merge)

All routes served from a single port (default 11434):

| Path Prefix | Handler | Source |
|---|---|---|
| `/v1/*` | OpenAI-compatible API | `tama-core` proxy (unchanged) |
| `/tama/v1/*` | Management API | Merged: proxy routes + web UI routes |
| `/health`, `/status`, `/metrics` | System endpoints | `tama-core` proxy (unchanged) |
| `/` and `/*` | SPA static files | `tama-web` (embedded dist/) |

**Route conflicts resolved:**
- `/tama/v1/models` and `/tama/v1/models/:id` — web UI handlers win (more feature-complete CRUD)
- `/tama/v1/hf/*repo_id` — web UI handler wins (direct HF API call, no proxy hop needed)
- Proxy-only routes (`/tama/v1/models/:id/load`, `/tama/v1/models/:id/unload`, `/tama/v1/pulls/*`, `/tama/v1/system/health`, `/tama/v1/system/reload-configs`, `/tama/v1/system/metrics/stream`, `/tama/v1/system/restart`, `/tama/v1/logs`, `/tama/v1/logs/:backend/events`, `/tama/v1/opencode/models`) — kept from proxy

---

### Task 1: Expose ProxyState, add web-ui feature, and web-specific fields

**Context:**
The web UI handlers currently use `AppState` (defined in `tama-web`) which has fields not present on `ProxyState`: `update_checker`, `binary_version`, `update_tx`, `upload_lock`, `jobs`, `capabilities`. After the merge, all handlers share a single state type. Since axum's `State<T>` only supports one type, we add the web-specific fields to `ProxyState` behind `#[cfg(feature = "web-ui")]` conditional compilation. `ProxyState::new()` initializes these fields when the feature is enabled.

**Field mapping (AppState → ProxyState):**

| AppState field | ProxyState field | Notes |
|---|---|---|
| `proxy_base_url` | Removed | No longer needed — single server |
| `client` | `client` (already exists) | ProxyState already has `reqwest::Client` |
| `logs_dir` | Derived via `config.logs_dir()` | `Config::logs_dir()` method in `loader.rs` |
| `config_path` | Derived via `config.loaded_from` | `Option<PathBuf>` on Config |
| `proxy_config` | `config` (already exists) | Same `Arc<RwLock<Config>>` |
| `jobs` | New: `web_jobs` | `Option<Arc<JobManager>>`, web-ui only |
| `capabilities` | New: `web_capabilities` | `Option<Arc<CapabilitiesCache>>`, web-ui only |
| `update_checker` | New: `web_update_checker` | `Arc<UpdateChecker>`, web-ui only |
| `binary_version` | New: `web_binary_version` | `String`, web-ui only |
| `update_tx` | New: `web_update_tx` | `Arc<Mutex<Option<broadcast::Sender<String>>>>>`, web-ui only |
| `upload_lock` | New: `web_upload_lock` | `Arc<RwLock<HashMap<String, UploadEntry>>>`, web-ui only |
| `download_queue` | `download_queue` (already exists) | Same field |

**Files:**
- Modify: `crates/tama-core/src/proxy/mod.rs`
- Modify: `crates/tama-core/src/proxy/types.rs`
- Modify: `crates/tama-core/src/proxy/state.rs`
- Modify: `crates/tama-core/Cargo.toml`

**What to implement:**

1. In `crates/tama-core/src/proxy/types.rs`, add web-specific fields to `ProxyState`:
   ```rust
   // Add to ProxyState struct, all behind #[cfg(feature = "web-ui")]
   #[cfg(feature = "web-ui")]
   pub web_jobs: Option<Arc<tama_web::jobs::JobManager>>,
   #[cfg(feature = "web-ui")]
   pub web_capabilities: Option<Arc<tama_web::api::backends::CapabilitiesCache>>,
   #[cfg(feature = "web-ui")]
   pub web_update_checker: Arc<crate::updates::UpdateChecker>,
   #[cfg(feature = "web-ui")]
   pub web_binary_version: String,
   #[cfg(feature = "web-ui")]
   pub web_update_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<String>>>>,
   #[cfg(feature = "web-ui")]
   pub web_upload_lock: Arc<tokio::sync::RwLock<std::collections::HashMap<String, tama_web::api::backup::UploadEntry>>>,
   ```
   
   **IMPORTANT:** `tama_web` types (`JobManager`, `CapabilitiesCache`, `UploadEntry`) are only available when `tama-web` is a dependency. Since `tama-core` doesn't depend on `tama-web`, we need to either:
   - Define equivalent types in `tama-core` (preferred — `JobManager` and `CapabilitiesCache` should live in `tama-core` anyway), OR
   - Use `dyn` traits / type erasure, OR
   - Add `tama-web` as an optional dependency of `tama-core`
   
   **Decision:** Add `tama-web` as an optional dependency of `tama-core` behind `web-ui` feature. This is acceptable because `tama-core` is only used by `tama-cli` (the binary), and the circular dependency concern doesn't apply — `tama-web → tama-core` already exists, and `tama-core → tama-web` is optional.

2. In `crates/tama-core/src/proxy/state.rs` (`ProxyState::new`), initialize web fields:
   ```rust
   #[cfg(feature = "web-ui")]
   web_jobs: None, // Set later by build_unified_router
   #[cfg(feature = "web-ui")]
   web_capabilities: None,
   #[cfg(feature = "web-ui")]
   web_update_checker: Arc::new(crate::updates::UpdateChecker::new()),
   #[cfg(feature = "web-ui")]
   web_binary_version: String::new(), // Set later by CLI
   #[cfg(cfg(feature = "web-ui"))]
   web_update_tx: Arc::new(tokio::sync::Mutex::new(None)),
   #[cfg(feature = "web-ui")]
   web_upload_lock: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
   ```

3. In `crates/tama-core/Cargo.toml`, add optional dependencies:
   ```toml
   [features]
   web-ui = [
     "dep:tama-web",
     "dep:include_dir", "dep:mime_guess",
   ]
   
   [dependencies]
   tama-web = { path = "../tama-web", features = ["ssr"], optional = true }
   include_dir = { workspace = true, optional = true }
   mime_guess = { workspace = true, optional = true }
   # uuid already exists with v4 feature — no need to add conditionally
   ```

4. In `crates/tama-core/src/proxy/mod.rs`, verify `ProxyState` is re-exported (check `pub use types::ProxyState` or similar).

**Steps:**
- [ ] Verify `ProxyState` is `pub` in `crates/tama-core/src/proxy/types.rs` (line ~221)
- [ ] Add `tama-web` as optional dependency to `crates/tama-core/Cargo.toml`
- [ ] Add `include_dir` and `mime_guess` as optional dependencies
- [ ] Add `web-ui` feature to `crates/tama-core/Cargo.toml`
- [ ] Add web-specific fields to `ProxyState` struct in `types.rs` (all `#[cfg(feature = "web-ui")]`)
- [ ] Initialize web fields in `ProxyState::new()` in `state.rs`
- [ ] Verify `ProxyState` is re-exported in `proxy/mod.rs`
- [ ] Run `cargo check --package tama-core --features web-ui`
  - Did it succeed? If not, fix dependency/type issues and re-run.
- [ ] Run `cargo check --package tama-core` (without web-ui)
  - Did it succeed? This ensures non-web builds are unaffected.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "chore: add web-ui feature and web fields to ProxyState"

**Acceptance criteria:**
- [ ] `ProxyState` has all fields needed by web handlers (jobs, capabilities, update_checker, binary_version, update_tx, upload_lock)
- [ ] All web fields are `#[cfg(feature = "web-ui")]`
- [ ] `tama-core` compiles with `--features web-ui`
- [ ] `tama-core` compiles without features (non-web build)
- [ ] No new clippy warnings in either build
- [ ] No circular dependency issues (tama-core → tama-web is optional, tama-web → tama-core is always)

---

### Task 2a: Refactor config and API handlers (api.rs, middleware, models)

**Context:**
The config handlers (`get_config`, `save_config`, `get_structured_config`, `save_structured_config`) and model handlers currently use `AppState`. They need to switch to `ProxyState`. The config handlers also have helper functions (`sync_proxy_config`, `trigger_proxy_reload`, `load_config_from_state`) that need updating.

**Field mapping for these handlers:**
- `state.config_path` → `state.config.read().await.loaded_from.clone()` (both are `Option<PathBuf>`)
- `state.proxy_config` → `state.config` (same `Arc<RwLock<Config>>`)
- `state.logs_dir` → `state.config.read().await.logs_dir()?` (uses `Config::logs_dir()` method from `loader.rs`)

**Files:**
- Modify: `crates/tama-web/src/api.rs`
- Modify: `crates/tama-web/src/api/models/*.rs`
- Modify: `crates/tama-web/src/api/middleware.rs` (CSRF — verify no AppState usage, should be clean)

**What to implement:**
1. Change all handler signatures from `State<Arc<AppState>>` to `State<Arc<tama_core::proxy::ProxyState>>`
2. In `sync_proxy_config`: replace `state.proxy_config` with `state.config` (same lock, just write directly)
3. In `trigger_proxy_reload`: replace HTTP call to `{proxy_base_url}/tama/v1/system/reload-configs` with `state.reload_model_configs().await`
4. In `load_config_from_state`: replace `state.config_path` with `state.config.read().await.loaded_from.clone()`
5. In `get_logs` (if in api.rs): replace `state.logs_dir` with `state.config.read().await.logs_dir()?`
6. In model handlers: update any `AppState` field references

**Steps:**
- [ ] Update handler signatures in `api.rs` to use `State<Arc<ProxyState>>`
- [ ] Replace `sync_proxy_config` to write directly to `state.config`
- [ ] Replace `trigger_proxy_reload` to call `state.reload_model_configs().await`
- [ ] Replace `load_config_from_state` to derive config_path from `state.config`
- [ ] Update model handlers in `api/models/` to use `ProxyState`
- [ ] Verify `middleware.rs` has no `AppState` references (CSRF middleware is stateless)
- [ ] Run `cargo check --package tama-web --features ssr`
  - Fix compilation errors one at a time
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: config and model handlers use ProxyState"

**Acceptance criteria:**
- [ ] All config handlers use `ProxyState`
- [ ] No HTTP calls to proxy for config sync or reload
- [ ] `cargo check --package tama-web --features ssr` passes for these handlers

---

### Task 2b: Refactor benchmark handlers (direct unload calls)

**Context:**
The benchmark handlers (`run.rs`, `spec.rs`, `mtp.rs`) make HTTP calls to the proxy's `/tama/v1/models/{id}/unload` endpoint via `unload_model_before_benchmark`. With a single server, they should call `state.unload_model(&model_id).await` directly.

**Files:**
- Modify: `crates/tama-web/src/api/benchmarks/run.rs`
- Modify: `crates/tama-web/src/api/benchmarks/spec.rs`
- Modify: `crates/tama-web/src/api/benchmarks/mtp.rs`

**What to implement:**
1. Change handler signatures from `State<Arc<AppState>>` to `State<Arc<ProxyState>>`
2. Replace `unload_model_before_benchmark(&client, &proxy_base_url, model_id, job_id)` with:
   ```rust
   let _ = state.unload_model(&req.model_id).await;
   ```
3. Remove `proxy_base_url` parameter from all internal async functions
4. Remove `reqwest::Client` usage (use `state.client` if any HTTP calls remain, but they shouldn't)
5. Update the `run_benchmark_task` / internal function signatures to accept `Arc<ProxyState>` instead of `(Client, String)`

**Steps:**
- [ ] Update handler signatures in `run.rs`, `spec.rs`, `mtp.rs`
- [ ] Replace HTTP unload calls with `state.unload_model(&model_id).await`
- [ ] Remove `proxy_base_url` from all internal function signatures
- [ ] Remove `client` parameter where no longer needed
- [ ] Run `cargo check --package tama-web --features ssr`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: benchmark handlers use ProxyState directly"

**Acceptance criteria:**
- [ ] No HTTP calls to proxy from benchmark handlers
- [ ] `state.unload_model()` called directly
- [ ] No `proxy_base_url` references in benchmark files

---

### Task 2c: Refactor self-update, updates, backup, and remaining handlers

**Context:**
The self-update handlers use `state.binary_version`, `state.update_tx`, `state.update_checker`. The backup handlers use `state.upload_lock`. The updates handlers use `state.update_checker`. These fields are now on `ProxyState` (added in Task 1) as `web_binary_version`, `web_update_tx`, etc.

**Files:**
- Modify: `crates/tama-web/src/api/self_update.rs`
- Modify: `crates/tama-web/src/api/updates.rs`
- Modify: `crates/tama-web/src/api/backup.rs`
- Modify: `crates/tama-web/src/api/downloads.rs`
- Modify: `crates/tama-web/src/api/logs.rs`
- Modify: `crates/tama-web/src/api/openapi.rs`
- Modify: `crates/tama-web/src/api/hf.rs`

**What to implement:**
1. **self_update.rs:** Replace `state.binary_version` → `state.web_binary_version`, `state.update_tx` → `state.web_update_tx`
2. **updates.rs:** Replace `state.update_checker` → `state.web_update_checker`
3. **backup.rs:** Replace `state.upload_lock` → `state.web_upload_lock`, `state.config_path` → derive from config
4. **downloads.rs:** `state.download_queue` is the same field on `ProxyState` — minimal change
5. **logs.rs:** `state.logs_dir` → `state.config.read().await.logs_dir()?`
6. **openapi.rs:** Update state type if used
7. **hf.rs:** Currently proxies to proxy via HTTP. Replace with direct call. The proxy's `handle_hf_list_quants` calls into `tama_core::proxy::tama_handlers::hf` — find the underlying function and call it directly, or simply remove this handler since the proxy's HF route will be in the merged router.

**For hf.rs specifically:** The web UI's HF handler at `/tama/v1/hf/*repo_id` proxies to the proxy's same route. After merge, the proxy's handler serves this route directly. **Action:** Remove the web UI's `hf_metadata` handler entirely — the proxy's `handle_hf_list_quants` will handle it in the merged router.

**Steps:**
- [ ] Update all handler signatures to use `State<Arc<ProxyState>>`
- [ ] Replace field names: `binary_version` → `web_binary_version`, `update_tx` → `web_update_tx`, etc.
- [ ] Remove `api/hf.rs` handler (proxy's handler takes over in merged router)
- [ ] Update `api/hf` module — either remove or keep only types/re-exports
- [ ] Run `cargo check --package tama-web --features ssr`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: self-update, backup, and remaining handlers use ProxyState"

**Acceptance criteria:**
- [ ] All remaining handlers use `ProxyState`
- [ ] Field names match Task 1's naming (`web_*` prefix)
- [ ] `api/hf.rs` proxy handler removed
- [ ] `cargo check --package tama-web --features ssr` passes

---

### Task 2d: Remove AppState and create build_web_routes

**Context:**
Now that all handlers use `ProxyState`, remove the old `AppState` struct and server entry points from `tama-web`. Create the `build_web_routes` function that returns an un-configured `Router` (no `.with_state()`) for the proxy to merge.

**Files:**
- Modify: `crates/tama-web/src/server.rs` (major cleanup)
- Create: `crates/tama-web/src/router.rs` (new module)
- Modify: `crates/tama-web/src/lib.rs`

**What to implement:**

1. **Remove from `server.rs`:**
   - `AppState` struct and `impl AppState`
   - `serve_static`, `proxy_tama`, `serve_index` handlers
   - `build_router` function
   - `run_with_opts`, `run`, `shutdown_signal_inner` functions
   - `DIST` static (move to `router.rs`)

2. **Create `router.rs`:**
   ```rust
   use axum::{
       routing::{any, delete, get, post},
       Router,
   };
   use include_dir::{include_dir, Dir};
   use std::sync::Arc;
   use tama_core::proxy::ProxyState;
   // ... imports for all handlers

   static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

   pub fn build_web_routes() -> Router {
       // Build all web UI routes
       // Use ProxyState in all State extractors
       // Include static file serving and SPA fallback
       // Do NOT call .with_state() — caller attaches state
       Router::new()
           .route("/tama/v1/system/capabilities", get(system_capabilities))
           // ... all web routes ...
           .route("/tama/v1/docs", get(api::openapi::serve_spec))
           .route("/tama/v1/logs/:backend", get(api::logs::get_backend_logs))
           .merge(csrf_routes)
           .merge(backend_routes)
           .route("/", get(serve_index))
           .route("/*path", get(serve_static_fallback))
           .layer(CatchPanicLayer::new())
   }
   ```

3. **Update `lib.rs`:** Export `router` module under `#[cfg(feature = "ssr")]`

**Steps:**
- [ ] Create `crates/tama-web/src/router.rs` with `build_web_routes()` function
- [ ] Move `DIST` static and `serve_static`/`serve_index` handlers to `router.rs`
- [ ] Move all route definitions from `server.rs::build_router` to `router.rs::build_web_routes`
- [ ] Remove `server.rs` entirely (or reduce to minimal re-exports for backward compat)
- [ ] Update `lib.rs` to export `router` module
- [ ] Run `cargo check --package tama-web --features ssr`
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: replace AppState/server with build_web_routes"

**Acceptance criteria:**
- [ ] `AppState` struct removed from `tama-web`
- [ ] `build_web_routes()` returns `Router` without `.with_state()`
- [ ] Static file serving included in `build_web_routes`
- [ ] SPA fallback included
- [ ] `server.rs` removed or minimal
- [ ] `cargo check --package tama-web --features ssr` passes

---

### Task 3: Merge routers in tama-core proxy server

**Context:**
The proxy server's router (`tama-core/src/proxy/server/router.rs`) currently only serves proxy routes. When built with `web-ui` feature, it should merge the web UI's routes (API + static files) into a single router. The proxy's `ProxyServer.run()` method currently spawns a separate web server task — this becomes unnecessary.

**Route priority (critical):** In axum, routes are matched in definition order. More specific routes MUST be defined before more general ones. The merged router must define proxy-specific routes before the web UI's broader catch-alls:

**Must come first (proxy routes):**
- `/tama/v1/models/:id/load` (POST) — before web's `/tama/v1/models/:id`
- `/tama/v1/models/:id/unload` (POST) — before web's `/tama/v1/models/:id`
- `/tama/v1/pulls` (POST) — before any wildcard
- `/tama/v1/pulls/:job_id` (GET) — before any wildcard
- `/tama/v1/pulls/:job_id/stream` (GET) — before any wildcard
- `/tama/v1/system/health` (GET) — before web's `/tama/v1/system/*`
- `/tama/v1/system/reload-configs` (POST)
- `/tama/v1/system/metrics/stream` (GET)
- `/tama/v1/system/restart` (POST)
- `/tama/v1/logs` (GET) — before web's `/tama/v1/logs/:backend`
- `/tama/v1/logs/:backend/events` (GET) — before web's `/tama/v1/logs/:backend`
- `/tama/v1/opencode/models` (GET)

**Can come after (web UI routes):**
- `/tama/v1/models` (GET, POST) — web's CRUD
- `/tama/v1/models/:id` (GET, PUT, DELETE) — web's CRUD
- `/tama/v1/models/:id/rename`, `/tama/v1/models/:id/refresh`, etc.
- `/tama/v1/backends/*` — web only
- `/tama/v1/benchmarks/*` — web only
- `/tama/v1/downloads/*` — web only
- `/tama/v1/config` — web only
- `/tama/v1/logs/:backend` (GET) — web's file-based fallback
- `/` and `/*` — static files / SPA fallback

**Files:**
- Modify: `crates/tama-core/src/proxy/server/router.rs`
- Modify: `crates/tama-core/src/proxy/server/mod.rs`

**What to implement:**

1. In `router.rs`, create `build_unified_router` (gated on `#[cfg(feature = "web-ui")]`):
   ```rust
   #[cfg(feature = "web-ui")]
   pub fn build_unified_router(state: Arc<ProxyState>) -> Router {
       // Build proxy routes (specific routes first)
       let proxy_routes = Router::new()
           // OpenAI-compatible routes
           .route("/v1", post(handle_chat_completions))
           .route("/v1/chat/completions", post(handle_chat_completions))
           // ... all existing proxy routes in current order ...
           ;

       // Merge web UI routes (they use State<Arc<ProxyState>> too)
       let web_routes = tama_web::router::build_web_routes();

       // Proxy routes first (higher priority), then web routes
       Router::new()
           .merge(proxy_routes)
           .merge(web_routes)
           .layer(CorsLayer::permissive())
           .layer(CatchPanicLayer::new())
           .with_state(state)
   }
   ```
   
   **Note:** Both proxy and web routes use `State<Arc<ProxyState>>`, so a single `.with_state(state)` at the end works for both.

2. The existing `build_router` function remains for non-web-ui builds (gated on `#[cfg(not(feature = "web-ui"))]`).

3. In `server/mod.rs` (`ProxyServer`):
   - `into_router()` calls `build_unified_router` when `web-ui` is enabled, `build_router` otherwise
   - `ProxyServer::new()` creates `JobManager` and `CapabilitiesCache` when `web-ui` is enabled, stores in `ProxyState` web fields
   - `run()` method simplified — no web server spawn, no shutdown channel

**Steps:**
- [ ] In `router.rs`, add `#[cfg(feature = "web-ui")]` import for `tama_web::router`
- [ ] Create `build_unified_router` function with correct route ordering
  - ALL proxy routes defined first
  - Web routes merged after via `.merge(tama_web::router::build_web_routes())`
  - Single `.with_state(state)` at the end
- [ ] In `server/mod.rs`, update `into_router()`:
  ```rust
  pub fn into_router(self) -> axum::Router {
      #[cfg(feature = "web-ui")]
      {
          router::build_unified_router(self.state)
      }
      #[cfg(not(feature = "web-ui"))]
      {
          router::build_router(self.state)
      }
  }
  ```
- [ ] In `server/mod.rs` (`ProxyServer::new`), initialize web fields:
  ```rust
  #[cfg(feature = "web-ui")]
  {
      state.web_jobs = Some(Arc::new(tama_web::jobs::JobManager::new()));
      state.web_capabilities = Some(Arc::new(tama_web::api::backends::CapabilitiesCache::new()));
  }
  ```
- [ ] In `server/mod.rs`, simplify `run()` method:
  - Remove `shutdown_tx` parameter (keep signature for now with `#[allow(unused_variables)]` if needed by callers)
  - Remove the `#[cfg(feature = "web-ui")]` block that spawns web server
  - Remove the `#[cfg(not(feature = "web-ui"))]` block — single code path
  - Remove web handle join logic
- [ ] Add routing regression test in `server/mod.rs` tests:
  ```rust
  #[cfg(feature = "web-ui")]
  #[tokio::test]
  async fn test_unified_router_route_priority() {
      // Verify proxy-specific routes match before web catch-alls
      // Test: /tama/v1/models/test/load returns 200 (proxy handler)
      // Test: /tama/v1/models/test returns 200 (web handler)
      // Test: /tama/v1/logs returns 200 (proxy handler)
      // Test: /tama/v1/logs/llama_cpp returns 200 (web handler)
  }
  ```
- [ ] Run `cargo check --package tama-core --features web-ui`
  - Fix compilation errors
- [ ] Run `cargo check --package tama-core` (without web-ui)
  - Ensure non-web build still works
- [ ] Run `cargo test --package tama-core --features web-ui test_proxy_routes_exist`
  - Ensure existing tests still pass
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "feat: merge proxy and web UI into unified router"

**Acceptance criteria:**
- [ ] `build_unified_router` combines all proxy + web routes
- [ ] Route priority correct: proxy-specific routes defined before web catch-alls
- [ ] Single `.with_state(Arc<ProxyState>)` at the end
- [ ] `ProxyServer::run()` no longer spawns a web server
- [ ] `ProxyServer::new()` initializes web fields (jobs, capabilities)
- [ ] Routing regression test passes
- [ ] Both `--features web-ui` and no-feature builds compile
- [ ] Existing proxy tests still pass

---

### Task 4: Simplify tama-cli serve command and deprecate web command

**Context:**
The `tama-cli` serve command (`handlers/serve.rs`) currently has complex logic to spawn both the proxy server and the web UI server as separate tokio tasks, with shared shutdown channels. After the merge, it just starts the proxy server which includes everything. The `binary_version` and other web fields need to be set on `ProxyState` before creating the server.

**Files:**
- Modify: `crates/tama-cli/src/handlers/serve.rs`
- Modify: `crates/tama-cli/src/cli.rs`
- Modify: `crates/tama-cli/src/handlers/web.rs`
- Modify: `crates/tama-cli/src/handlers/mod.rs`
- Modify: `crates/tama-cli/src/main.rs` (or wherever web command is dispatched)
- Modify: `crates/tama-cli/Cargo.toml`

**What to implement:**

1. **`handlers/serve.rs`:**
   ```rust
   pub async fn cmd_serve(
       config: &Config,
       host: String,
       port: u16,
       auto_unload: bool,
       idle_timeout: u64,
   ) -> Result<()> {
       // Apply CLI overrides to config
       let mut updated_config = config.clone();
       updated_config.proxy.host = host.clone();
       updated_config.proxy.port = port;
       updated_config.proxy.auto_unload = auto_unload;
       updated_config.proxy.idle_timeout_secs = idle_timeout;

       setup_hf_token(&updated_config);

       let db_dir = tama_core::config::Config::config_dir().ok();
       // ... backfill logic (unchanged) ...

       let state = Arc::new(ProxyState::new(updated_config.clone(), db_dir));

       // Set web-specific fields on ProxyState
       #[cfg(feature = "web-ui")]
       {
       let mut state_inner = state.as_ref().clone();
       // Actually, ProxyState fields aren't mutable after creation.
       // Need to either: make fields mutable, or pass version to ProxyState::new,
       // or set fields before Arc::new.
       // Best approach: set fields on the state before wrapping in Arc.
       }

       // Create and run proxy server
       let server = ProxyServer::new(state.clone()).await;
       server.run(addr, None).await?;
       Ok(())
   }
   ```
   
   **Problem:** `ProxyState` fields are set in `ProxyState::new()` and the state is wrapped in `Arc` immediately. The `web_binary_version` needs to be set from the CLI (`env!("CARGO_PKG_VERSION")`).
   
   **Solution:** Add a `with_web_options` builder method or pass version to `ProxyState::new`. Simplest: add a method:
   ```rust
   impl ProxyState {
       #[cfg(feature = "web-ui")]
       pub fn set_binary_version(&mut self, version: String) {
           self.web_binary_version = version;
       }
   }
   ```
   Call before `Arc::new` or use `Arc::get_mut()` immediately after.

2. **`cli.rs`:** Deprecate the `Web` command:
   - Add `#[command(about = "Deprecated, use `tama serve` instead")]
   - Keep `#[cfg(feature = "web-ui")]`
   - Web command calls `cmd_serve` with default port and no proxy_url

3. **`handlers/web.rs`:** Simplify to call `cmd_serve` directly or print deprecation and exit.

4. **`Cargo.toml`:** Update web-ui feature:
   ```toml
   web-ui = ["dep:tama-web", "tama-core/web-ui"]
   ```

**Steps:**
- [ ] Add `ProxyState::set_binary_version()` method (or similar) for CLI to set version
- [ ] Simplify `start_proxy_server` in `serve.rs` to a single code path
- [ ] Remove all `#[cfg(feature = "web-ui")]` branches from `serve.rs`
- [ ] Remove web server spawn, shutdown channel, and web handle join logic
- [ ] Remove `proxy_base_url`, `web_addr`, `logs_dir`, `config_path` local variables
- [ ] Set `web_binary_version` on ProxyState from `env!("CARGO_PKG_VERSION")`
- [ ] Update `cli.rs` to deprecate the `Web` command
- [ ] Simplify `handlers/web.rs` to call `cmd_serve` with defaults
- [ ] Update `Cargo.toml` web-ui feature to include `tama-core/web-ui`
- [ ] Check `main.rs` for web command dispatch and update
- [ ] Run `cargo check --package tama --features web-ui`
- [ ] Run `cargo check --package tama` (without web-ui)
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: "refactor: simplify serve command for unified server"

**Acceptance criteria:**
- [ ] `tama serve` starts a single server with proxy + web UI
- [ ] No `#[cfg(feature = "web-ui")]` branches in `serve.rs`
- [ ] `binary_version` is set on ProxyState from CLI
- [ ] `tama web` command is deprecated (hidden or with warning)
- [ ] Both feature flags compile correctly

---

### Task 5: Update tests, cleanup, and full verification

**Context:**
After the merge, tests that create `AppState` instances need to create `ProxyState` instances instead. The `tama-web` tests that start the old `run_with_opts` server need to use the unified router. The `tama-core` proxy tests need to verify the unified router works correctly.

**Files:**
- Modify: `crates/tama-web/tests/server_test.rs`
- Modify: `crates/tama-web/tests/config_structured_test.rs`
- Modify: `crates/tama-web/tests/downloads_api.rs`
- Modify: `crates/tama-web/src/api/backends/manage.rs` (test fixtures)
- Modify: `crates/tama-core/src/proxy/server/mod.rs` (existing tests)
- Modify: `crates/tama-core/src/proxy/server/listener.rs` (signature cleanup)

**What to implement:**

1. **`tama-web/tests/`:** All tests create `AppState` with `proxy_base_url`, etc. Replace with `ProxyState`:
   ```rust
   // Old:
   let state = Arc::new(AppState {
       proxy_base_url: "http://127.0.0.1:11434".to_string(),
       // ... all fields ...
   });
   let app = build_router(state);

   // New:
   let config = Config::default();
   let state = Arc::new(ProxyState::new(config, None));
   let app = build_unified_router(state);
   ```

2. **`tama-core` proxy tests:** The existing tests use `server.into_router()` which now calls `build_unified_router` when `web-ui` is enabled. Verify they still pass. The routing regression test added in Task 3 should catch any route priority issues.

3. **Listener signature:** The `listener::run` function takes `shutdown_tx: Option<watch::Sender<()>>`. This is no longer needed for web UI coordination. Keep the parameter for now (it's harmless) but document that it's unused.

4. **Backend test fixtures:** In `api/backends/manage.rs`, update test fixtures that create `AppState` to create `ProxyState`.

**Steps:**
- [ ] Update `tama-web/tests/server_test.rs` to use `ProxyState` and unified router
- [ ] Update `tama-web/tests/config_structured_test.rs` to use `ProxyState`
- [ ] Update `tama-web/tests/downloads_api.rs` to use `ProxyState`
- [ ] Update test fixtures in `api/backends/manage.rs`
- [ ] Run `cargo test --workspace --features web-ui`
  - Fix any test failures systematically
- [ ] Run `cargo test --workspace` (without web-ui)
  - Ensure non-web tests pass
- [ ] Run `cargo clippy --workspace --features web-ui -- -D warnings`
  - Fix any warnings
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Fix any warnings
- [ ] Run `cargo fmt --all -- --check`
  - Fix any formatting issues
- [ ] Manual smoke test:
  ```bash
  cargo run --features web-ui -- serve --port 11434
  # In another terminal:
  curl http://127.0.0.1:11434/health           # 200 OK
  curl http://127.0.0.1:11434/v1/models        # OpenAI models
  curl http://127.0.0.1:11434/tama/v1/models   # Tama models
  curl http://127.0.0.1:11434/                 # HTML (web UI)
  ```
- [ ] Commit with message: "test: update tests for unified server architecture"

**Acceptance criteria:**
- [ ] All workspace tests pass with `--features web-ui`
- [ ] All workspace tests pass without features
- [ ] Clippy clean with `-D warnings` in both builds
- [ ] `cargo fmt` clean
- [ ] Smoke test: all 4 endpoints return expected responses on single port

---

## Verification

After all tasks are complete:

1. **Build verification:**
   ```bash
   cargo build --workspace --features web-ui
   cargo build --workspace  # without web-ui
   ```

2. **Test verification:**
   ```bash
   cargo test --workspace --features web-ui
   cargo test --workspace
   ```

3. **Lint verification:**
   ```bash
   cargo clippy --workspace --features web-ui -- -D warnings
   cargo clippy --workspace -- -D warnings
   cargo fmt --all -- --check
   ```

4. **Smoke test:**
   ```bash
   cargo run --features web-ui -- serve --port 11434
   # Visit http://127.0.0.1:11434 — web UI should load
   # curl http://127.0.0.1:11434/health — should return 200
   # curl http://127.0.0.1:11434/v1/models — should return OpenAI models
   # curl http://127.0.0.1:11434/tama/v1/models — should return Tama models
   ```

5. **No regressions:**
   - OpenAI-compatible clients still work on the same port
   - Web UI loads and all features work (models, backends, benchmarks, config, downloads)
   - SSE streams work (metrics, downloads, jobs, logs)
   - CSRF protection still works
   - `tama serve --host 0.0.0.0 --port 11434` works (bind to all interfaces)
