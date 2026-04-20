# Proxy API Endpoints Plan

**Goal:** Add all missing llama.cpp-compatible API endpoints to the Koji proxy using a wildcard routing approach — only `/koji/*` needs custom handlers; everything else is forwarded directly.

**Architecture:** Replace per-endpoint handlers with two catch-all routes:
- `POST /*path` — forwards POST requests to the backend (chat completions, embeddings, responses, tokenize, etc.)
- `GET /*path` — forwards GET requests to the backend (props, slots, models, metrics)

The existing Koji management endpoints (`/koji/v1/models`, `/koji/v1/pulls`, etc.) keep their explicit handlers. This eliminates ~20 individual handler functions and makes adding new llama.cpp API routes a zero-effort change — any new endpoint the backend adds works automatically.

**Tech Stack:** Rust, Axum (web framework), existing `forward_request()` in `crates/koji-core/src/proxy/forward.rs`

---

### Task 1: Add wildcard forwarding routes

**Context:**
Currently each endpoint has its own handler that extracts the model field, auto-loads the server, and forwards. This is repetitive — almost every handler does the same thing (parse body → extract `model` → forward). A wildcard route handles all of this generically:
- POST with JSON body: try to extract `model` field for auto-loading, then forward
- GET requests or non-JSON POST: forward directly (no auto-load needed)

The existing `forward_request()` already handles model name rewriting for both streaming and non-streaming responses, so no changes are needed there.

**Files:**
- Modify: `crates/koji-core/src/proxy/server/router.rs`
- Modify: `crates/koji-core/src/proxy/handlers.rs` (add 2 new handlers)

**What to implement:**
Add two wildcard handler functions in `handlers.rs`:

1. `handle_forward_post(path: String, state: State<Arc<ProxyState>>, req: Request<Body>)` — handles all POST requests not matching `/koji/*`. Logic:
   - Extract body bytes
   - Try to parse as JSON and extract `.get("model")?.as_str()` for auto-loading
   - If model found, look up or auto-load the target server (same logic as existing `handle_chat_completions`)
   - Call `forward_request()` with the original path (preserving `/v1/chat/completions`, `/embeddings`, etc.)

2. `handle_forward_get(path: String, state: State<Arc<ProxyState>>, req: Request<Body>)` — handles all GET requests not matching `/koji/*`. Logic:
   - No body to parse (GET has no body in axum)
   - No auto-load needed for most GET endpoints (health, props, slots, metrics, models)
   - Call `forward_request()` with the original path

Then update `router.rs` to add these as fallback routes **before** the existing `/koji/*` explicit routes. The route order matters:
1. Explicit `/koji/v1/*` routes first (specific handlers)
2. Wildcard `POST /*path` and `GET /*path` last (catch-all)

**Important:** The wildcard must not match `/koji/*` paths since those have explicit handlers. In axum, more specific routes take precedence over wildcards, so this works naturally — `/koji/v1/models` will hit the explicit handler, while `/v1/embeddings` falls through to the wildcard.

**Steps:**
- [ ] Add `handle_forward_post` function to `crates/koji-core/src/proxy/handlers.rs`:
  ```rust
  pub async fn handle_forward_post(
      Path(path): Path<String>,
      state: State<Arc<ProxyState>>,
      req: Request<Body>,
  ) -> Response {
      // Same model extraction + auto-load logic as handle_chat_completions
      let (mut parts, body) = req.into_parts();
      let body_bytes = to_bytes(body, MAX_REQUEST_BODY_SIZE).await.unwrap_or_default();

      // Try to extract model for auto-loading
      let model_name: Option<String> = serde_json::from_slice::<serde_json::Value>(&body_bytes)
          .ok()
          .and_then(|v| v.get("model").and_then(|m| m.as_str()))
          .map(|s| s.to_string());

      let server_name = if let Some(ref model) = model_name {
          match state.get_available_server_for_model(model).await {
              Some(name) => name,
              None => {
                  let card = state.get_model_card(model).await;
                  match state.load_model(model, card.as_ref()).await {
                      Ok(s) => s,
                      Err(e) => return error_response(500, &format!("Failed to load model: {}", e)),
                  }
              }
          }
      } else {
          // No model field — forward to first available server or return error
          let models = state.models.read().await;
          if let Some(name) = models.keys().next().cloned() {
              name
          } else {
              return error_response(503, "No backend server available");
          }
      };

      state.update_last_accessed(&server_name).await;
      forward_request(&state, &server_name, &parts, &body_bytes, model_name.as_deref().unwrap_or("")).await
  }
  ```

- [ ] Add `handle_forward_get` function to `crates/koji-core/src/proxy/handlers.rs`:
  ```rust
  pub async fn handle_forward_get(
      Path(path): Path<String>,
      state: State<Arc<ProxyState>>,
      req: Request<Body>,
  ) -> Response {
      let (parts, body) = req.into_parts();
      let body_bytes = to_bytes(body, MAX_REQUEST_BODY_SIZE).await.unwrap_or_default();

      // GET requests don't have a model field — forward to any available server
      let models = state.models.read().await;
      let server_name = models.keys().next().cloned().unwrap_or_else(|| String::new());
      drop(models);

      if server_name.is_empty() {
          return (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
              "error": {"message": "No backend server available", "type": "ServiceUnavailableError"}
          }))).into_response();
      }

      forward_request(&state, &server_name, &parts, &body_bytes, "").await
  }
  ```

- [ ] Export new handlers from `handlers.rs`
- [ ] Update `crates/koji-core/src/proxy/server/router.rs` to add wildcard routes:
  ```rust
  .route("/*path", post(handle_forward_post))
  .route("/*path", get(handle_forward_get))
  ```
  Place these **before** `.fallback(handle_fallback)` but **after** all explicit `/koji/*` routes.

- [ ] Run `cargo test --package koji-core -- proxy::handlers::tests` to verify existing tests still pass
- [ ] Run `cargo fmt --all`
- [ ] Run `cargo build --workspace`
- [ ] Commit with message: "feat(proxy): add wildcard forwarding for all non-koji endpoints"

**Acceptance criteria:**
- [ ] All existing explicit routes (`/koji/v1/*`, `/v1/chat/completions`, `/v1/models`, `/health`, `/metrics`, `/status`) continue to work
- [ ] `POST /v1/embeddings` forwards correctly without a dedicated handler
- [ ] `POST /v1/responses` forwards correctly
- [ ] `GET /props` forwards correctly
- [ ] `GET /slots` forwards correctly
- [ ] `GET /metrics/prometheus` forwards correctly
- [ ] `POST /completion`, `/tokenize`, `/detokenize`, `/apply-template`, `/infill` all forward correctly
- [ ] `POST /lora-adapters`, `POST /reranking`, `/v1/messages` all forward correctly
- [ ] Model auto-loading works for POST requests with a `model` field
- [ ] Existing tests still pass
- [ ] No regressions in existing endpoints

---

## What This Covers (All Missing Endpoints)

With wildcard routing, ALL of these work automatically — no new code needed beyond Task 1:

| Route | Method | Works via wildcard? |
|-------|--------|-------------------|
| `/v1/embeddings` | POST | ✅ Yes |
| `/embeddings` | POST | ✅ Yes |
| `/v1/responses` | POST | ✅ Yes |
| `/props` | GET, POST | ✅ Yes |
| `/completion` | POST | ✅ Yes |
| `/tokenize` | POST | ✅ Yes |
| `/detokenize` | POST | ✅ Yes |
| `/apply-template` | POST | ✅ Yes |
| `/infill` | POST | ✅ Yes |
| `/lora-adapters` | GET, POST | ✅ Yes |
| `/slots` | GET | ✅ Yes |
| `/slots/:id?action=save` | POST | ✅ Yes (query params preserved) |
| `/slots/:id?action=restore` | POST | ✅ Yes |
| `/slots/:id?action=erase` | POST | ✅ Yes |
| `/v1/messages` | POST | ✅ Yes |
| `/v1/messages/count_tokens` | POST | ✅ Yes |
| `/reranking` | POST | ✅ Yes |
| `/rerank` | POST | ✅ Yes |
| `/v1/rerank` | POST | ✅ Yes |
| `/v1/reranking` | POST | ✅ Yes |
| `/metrics/prometheus` | GET | ✅ Yes |

Plus any future llama.cpp endpoints — zero code changes needed.

---

## Notes

- The wildcard `/*path` captures everything after the root, including nested paths like `v1/chat/completions` and query strings like `slots/0?action=save`
- Axum preserves query parameters on wildcard routes automatically
- Model name rewriting in responses already works via existing `forward_request()` logic
- For GET requests without a model field (like `/props`, `/slots`, `/metrics`), the handler forwards to any available server — if none exist, it returns 503
- The only edge case is endpoints that require a specific loaded server but don't have a `model` field in the request. These are handled by forwarding to whatever server is available (which is the same behavior as the current `/health` and `/metrics` endpoints)
