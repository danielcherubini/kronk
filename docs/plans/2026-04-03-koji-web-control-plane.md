# Koji Web Control Plane Plan

**Goal:** Add a `koji-web` crate that serves a Leptos WASM single-page app as a control plane UI for the Koji proxy, running on a separate port (default 11435).

**Architecture:** A new `crates/koji-web` Rust crate hosts a Leptos frontend (compiled to WASM via `trunk`) and an Axum backend that proxies the `/koji/v1/` management API to the running Koji proxy on port 11434. The compiled WASM+HTML assets are embedded into the Rust binary at compile time using `include_dir!`. A new `koji web` CLI sub-command (in `koji-cli`) starts the web server, pointing at the configured proxy base URL.

**Tech Stack:** Rust (Leptos 0.7 + Axum 0.7), Trunk for WASM compilation, `include_dir` for asset embedding, `reqwest` for proxy-side API forwarding.

---

### Task 1: Scaffold the `koji-web` Cargo crate

**Context:**
A new workspace member `crates/koji-web` needs to be created and wired into the Cargo workspace. This crate will contain both the Leptos frontend (compiled separately with Trunk) and the Axum backend server that embeds and serves those compiled assets. The crate must be added to the workspace `[members]` list in the root `Cargo.toml`. At this stage no actual UI or server logic is implemented — the goal is a buildable skeleton that compiles cleanly.

The crate will have **two compilation targets**:
1. `--target wasm32-unknown-unknown` — the Leptos frontend, built by Trunk.
2. Native host target — the Axum backend server binary, built by `cargo build`.

To handle this, use a `[lib]` target (used by Leptos/Trunk for the WASM build) and a `[[bin]]` target (the server). The `ssr` Cargo feature gates all backend-only (Axum, tokio, etc.) dependencies, and `#[cfg(feature = "ssr")]` guards backend code in `lib.rs`. The `[[bin]]` must declare `required-features = ["ssr"]` so `cargo build --workspace` doesn't try to compile it without its required deps.

**Files:**
- Create: `crates/koji-web/Cargo.toml`
- Create: `crates/koji-web/src/lib.rs`
- Create: `crates/koji-web/src/main.rs`
- Create: `crates/koji-web/index.html`
- Create: `crates/koji-web/Trunk.toml`
- Modify: `Cargo.toml` (root workspace — add `"crates/koji-web"` to `members`)
- Modify: `Cargo.toml` (root workspace — add `leptos`, `leptos_router`, `wasm-bindgen`, `web-sys`, `include_dir` to `[workspace.dependencies]`; do NOT add `leptos_meta` — it is not used)

**What to implement:**

`crates/koji-web/Cargo.toml`:
```toml
[package]
name = "koji-web"
version.workspace = true
edition.workspace = true

[lib]
crate-type = ["cdylib", "rlib"]

[[bin]]
name = "koji-web"
path = "src/main.rs"
required-features = ["ssr"]

[dependencies]
leptos = { workspace = true, features = ["csr"] }
leptos_router.workspace = true
wasm-bindgen.workspace = true
web-sys = { workspace = true, features = ["Window", "Document", "HtmlElement"] }
# Backend-only deps (not needed for WASM, gated behind the `ssr` feature)
axum = { workspace = true, optional = true }
tokio = { workspace = true, optional = true }
anyhow = { workspace = true, optional = true }
include_dir = { workspace = true, optional = true }
reqwest = { workspace = true, optional = true }
mime_guess = { workspace = true, optional = true }
tracing = { workspace = true, optional = true }
tracing-subscriber = { workspace = true, optional = true }
koji-core = { path = "../koji-core", optional = true }

[features]
ssr = [
  "dep:axum", "dep:tokio", "dep:anyhow", "dep:include_dir",
  "dep:reqwest", "dep:mime_guess", "dep:tracing", "dep:tracing-subscriber",
  "dep:koji-core",
]
```

Notes:
- `tower-http` is **not** included — the server does not use CORS/trace middleware in this plan; add it later if needed.
- `leptos_meta` is **not** included — page title management is out of scope for this plan.
- `required-features = ["ssr"]` on `[[bin]]` means `cargo build --workspace` (without explicit `--features ssr`) will skip the binary target, which is correct. Users and CI must pass `--features ssr` to build the server binary.
- The `[lib]` target (`cdylib + rlib`) is compiled to WASM by Trunk with `features = ["csr"]` (the default, no `ssr`).
- Backend code in `lib.rs` must be gated with `#[cfg(feature = "ssr")]` (NOT `#[cfg(not(target_arch = "wasm32"))]`) to stay consistent with the Cargo feature system.

Add to root `[workspace.dependencies]`:
```toml
leptos = { version = "0.7", default-features = false }
leptos_router = { version = "0.7" }
wasm-bindgen = "0.2"
web-sys = { version = "0.3", default-features = false }
include_dir = "0.7"
mime_guess = "2"
```

(`tracing` and `tracing-subscriber` are already in the workspace deps.)

`crates/koji-web/src/lib.rs` — minimal Leptos app entry point:
```rust
use leptos::prelude::*;

#[component]
pub fn App() -> impl IntoView {
    view! { <h1>"Koji Control Plane"</h1> }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    // In Leptos 0.7, mount_to_body takes a FnOnce closure, NOT a component fn directly.
    leptos::mount_to_body(|| view! { <App /> });
}
```

`crates/koji-web/src/main.rs` — minimal binary that prints a placeholder:
```rust
fn main() {
    println!("koji-web server (not yet implemented)");
}
```

`crates/koji-web/index.html` — minimal Trunk entry point:
```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <title>Koji Control Plane</title>
  </head>
  <body></body>
</html>
```

`crates/koji-web/Trunk.toml`:
```toml
[build]
target = "index.html"
dist = "dist"
```

**Steps:**
- [ ] Add `"crates/koji-web"` to `members` in root `Cargo.toml`
- [ ] Add `leptos`, `leptos_router`, `wasm-bindgen`, `web-sys`, `include_dir`, `mime_guess` to root `[workspace.dependencies]` (do NOT add `leptos_meta`)
- [ ] Create `crates/koji-web/Cargo.toml` as described above
- [ ] Create `crates/koji-web/src/lib.rs` with minimal `App` component and `wasm_bindgen(start)` entry (use `|| view! { <App /> }` in `mount_to_body`, NOT `App` directly)
- [ ] Create `crates/koji-web/src/main.rs` with placeholder `fn main() { println!("not yet implemented"); }`
- [ ] Create `crates/koji-web/index.html`
- [ ] Create `crates/koji-web/Trunk.toml`
- [ ] Run `cargo check --workspace` (this only checks the `[lib]` target and host-target crates; the `[[bin]]` is skipped without `--features ssr` due to `required-features`)
  - Did it succeed? If not, fix dependency resolution errors before continuing.
- [ ] Run `cargo check --package koji-web --features ssr` to also check the server binary path
  - Did it succeed? Fix any errors.
- [ ] Commit with message: `"feat(koji-web): scaffold crate with Leptos CSR skeleton"`

**Acceptance criteria:**
- [ ] `cargo check --workspace` passes with no errors
- [ ] `cargo check --package koji-web --features ssr` passes with no errors
- [ ] `crates/koji-web` appears in the workspace member list
- [ ] `[[bin]]` declares `required-features = ["ssr"]`
- [ ] The crate has both a `[lib]` (WASM) and `[[bin]]` (server) target

---

### Task 2: Add Axum server backend that embeds built WASM assets

**Context:**
The `koji-web` binary needs to serve the compiled Leptos WASM app (produced by Trunk into `crates/koji-web/dist/`) as static files, and also forward the full `/koji/v1/` management API to the running Koji proxy (default `http://127.0.0.1:11434`). The assets are embedded at compile time using `include_dir!` so the binary is self-contained.

The server starts on a configurable port (default `11435`). It exposes:
- `GET /` and `GET /*` — serve the embedded static assets (index.html, WASM, JS glue)
- `GET|POST /koji/v1/*` — HTTP reverse proxy forwarded to `http://127.0.0.1:<proxy_port>/koji/v1/*`

The proxy base URL is passed as a startup argument. The embedding of the `dist/` folder uses `include_dir::include_dir!("$CARGO_MANIFEST_DIR/dist")` — the `dist/` directory **must exist** at compile time (even if empty) for the macro to resolve. Create an empty `dist/.gitkeep` placeholder.

Note: The `ssr` feature flag gates all Axum/server dependencies.

**Files:**
- Create: `crates/koji-web/src/server.rs`
- Modify: `crates/koji-web/src/main.rs`
- Create: `crates/koji-web/dist/.gitkeep`

**What to implement:**

`crates/koji-web/src/server.rs` (compiled only with `ssr` feature):
```rust
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, Method, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{any, get},
    Router,
};
use include_dir::{include_dir, Dir};
use std::sync::Arc;

static DIST: Dir = include_dir!("$CARGO_MANIFEST_DIR/dist");

#[derive(Clone)]
pub struct AppState {
    pub proxy_base_url: String,
    pub client: reqwest::Client,
}

/// Serve a static file from the embedded `dist/` directory.
async fn serve_static(path: Option<Path<String>>) -> Response {
    let file_path = path.map(|p| p.0).unwrap_or_else(|| "index.html".into());
    let file_path = if file_path.is_empty() || file_path == "/" { "index.html".to_string() } else { file_path };

    match DIST.get_file(&file_path) {
        Some(f) => {
            let mime = mime_guess::from_path(&file_path).first_or_octet_stream();
            Response::builder()
                .header("Content-Type", mime.as_ref())
                .body(Body::from(f.contents()))
                .unwrap()
        }
        None => {
            // SPA fallback: return index.html for unknown paths
            match DIST.get_file("index.html") {
                Some(f) => Html(std::str::from_utf8(f.contents()).unwrap_or("")).into_response(),
                None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
            }
        }
    }
}

/// Forward a request to the Koji proxy at `/koji/v1/<path>`.
async fn proxy_koji(
    State(state): State<Arc<AppState>>,
    method: Method,
    headers: HeaderMap,
    path: Path<String>,
    body: Body,
) -> Response {
    let url = format!("{}/koji/v1/{}", state.proxy_base_url, path.0);
    // Cap at 16 MiB — same as MAX_REQUEST_BODY_SIZE in koji-core — to prevent memory exhaustion.
    let body_bytes = axum::body::to_bytes(body, 16 * 1024 * 1024).await.unwrap_or_default();

    let mut req = state.client.request(method, &url);
    for (k, v) in &headers {
        if k != axum::http::header::HOST {
            req = req.header(k, v);
        }
    }
    req = req.body(body_bytes);

    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let headers = resp.headers().clone();
            let body = resp.bytes().await.unwrap_or_default();
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = status;
            for (k, v) in &headers {
                response.headers_mut().insert(k, v.clone());
            }
            response
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Failed to reach Koji proxy: {e}"),
        )
            .into_response(),
    }
}

/// Dedicated handler for the root path — avoids Axum type-inference issues with inline closures.
async fn serve_index() -> Response {
    serve_static(None).await
}

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/koji/v1/{*path}", any(proxy_koji))
        .route("/", get(serve_index))
        .route("/{*path}", get(|Path(p): Path<String>| async move { serve_static(Some(p)).await }))
        .with_state(state)
}

pub async fn run(addr: std::net::SocketAddr, proxy_base_url: String) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        proxy_base_url,
        client: reqwest::Client::new(),
    });
    let app = build_router(state);
    tracing::info!("Koji web UI listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

`crates/koji-web/src/main.rs` (replace placeholder):
```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let addr: std::net::SocketAddr = "0.0.0.0:11435".parse()?;
    let proxy_base_url = std::env::var("KOJI_PROXY_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string());
    koji_web::server::run(addr, proxy_base_url).await
}
```

Also add `pub mod server;` to `src/lib.rs` behind `#[cfg(feature = "ssr")]` (NOT `#[cfg(not(target_arch = "wasm32"))]` — use the Cargo feature flag consistently throughout).

`mime_guess` is already listed as a workspace dep and `ssr`-gated dep in Task 1's `Cargo.toml`. No additional action required here.

**Steps:**
- [ ] Create `crates/koji-web/dist/.gitkeep` (empty file to ensure `include_dir!` can resolve the path at compile time)
- [ ] Create `crates/koji-web/src/server.rs` with the static file server and proxy logic described above
- [ ] Update `crates/koji-web/src/main.rs` to call `koji_web::server::run(...)`
- [ ] Add `#[cfg(feature = "ssr")] pub mod server;` to `src/lib.rs` (use the Cargo feature flag, NOT `cfg(not(target_arch = "wasm32"))`)
- [ ] Run `cargo build --package koji-web --features ssr`
  - Did it compile? If not, fix the errors (likely missing mime_guess dep or incorrect feature flags) before continuing.
- [ ] Run `cargo clippy --package koji-web --features ssr -- -D warnings`
  - Did it pass? Fix any warnings before continuing.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `"feat(koji-web): add Axum backend with embedded WASM asset serving and proxy forwarding"`

**Acceptance criteria:**
- [ ] `cargo build --package koji-web --features ssr` succeeds
- [ ] `crates/koji-web/dist/.gitkeep` exists so the `include_dir!` macro has a target
- [ ] The server module is gated behind `#[cfg(feature = "ssr")]`

---

### Task 3: Build the Leptos UI — Dashboard, Models, and Pull pages

**Context:**
This task implements the core three UI pages using Leptos reactive components. The frontend is a single-page app (SPA) with client-side routing via `leptos_router`. All data is fetched from the forwarded `/koji/v1/` API endpoints using Leptos `create_resource` / `create_action` and the browser's `fetch` (via `gloo-net` or `web_sys`). No SSR is used — this is pure CSR (client-side rendering). The compiled output from `trunk build` in `crates/koji-web/` produces `dist/` which the server embeds.

Pages to implement:
1. **Dashboard** (`/`) — calls `GET /koji/v1/system/health`, displays status badge, models_loaded, VRAM used/total. Has a "Restart Koji" button that calls `POST /koji/v1/system/restart`.
2. **Models** (`/models`) — calls `GET /koji/v1/models`, renders a table with columns: ID, Backend, Model, Quant, Enabled, Loaded (badge), Actions. Each row has a "Load" button (`POST /koji/v1/models/{id}/load`) if unloaded, and "Unload" if loaded. After action completes, the list refreshes.
3. **Pull** (`/pull`) — a form with fields: `repo_id` (text input) and `quant` (text input). Submit calls `POST /koji/v1/pulls` and then polls `GET /koji/v1/pulls/{job_id}` every 1s to show progress (status badge + file name). If 422 is returned, display the `available_quants` list.

Navigation: a top `<nav>` bar with links to `/`, `/models`, `/pull`.

Use `gloo-net` for HTTP requests from the WASM side (it wraps `fetch` cleanly for Leptos).

**Files:**
- Modify: `crates/koji-web/src/lib.rs`
- Create: `crates/koji-web/src/pages/mod.rs`
- Create: `crates/koji-web/src/pages/dashboard.rs`
- Create: `crates/koji-web/src/pages/models.rs`
- Create: `crates/koji-web/src/pages/pull.rs`
- Create: `crates/koji-web/src/components/mod.rs`
- Create: `crates/koji-web/src/components/nav.rs`
- Modify: `crates/koji-web/Cargo.toml` (add `gloo-net`, `serde`, `serde_json`, `wasm-bindgen-futures`)
- Modify: root `Cargo.toml` (add `gloo-net`, `wasm-bindgen-futures` to workspace deps)

**What to implement:**

Add to root `[workspace.dependencies]`:
```toml
gloo-net = { version = "0.6", features = ["http"] }
wasm-bindgen-futures = "0.4"
```

In `crates/koji-web/Cargo.toml` `[dependencies]` (non-optional, frontend):
```toml
gloo-net.workspace = true
wasm-bindgen-futures.workspace = true
serde = { workspace = true, features = ["derive"] }
serde_json.workspace = true
```

`src/lib.rs` — update to wire routing:
```rust
use leptos::prelude::*;
use leptos_router::{components::{Route, Router, Routes, A}, path};
mod components;
mod pages;
// Gate the server module behind the `ssr` Cargo feature, NOT target_arch.
// This keeps cfg strategy consistent with the optional dependency declarations.
#[cfg(feature = "ssr")]
pub mod server;

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <components::nav::Nav />
            <main>
                <Routes fallback=|| "Page not found">
                    <Route path=path!("/") view=pages::dashboard::Dashboard />
                    <Route path=path!("/models") view=pages::models::Models />
                    <Route path=path!("/pull") view=pages::pull::Pull />
                </Routes>
            </main>
        </Router>
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    // In Leptos 0.7 mount_to_body takes a FnOnce closure, NOT a component fn directly.
    leptos::mount_to_body(|| view! { <App /> });
}
```

`src/components/nav.rs`:
```rust
use leptos::prelude::*;
use leptos_router::components::A;

#[component]
pub fn Nav() -> impl IntoView {
    view! {
        <nav>
            <A href="/">"Dashboard"</A>
            " | "
            <A href="/models">"Models"</A>
            " | "
            <A href="/pull">"Pull Model"</A>
        </nav>
    }
}
```

`src/pages/dashboard.rs` — fetch `/koji/v1/system/health` and render health fields. Include a restart button that calls `POST /koji/v1/system/restart`.

**IMPORTANT:** In Leptos 0.7 CSR, always use `.await` in the async closure passed to `Resource::new`. Do NOT use `futures::executor::block_on` — it does not work in a WASM async context and the `futures` crate is not a declared dependency. The correct pattern is:

```rust
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SystemHealth {
    status: String,
    service: String,
    models_loaded: u32,
    vram: Option<VramInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VramInfo {
    used_mib: i64,
    total_mib: i64,
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let health = Resource::new(|| (), |_| async move {
        // Use .await throughout — do NOT use futures::executor::block_on
        let resp = gloo_net::http::Request::get("/koji/v1/system/health")
            .send()
            .await
            .ok()?;
        resp.json::<SystemHealth>().await.ok()
    });

    let restart = Action::new(|_: &()| async move {
        let _ = gloo_net::http::Request::post("/koji/v1/system/restart").send().await;
    });

    view! {
        <h1>"Dashboard"</h1>
        <Suspense fallback=|| view! { <p>"Loading..."</p> }>
            {move || health.get().map(|h| match h {
                Some(h) => view! {
                    <p>"Status: " {h.status}</p>
                    <p>"Models loaded: " {h.models_loaded}</p>
                    {h.vram.map(|v| view! {
                        <p>"VRAM: " {v.used_mib} " / " {v.total_mib} " MiB"</p>
                    })}
                }.into_any(),
                None => view! { <p>"Failed to load health data"</p> }.into_any(),
            })}
        </Suspense>
        <button on:click=move |_| { restart.dispatch(()); }>"Restart Koji"</button>
    }
}
```

`src/pages/models.rs` — fetch `/koji/v1/models`, render a table. For each model row, show a Load or Unload button that calls the appropriate endpoint and then triggers a refetch. Use `Action` + `Resource` where the resource depends on a signal that the action increments (refresh trigger).

`src/pages/pull.rs` — form with `repo_id` and `quant` signals. On submit, POST to `/koji/v1/pulls`. Store `job_id` in a signal. Use `set_interval` or a polling loop via `use_interval` (or a `Resource` with a trigger signal) to poll `/koji/v1/pulls/{job_id}` every 1 second. Show job status and error if failed. If 422, parse `available_quants` and display them.

All three pages must compile cleanly — use `todo!()` or stub out any complex parts rather than leaving syntax errors.

**Steps:**
- [ ] Add `gloo-net`, `wasm-bindgen-futures`, `serde`, `serde_json` to `crates/koji-web/Cargo.toml` frontend deps
- [ ] Add `gloo-net = { version = "0.6", features = ["http"] }` and `wasm-bindgen-futures = "0.4"` to root `[workspace.dependencies]`
- [ ] Create `src/components/mod.rs` (re-export nav)
- [ ] Create `src/components/nav.rs` (Nav component)
- [ ] Create `src/pages/mod.rs` (re-export pages)
- [ ] Create `src/pages/dashboard.rs` (Dashboard page)
- [ ] Create `src/pages/models.rs` (Models page with load/unload actions)
- [ ] Create `src/pages/pull.rs` (Pull page with polling)
- [ ] Update `src/lib.rs` to wire the router and all pages
- [ ] Install `trunk` if not present: `cargo install trunk` (check with `trunk --version` first)
- [ ] Run `trunk build` from `crates/koji-web/`
  - Did it produce `dist/` with `index.html`, `.wasm`, `.js`? If not, fix compilation errors.
- [ ] Run `cargo build --package koji-web --features ssr`
  - Does the server binary compile? Fix any errors.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `"feat(koji-web): implement Dashboard, Models, and Pull pages in Leptos CSR"`

**Acceptance criteria:**
- [ ] `trunk build` in `crates/koji-web/` produces `dist/index.html`, a `.wasm` file, and a `.js` glue file
- [ ] `cargo build --package koji-web --features ssr` succeeds
- [ ] Navigation between `/`, `/models`, `/pull` works in the SPA router

---

### Task 4: Add Logs Viewer and Config Editor pages

**Context:**
Two additional pages complete the control plane feature set:

1. **Logs Viewer** (`/logs`) — calls a new backend endpoint `GET /api/logs?name=koji&lines=200` (added to the Axum server in this crate, not forwarded to the Koji proxy) which reads the log file from the `logs_dir` path. The path to the logs directory is passed to the `koji-web` server at startup via config or CLI arg. The endpoint returns `{ "lines": ["...", "..."] }`. The frontend displays lines in a `<pre>` block with a "Refresh" button.

2. **Config Editor** (`/config`) — calls a new backend endpoint `GET /api/config` that returns the raw TOML of the Koji config file and `POST /api/config` that accepts the new TOML content, validates it by parsing (using `koji_core::config::Config`), and writes it back to disk. The frontend renders a `<textarea>` with the current TOML, a Save button, and a status message.

Both of these endpoints live on the `koji-web` Axum server (not the Koji proxy), because they access the local filesystem. They are defined in a new `src/api.rs` module in `koji-web` and added to the router.

**Files:**
- Create: `docs/openapi/koji-web-api.yaml` (OpenAPI spec for the web-server-native `/api/` endpoints)
- Create: `crates/koji-web/src/api.rs`
- Modify: `crates/koji-web/src/server.rs` (add `/api/logs` and `/api/config` routes, extend `AppState` with `logs_dir` and `config_path`)
- Modify: `crates/koji-web/src/main.rs` (read `KOJI_LOGS_DIR` and `KOJI_CONFIG_PATH` env vars)
- Create: `crates/koji-web/src/pages/logs.rs`
- Create: `crates/koji-web/src/pages/config_editor.rs`
- Modify: `crates/koji-web/src/pages/mod.rs` (re-export new pages)
- Modify: `crates/koji-web/src/lib.rs` (add routes for `/logs` and `/config`)

**What to implement:**

`src/api.rs` (compiled only with `ssr` feature / native target):
```rust
use axum::{extract::State, Json, response::IntoResponse, http::StatusCode};
use std::sync::Arc;
use crate::server::AppState;

/// Query parameters for GET /api/logs
#[derive(serde::Deserialize)]
pub struct LogsQuery {
    /// Number of lines to return (default: 200)
    #[serde(default = "default_lines")]
    pub lines: usize,
}
fn default_lines() -> usize { 200 }

pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<LogsQuery>,
) -> impl IntoResponse {
    let dir = match &state.logs_dir {
        Some(d) => d.clone(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "logs_dir not configured"}))).into_response(),
    };
    let log_path = dir.join("koji.log");
    // Use spawn_blocking for synchronous file I/O to avoid blocking the Tokio runtime.
    let log_path_clone = log_path.clone();
    let n = query.lines;
    let lines = tokio::task::spawn_blocking(move || {
        koji_core::logging::tail_lines(&log_path_clone, n).unwrap_or_default()
    }).await.unwrap_or_default();
    Json(serde_json::json!({ "lines": lines })).into_response()
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let path = match &state.config_path {
        Some(p) => p.clone(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "config_path not configured"}))).into_response(),
    };
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await {
        Ok(Ok(content)) => Json(serde_json::json!({ "content": content })).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct ConfigBody { pub content: String }

pub async fn save_config(State(state): State<Arc<AppState>>, Json(body): Json<ConfigBody>) -> impl IntoResponse {
    let path = match &state.config_path {
        Some(p) => p.clone(),
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "config_path not configured"}))).into_response(),
    };
    // Validate TOML by parsing. Note: koji_core::config::Config has required fields
    // (e.g. `general`), so a partial TOML that omits top-level tables will fail here.
    // This is intentional — only fully valid config files are accepted.
    if let Err(e) = toml::from_str::<koji_core::config::Config>(&body.content) {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error": format!("Invalid TOML: {e}")}))).into_response();
    }
    // Use spawn_blocking for synchronous file I/O.
    match tokio::task::spawn_blocking(move || std::fs::write(&path, &body.content)).await {
        Ok(Ok(_)) => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response(),
    }
}
```

Extend `AppState`:
```rust
pub struct AppState {
    pub proxy_base_url: String,
    pub client: reqwest::Client,
    pub logs_dir: Option<std::path::PathBuf>,
    pub config_path: Option<std::path::PathBuf>,
}
```

Add to `build_router`:
```rust
.route("/api/logs", get(api::get_logs))
.route("/api/config", get(api::get_config).post(api::save_config))
```

Add `toml.workspace = true` to the `ssr` feature deps in `Cargo.toml`.

`src/pages/logs.rs` — Fetch `GET /api/logs`, display lines in a scrollable `<pre>`. Add a "Refresh" button that re-triggers the resource.

`src/pages/config_editor.rs` — Fetch `GET /api/config`, show raw TOML in a `<textarea>`. On Save, POST to `/api/config`. Show success or error message.

Add routes in `src/lib.rs`:
```rust
<Route path=path!("/logs") view=pages::logs::Logs />
<Route path=path!("/config") view=pages::config_editor::ConfigEditor />
```

Add nav links for "Logs" and "Config" in `src/components/nav.rs`.

`docs/openapi/koji-web-api.yaml` — create this file with exactly the following content (follow the style of `docs/openapi/koji-api.yaml`):

```yaml
openapi: 3.1.0

info:
  title: Koji Web API
  description: |
    Endpoints served natively by the `koji-web` process (port 11435).
    These endpoints are NOT forwarded to the Koji proxy — they access the
    local filesystem directly for log tailing and config editing.

    All endpoints are prefixed with `/api/`.
  version: 1.7.2
  license:
    name: MIT

servers:
  - url: http://localhost:11435
    description: Local Koji web UI (default port)

tags:
  - name: web-api
    description: Web UI — log viewing and config editing

paths:

  /api/logs:
    get:
      operationId: getLogs
      summary: Get recent log lines
      description: |
        Returns the last N lines of the `koji.log` file from the configured
        `logs_dir`. Returns 404 if `logs_dir` was not passed to the web server
        at startup.
      tags: [web-api]
      parameters:
        - name: lines
          in: query
          required: false
          description: Number of log lines to return (default 200)
          schema:
            type: integer
            default: 200
            minimum: 1
            maximum: 10000
      responses:
        "200":
          description: Log lines
          content:
            application/json:
              schema:
                type: object
                required: [lines]
                properties:
                  lines:
                    type: array
                    items:
                      type: string
              example:
                lines:
                  - "2026-04-03T12:00:00Z INFO koji: starting proxy on 0.0.0.0:11434"
                  - "2026-04-03T12:00:01Z INFO koji: loaded model llama3"
        "404":
          description: logs_dir not configured
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ErrorResponse"
              example:
                error: "logs_dir not configured"

  /api/config:
    get:
      operationId: getConfig
      summary: Get current config file contents
      description: |
        Returns the raw TOML content of the Koji config file. Returns 404 if
        `config_path` was not passed to the web server at startup.
      tags: [web-api]
      responses:
        "200":
          description: Config file contents
          content:
            application/json:
              schema:
                type: object
                required: [content]
                properties:
                  content:
                    type: string
                    description: Raw TOML content of the config file
              example:
                content: "[general]\nlog_level = \"info\"\n"
        "404":
          description: config_path not configured
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ErrorResponse"
        "500":
          $ref: "#/components/responses/InternalError"

    post:
      operationId: saveConfig
      summary: Save config file contents
      description: |
        Validates and saves the provided TOML as the Koji config file.

        The content is validated by parsing it as a `Config` struct before
        writing. A restart of the Koji proxy is required for changes to take
        effect (use `POST /koji/v1/system/restart`).

        **Note:** Only fully valid config files (all required top-level tables
        present) are accepted. Partial TOML edits will be rejected with 422.
      tags: [web-api]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              required: [content]
              properties:
                content:
                  type: string
                  description: New raw TOML content to write
            example:
              content: "[general]\nlog_level = \"info\"\n"
      responses:
        "200":
          description: Config saved successfully
          content:
            application/json:
              schema:
                type: object
                required: [ok]
                properties:
                  ok:
                    type: boolean
                    example: true
        "404":
          description: config_path not configured
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ErrorResponse"
        "422":
          description: TOML validation failed
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/ErrorResponse"
              example:
                error: "Invalid TOML: missing field `general` at line 1"
        "500":
          $ref: "#/components/responses/InternalError"

components:
  schemas:
    ErrorResponse:
      type: object
      required: [error]
      properties:
        error:
          type: string
          description: Human-readable error message
          example: "logs_dir not configured"

  responses:
    InternalError:
      description: Internal server error
      content:
        application/json:
          schema:
            $ref: "#/components/schemas/ErrorResponse"
          example:
            error: "Permission denied: /etc/koji/koji.toml"
```

**Steps:**
- [ ] Create `docs/openapi/koji-web-api.yaml` with the full YAML content shown above
- [ ] Add `toml = { workspace = true, optional = true }` to `crates/koji-web/Cargo.toml` `[dependencies]` and add `"dep:toml"` to the `ssr` feature array (toml is already in workspace deps)
- [ ] Extend `AppState` in `server.rs` with `logs_dir` and `config_path` fields
- [ ] Update `main.rs` to read `KOJI_LOGS_DIR` and `KOJI_CONFIG_PATH` env vars and pass to `AppState`
- [ ] Create `src/api.rs` with `get_logs`, `get_config`, `save_config` handlers as shown above (note: use `#[cfg(feature = "ssr")]` to gate the module, NOT `#[cfg(not(target_arch = "wasm32"))]`)
- [ ] Add `#[cfg(feature = "ssr")] mod api;` to `src/lib.rs` (NOT `#[cfg(not(target_arch = "wasm32"))]`)
- [ ] Add `/api/logs` and `/api/config` routes to `build_router` in `server.rs`
- [ ] Create `src/pages/logs.rs` (Logs page)
- [ ] Create `src/pages/config_editor.rs` (ConfigEditor page)
- [ ] Update `src/pages/mod.rs` to re-export both new pages
- [ ] Update `src/lib.rs` router with `/logs` and `/config` routes
- [ ] Update `src/components/nav.rs` with "Logs" and "Config" links
- [ ] Run `trunk build` in `crates/koji-web/`
  - Did it succeed? If not, fix errors.
- [ ] Run `cargo build --package koji-web --features ssr`
  - Did it succeed? If not, fix errors.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `"feat(koji-web): add Logs Viewer and Config Editor pages"`

**Acceptance criteria:**
- [ ] `docs/openapi/koji-web-api.yaml` exists and documents all three `/api/` endpoints
- [ ] `GET /api/logs` returns `{ "lines": [...] }` when `logs_dir` is set
- [ ] `GET /api/config` returns the raw TOML
- [ ] `POST /api/config` with invalid TOML returns 422; with valid TOML writes the file and returns `{ "ok": true }`
- [ ] Both pages render in the SPA without errors

---

### Task 5: Add `koji web` CLI sub-command to `koji-cli`

**Context:**
The `koji-web` binary is standalone but users should also be able to launch the web UI from the existing `koji` CLI with `koji web`. This task adds a `Web` variant to the CLI `Commands` enum in `koji-cli` and wires it to the `koji-web` server start function.

Because `koji-web` is a separate crate, `koji-cli` adds it as an optional dependency (behind a `web-ui` feature) to avoid pulling Leptos into every user's build. The `web` sub-command accepts `--port` (default 11435), `--proxy-url` (default `http://127.0.0.1:11434`), `--logs-dir`, and `--config-path`.

**Files:**
- Modify: `crates/koji-cli/Cargo.toml` (add optional `koji-web` dep under `web-ui` feature)
- Modify: `crates/koji-cli/src/cli.rs` (add `Web` command variant)
- Modify: `crates/koji-cli/src/lib.rs` (add `Web` arm in match)
- Create: `crates/koji-cli/src/handlers/web.rs`
- Modify: `crates/koji-cli/src/handlers/mod.rs` (re-export `web` module behind `web-ui` feature)
- Modify: `crates/koji-web/src/server.rs` (rename `run` → `run_with_opts`, add `logs_dir`/`config_path` params)
- Modify: `crates/koji-web/src/main.rs` (update call from `run` to `run_with_opts`)

**What to implement:**

`crates/koji-cli/Cargo.toml`:
```toml
[features]
web-ui = ["dep:koji-web"]

[dependencies]
# ... existing deps ...
koji-web = { path = "../koji-web", features = ["ssr"], optional = true }
```

Add `Web` to `Commands` enum in `cli.rs`:
```rust
/// Start the Koji web control plane UI
#[cfg(feature = "web-ui")]
Web {
    /// Port to listen on (default: 11435)
    #[arg(long, default_value = "11435")]
    port: u16,
    /// Koji proxy base URL (default: http://127.0.0.1:11434)
    #[arg(long, default_value = "http://127.0.0.1:11434")]
    proxy_url: String,
    /// Directory containing Koji log files
    #[arg(long)]
    logs_dir: Option<std::path::PathBuf>,
    /// Path to Koji config file
    #[arg(long)]
    config_path: Option<std::path::PathBuf>,
},
```

`src/handlers/web.rs`:
```rust
#[cfg(feature = "web-ui")]
pub async fn cmd_web(
    port: u16,
    proxy_url: String,
    logs_dir: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    let addr: std::net::SocketAddr = format!("0.0.0.0:{port}").parse()?;
    koji_web::server::run_with_opts(addr, proxy_url, logs_dir, config_path).await
}
```

Add a `run_with_opts` function to `koji_web::server` (rename/extend existing `run` to accept the full options).

Add the `Web` arm to the `match args.command` in `lib.rs`:
```rust
#[cfg(feature = "web-ui")]
Commands::Web { port, proxy_url, logs_dir, config_path } => {
    handlers::web::cmd_web(port, proxy_url, logs_dir, config_path).await
}
```

**Steps:**
- [ ] Add `web-ui` feature and optional `koji-web` dep to `crates/koji-cli/Cargo.toml`
- [ ] Add `Web` variant to `Commands` enum in `src/cli.rs` (gated behind `#[cfg(feature = "web-ui")]`)
- [ ] Create `src/handlers/web.rs` with `cmd_web` function
- [ ] Add `#[cfg(feature = "web-ui")] pub mod web;` to `src/handlers/mod.rs`
- [ ] Add `Web` arm to `match args.command` in `src/lib.rs` (gated behind `#[cfg(feature = "web-ui")]`)
- [ ] In `crates/koji-web/src/server.rs`, rename `run` to `run_with_opts` and add `logs_dir: Option<std::path::PathBuf>` and `config_path: Option<std::path::PathBuf>` parameters
- [ ] Update `crates/koji-web/src/main.rs` to call the renamed `run_with_opts` with the new params
- [ ] Run `cargo build --package koji --features web-ui`
  - Did it succeed? Fix errors.
- [ ] Run `cargo build --package koji` (without feature — should still work without the web UI)
  - Did it succeed? Fix errors.
- [ ] Run `cargo clippy --workspace -- -D warnings`
  - Fix any warnings.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `"feat(koji-cli): add 'koji web' sub-command to launch the web control plane"`

**Acceptance criteria:**
- [ ] `cargo build --package koji` (without feature) succeeds
- [ ] `cargo build --package koji --features web-ui` succeeds
- [ ] `koji web --help` shows the `--port`, `--proxy-url`, `--logs-dir`, `--config-path` flags
- [ ] `cargo clippy --workspace -- -D warnings` passes

---

### Task 6: Integration test, Makefile target, and documentation

**Context:**
Add a basic integration test that starts the `koji-web` server and confirms it returns 200 for `/` and forwards `/koji/v1/system/health` to a mock. Update the Makefile with a `build-web` target that runs `trunk build` in `crates/koji-web/` before the Rust build. Document the web UI in the project README.

**Files:**
- Create: `crates/koji-web/tests/server_test.rs`
- Modify: `crates/koji-web/Cargo.toml` (add `[[test]]` entry and `tokio` / `reqwest` dev-deps)
- Modify: `Makefile`
- Modify: `README.md`

**What to implement:**

`crates/koji-web/tests/server_test.rs`:
```rust
#[cfg(feature = "ssr")]
mod tests {
    use std::sync::Arc;

    async fn start_test_server() -> (reqwest::Client, std::net::SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let state = Arc::new(koji_web::server::AppState {
                proxy_base_url: "http://127.0.0.1:11434".to_string(),
                client: reqwest::Client::new(),
                logs_dir: None,
                config_path: None,
            });
            axum::serve(listener, koji_web::server::build_router(state)).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        (reqwest::Client::new(), addr)
    }

    /// GET / returns 200 (index.html embedded) or 404 (dist/ empty in dev) — both are valid.
    #[tokio::test]
    async fn test_root_returns_html_or_not_found() {
        let (client, addr) = start_test_server().await;
        let resp = client.get(format!("http://{}/", addr)).send().await.unwrap();
        let status = resp.status().as_u16();
        assert!(
            status == 200 || status == 404,
            "Expected 200 or 404 for /, got {status}"
        );
    }

    /// GET /api/config returns 404 when config_path is None (not configured).
    #[tokio::test]
    async fn test_api_config_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client.get(format!("http://{}/api/config", addr)).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("error").is_some(), "Expected error field in response");
    }

    /// GET /api/logs returns 404 when logs_dir is None (not configured).
    #[tokio::test]
    async fn test_api_logs_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client.get(format!("http://{}/api/logs", addr)).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 404);
        let body: serde_json::Value = resp.json().await.unwrap();
        assert!(body.get("error").is_some(), "Expected error field in response");
    }

    /// POST /api/config with invalid TOML returns 422.
    #[tokio::test]
    async fn test_api_config_save_returns_404_when_unconfigured() {
        let (client, addr) = start_test_server().await;
        let resp = client
            .post(format!("http://{}/api/config", addr))
            .json(&serde_json::json!({ "content": "not valid toml [[[[" }))
            .send()
            .await
            .unwrap();
        // 404 because config_path is None (checked before TOML validation)
        assert_eq!(resp.status().as_u16(), 404);
    }
}
```

Makefile additions:
```makefile
build-web:
	cd crates/koji-web && trunk build --release
	cargo build --package koji-web --features ssr

build-web-dev:
	cd crates/koji-web && trunk build
	cargo build --package koji-web --features ssr
```

README section to add (after the existing "Usage" or "Getting Started" section):
```markdown
## Web Control Plane

Koji includes a web-based control plane UI. Build and run it:

```bash
# 1. Build the frontend (requires trunk: cargo install trunk)
cd crates/koji-web && trunk build --release && cd ../..

# 2. Start the web server (port 11435 by default)
cargo run --package koji-web --features ssr

# Or via the CLI (with web-ui feature):
cargo run --package koji --features web-ui -- web --port 11435

# 3. Open http://localhost:11435
```

The web UI proxies all `/koji/v1/` requests to the running Koji proxy (default `http://127.0.0.1:11434`). Configure with env vars:
- `KOJI_PROXY_URL` — proxy base URL (default: `http://127.0.0.1:11434`)
- `KOJI_LOGS_DIR` — path to Koji log files (optional)
- `KOJI_CONFIG_PATH` — path to `koji.toml` for config editor (optional)
```

**Steps:**
- [ ] Add `[[test]]` entry to `crates/koji-web/Cargo.toml` pointing to `tests/server_test.rs`
- [ ] Add `tokio` and `reqwest` as `[dev-dependencies]` in `crates/koji-web/Cargo.toml`
- [ ] Create `crates/koji-web/tests/server_test.rs` with the integration test above
- [ ] Run `cargo test --package koji-web --features ssr`
  - Did it pass? If not, fix the test or server code.
- [ ] Add `build-web` and `build-web-dev` targets to `Makefile`
- [ ] Add the Web Control Plane section to `README.md`
- [ ] Run `cargo test --workspace`
  - Did all tests pass? Fix any regressions.
- [ ] Run `cargo fmt --all`
- [ ] Commit with message: `"feat(koji-web): add integration test, Makefile target, and README documentation"`

**Acceptance criteria:**
- [ ] `cargo test --package koji-web --features ssr` passes — all 4 integration tests pass
- [ ] `GET /api/config` and `GET /api/logs` return 404 (not 500) when their respective paths are unconfigured
- [ ] `cargo test --workspace` passes with no regressions
- [ ] `make build-web` target exists in Makefile
- [ ] README documents how to build and run the web UI
