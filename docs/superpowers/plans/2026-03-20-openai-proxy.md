# OpenAI Proxy Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a standalone proxy endpoint that automatically manages the lifecycle of local models. It will intercept OpenAI-compliant requests, load the requested model if it isn't running, route the request to it, stream the response back, and shut it down after an idle timeout.

**Architecture:** 
- **API Server:** `axum` running on a configured host/port (default `0.0.0.0:8080`).
- **State Management:** A shared `ProxyState` wrapped in an `Arc<tokio::sync::RwLock>` to track running models (their process handles, active ports, and last access times).
- **Request Interception:** Read request body into `Bytes`, parse to extract the `"model"` field, and reconstruct the request to forward it to the local backend using `reqwest`.
- **Response Streaming:** `axum` will convert the `reqwest::Response` stream directly into an `axum::body::Body` to ensure SSE (Server-Sent Events) stream correctly back to the client.

**Tech Stack:** Rust, Tokio, Axum, Reqwest, Serde JSON, Bytes

---

### Task 1: Update Dependencies and Configuration

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/kronk-core/Cargo.toml`
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Add dependencies to workspace**
Open `Cargo.toml` and add under `[workspace.dependencies]`:
```toml
axum = "0.7"
tower-http = { version = "0.5", features = ["cors", "trace"] }
bytes = "1.0"
```

- [ ] **Step 2: Add dependencies to core**
Open `crates/kronk-core/Cargo.toml` and add under `[dependencies]`:
```toml
axum.workspace = true
tower-http.workspace = true
bytes.workspace = true
```

- [ ] **Step 3: Define ProxyConfig**
Modify `crates/kronk-core/src/config.rs` to add the proxy configuration.
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default = "default_proxy_enabled")]
    pub enabled: bool,
    #[serde(default = "default_proxy_host")]
    pub host: String,
    #[serde(default = "default_proxy_port")]
    pub port: u16,
    #[serde(default = "default_proxy_timeout")]
    pub idle_timeout_secs: u64,
}

fn default_proxy_enabled() -> bool { false }
fn default_proxy_host() -> String { "0.0.0.0".to_string() }
fn default_proxy_port() -> u16 { 8080 }
fn default_proxy_timeout() -> u64 { 300 } // 5 minutes
```
Add `#[serde(default)] pub proxy: ProxyConfig,` to the `Config` struct and update `impl Default for Config` to instantiate the `proxy` field.

- [ ] **Step 4: Verify Compilation**
Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/kronk-core/Cargo.toml crates/kronk-core/src/config.rs
git commit -m "feat: add proxy configuration and axum dependencies"
```

---

### Task 2: Implement Proxy State & Lifecycle Manager

**Files:**
- Create: `crates/kronk-core/src/proxy.rs`
- Modify: `crates/kronk-core/src/lib.rs`

- [ ] **Step 1: Expose Module**
In `crates/kronk-core/src/lib.rs`, add `pub mod proxy;`.

- [ ] **Step 2: Define State Structs**
Create `crates/kronk-core/src/proxy.rs` with the following state structs to track models:
```rust
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use std::time::Instant;
use std::collections::HashMap;

pub struct RunningModel {
    pub port: u16,
    pub last_accessed: Instant,
    pub shutdown_tx: mpsc::Sender<()>,
}

#[derive(Clone)]
pub struct ProxyState {
    pub config: crate::config::Config,
    pub active_models: Arc<RwLock<HashMap<String, RunningModel>>>,
    pub client: reqwest::Client,
}
```

- [ ] **Step 3: Implement get_or_start_model**
In `crates/kronk-core/src/proxy.rs`, write an async function on `ProxyState`:
```rust
impl ProxyState {
    pub async fn get_or_start_model(&self, model_name: &str) -> anyhow::Result<u16> {
        // 1. Check if already running (read lock)
        {
            let models = self.active_models.read().await;
            if let Some(m) = models.get(model_name) {
                // We cannot update last_accessed through a read lock, so we might need a separate mechanism or use a write lock for access update.
                // For simplicity, upgrade to write lock if we need to update access time.
            }
        }
        
        let mut models = self.active_models.write().await;
        if let Some(m) = models.get_mut(model_name) {
            m.last_accessed = Instant::now();
            return Ok(m.port);
        }
        
        // 2. Not running. Validate and start.
        let (_, _backend) = self.config.resolve_server(model_name)?;
        
        // Find a free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        drop(listener); // Free the port for the backend to use
        
        // Construct args and start ProcessSupervisor here...
        // For the plan, assume we set up the supervisor and wait for Ready event
        
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        
        // 3. Store in map
        models.insert(model_name.to_string(), RunningModel {
            port,
            last_accessed: Instant::now(),
            shutdown_tx,
        });
        
        Ok(port)
    }
}
```

- [ ] **Step 4: Implement Idle Reaper**
In `crates/kronk-core/src/proxy.rs`, implement the background task to cull inactive models:
```rust
pub fn start_idle_reaper(state: ProxyState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            interval.tick().await;
            let mut models = state.active_models.write().await;
            let now = Instant::now();
            models.retain(|name, model| {
                if now.duration_since(model.last_accessed).as_secs() > state.config.proxy.idle_timeout_secs {
                    tracing::info!("Unloading idle model: {}", name);
                    let _ = model.shutdown_tx.try_send(());
                    false // Remove from map
                } else {
                    true // Keep in map
                }
            });
        }
    });
}
```

- [ ] **Step 5: Verify Compilation**
Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 6: Commit**
```bash
git add crates/kronk-core/src/proxy.rs crates/kronk-core/src/lib.rs
git commit -m "feat: implement proxy model lifecycle manager"
```

---

### Task 3: Implement Detailed OpenAI Endpoints

**Files:**
- Modify: `crates/kronk-core/src/proxy.rs`

- [ ] **Step 1: Implement `GET /v1/models`**
Returns a list of available servers pretending to be OpenAI models.
```rust
use axum::{extract::State, Json};
use serde_json::{json, Value};

async fn list_models(State(state): State<ProxyState>) -> Json<Value> {
    let mut data = Vec::new();
    for (name, _) in &state.config.servers {
        data.push(json!({
            "id": name,
            "object": "model",
            "created": 0,
            "owned_by": "kronk"
        }));
    }
    Json(json!({ "object": "list", "data": data }))
}
```

- [ ] **Step 2: Implement `GET /v1/models/:model`**
Returns details for a single model.
```rust
use axum::{extract::Path, http::StatusCode};

async fn get_model(
    Path(model_name): Path<String>,
    State(state): State<ProxyState>
) -> Result<Json<Value>, StatusCode> {
    if state.config.servers.contains_key(&model_name) {
        Ok(Json(json!({
            "id": model_name,
            "object": "model",
            "created": 0,
            "owned_by": "kronk"
        })))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
```

- [ ] **Step 3: Implement the Streaming Reverse Proxy**
Create a single catch-all handler for completions, chat, and embeddings.
```rust
use axum::{
    body::Body,
    extract::Request,
    response::{IntoResponse, Response},
};
use bytes::Bytes;

async fn proxy_request(
    State(state): State<ProxyState>,
    req: Request,
) -> Result<Response, (StatusCode, String)> {
    let path = req.uri().path().to_string();
    ```rust
    let query = req.uri().query().map(|q| format!("?{}", q)).unwrap_or_default();
    
    // 1. Read the entire body into memory to parse it and forward it
    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // 2. Parse JSON just to find the "model" key
    let json_body: Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON: {}", e)))?;
    
    let model_name = json_body.get("model")
        .and_then(|m| m.as_str())
        .ok_or((StatusCode::BAD_REQUEST, "Missing 'model' field in JSON payload".to_string()))?;

    // 3. Ensure model is running and get its local port
    let port = state.get_or_start_model(model_name).await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 4. Construct upstream URL
    let target_url = format!("http://127.0.0.1:{}{}{}", port, path, query);

    // 5. Forward the request using Reqwest
```rust
    let reqwest_res = state.client.post(&target_url)
        .header("Content-Type", "application/json")
        .body(reqwest::Body::from(body_bytes)) // Pass along exact bytes received
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    // 6. Convert the Reqwest response (SSE stream) directly into an Axum body
    let mut response_builder = Response::builder().status(reqwest_res.status());
    for (key, value) in reqwest_res.headers() {
        response_builder = response_builder.header(key, value);
    }
    let axum_body = Body::from_stream(reqwest_res.bytes_stream());
    
    Ok(response_builder.body(axum_body).unwrap())
}
```

- [ ] **Step 4: Verify Compilation**
Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add crates/kronk-core/src/proxy.rs
git commit -m "feat: implement openai compliant proxy endpoints"
```

---

### Task 4: Setup Router and CLI Wiring

**Files:**
- Modify: `crates/kronk-core/src/proxy.rs`
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Create `start_server` Function**
In `crates/kronk-core/src/proxy.rs`:
```rust
pub async fn start_server(config: crate::config::Config) -> anyhow::Result<()> {
    let state = ProxyState {
        config: config.clone(),
        active_models: Arc::new(RwLock::new(HashMap::new())),
        client: reqwest::Client::new(),
    };

    start_idle_reaper(state.clone());

    let app = axum::Router::new()
        .route("/v1/models", axum::routing::get(list_models))
        .route("/v1/models/:model", axum::routing::get(get_model))
        // Catch-all POST for the proxy handler
        .route("/v1/*path", axum::routing::post(proxy_request))
        .with_state(state);

    let bind_addr = format!("{}:{}", config.proxy.host, config.proxy.port);
    tracing::info!("Starting OpenAI proxy server on http://{}", bind_addr);
    
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(listener, app).await?;
    
    Ok(())
}
```

- [ ] **Step 2: Add CLI Subcommand**
In `crates/kronk-cli/src/main.rs`, add `Proxy` to the `Commands` enum:
```rust
    /// Start the OpenAI compliant proxy manager
    Proxy,
```

- [ ] **Step 3: Route the CLI Command**
In the `main` match statement in `crates/kronk-cli/src/main.rs`, add the proxy route:
```rust
Commands::Proxy => {
    kronk_core::proxy::start_server(config).await?;
    Ok(())
}
```

- [ ] **Step 4: Verify Compilation**
Run: `cargo check --workspace`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add crates/kronk-core/src/proxy.rs crates/kronk-cli/src/main.rs
git commit -m "feat: wire proxy to cli command"
```

## Review Loop

1. Run Reviewer Subagent on this plan document before proceeding to implement.
2. Ensure plan logic matches the structure needed for proper execution.

## Execution

After saving, the plan is ready for subagent-driven development execution.
