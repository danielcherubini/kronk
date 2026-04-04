# System Metrics: CPU, RAM, and GPU Utilization

**Goal:** Expose CPU usage %, RAM used/total, and GPU utilization % from the proxy health endpoint and display them in the web dashboard.

**Architecture:**
A new `SystemMetrics` struct is collected every 5 seconds by a background Tokio task and stored as `Arc<RwLock<SystemMetrics>>` inside `ProxyState`. The `sysinfo` crate (already a workspace dependency, pinned to `"0.30"`) provides CPU and RAM. GPU utilization % comes from `nvidia-smi` for NVIDIA (consistent with the existing `query_vram` pattern). The `/kronk/v1/system/health` endpoint reads the cached struct. The web dashboard is updated to display all three new metrics.

**Tech Stack:** Rust, `sysinfo = "0.30"`, existing `nvidia-smi` shell approach, Axum, Leptos (web UI).

---

## Task 1: Add `SystemMetrics` struct and collection functions in `gpu.rs`

**Context:**
All system-level hardware queries currently live in `crates/kronk-core/src/gpu.rs`. We add CPU %, RAM, and GPU utilization % collection to the same file. The `sysinfo` crate (already in `kronk-core`'s dependencies via `sysinfo.workspace = true`) provides cross-platform CPU and RAM. GPU utilization % is queried from `nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits` (integer, 0–100), matching the existing `query_vram` style.

The `sysinfo = "0.30"` API works as follows (note: `CpuExt`/`SystemExt` extension traits do NOT exist in 0.30 — all methods are directly on `System`):
```rust
use sysinfo::System;
let mut sys = System::new();
sys.refresh_cpu_usage();
std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
sys.refresh_cpu_usage();
let cpu_pct: f32 = sys.global_cpu_info().cpu_usage();  // 0.0–100.0

sys.refresh_memory();
let ram_used_mib: u64 = sys.used_memory() / 1024 / 1024;
let ram_total_mib: u64 = sys.total_memory() / 1024 / 1024;
```

Note: `sys.used_memory()` and `sys.total_memory()` return **bytes** in sysinfo 0.30.

**Files:**
- Modify: `crates/kronk-core/src/gpu.rs`

**What to implement:**

Add this struct (with `Serialize`, `Deserialize`, `Clone`, `Debug`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemMetrics {
    pub cpu_usage_pct: f32,          // 0.0–100.0
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    pub gpu_utilization_pct: Option<u8>,  // 0–100, None if not available
    pub vram: Option<VramInfo>,
}
```

Add one public function:
```rust
/// Collect a snapshot of system metrics (CPU, RAM, GPU util, VRAM).
/// This function blocks — call via `tokio::task::spawn_blocking`.
pub fn collect_system_metrics() -> SystemMetrics
```

Implementation:
1. Use `sysinfo` for CPU and RAM (as shown above — two `refresh_cpu()` calls with sleep between them).
2. Call `query_vram()` for VRAM (already exists).
3. For GPU utilization: run `nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits`, parse the first line as `u8`. If the command fails or parse fails, return `None`.

**Steps:**
- [ ] Read `crates/kronk-core/src/gpu.rs` fully before making changes.
- [ ] Add `Serialize, Deserialize` to the `#[derive(...)]` on the existing `VramInfo` struct in `gpu.rs`. It currently derives only `Debug, Clone` — add the two serde derives. (`serde` is already imported in the file.)
- [ ] Write a failing test `test_collect_system_metrics` in the `#[cfg(test)]` block at the bottom of `gpu.rs`: call `collect_system_metrics()`, assert `cpu_usage_pct` is between 0.0 and 100.0, `ram_total_mib` > 0, `ram_used_mib` <= `ram_total_mib`. (GPU fields can be `None` in CI — do not assert them.)
- [ ] Run `cargo test --package kronk-core test_collect_system_metrics -- --nocapture`
  - Did it fail with "unresolved" or "not found"? Good, proceed.
- [ ] Add `SystemMetrics` struct and `collect_system_metrics()` to `gpu.rs`.
- [ ] Run `cargo test --package kronk-core test_collect_system_metrics -- --nocapture`
  - Did it pass? If not, fix and re-run before continuing.
- [ ] Run `cargo fmt --all && cargo build --workspace`
  - Did both succeed? If not, fix and re-run.
- [ ] Commit: `"feat: add SystemMetrics struct and collect_system_metrics() to gpu.rs"`

**Acceptance criteria:**
- [ ] `SystemMetrics` struct is public and derives `Serialize`/`Deserialize`/`Clone`/`Debug`/`Default`
- [ ] `collect_system_metrics()` returns real CPU %, RAM in MiB, and VRAM (when available)
- [ ] Test passes on a machine without NVIDIA GPU (GPU fields return `None`)
- [ ] `cargo build --workspace` succeeds

---

## Task 2: Add cached metrics background task to `ProxyState`

**Context:**
CPU usage is meaningless as a point-in-time snapshot — it needs to be measured over an interval. We run a background Tokio task that calls `collect_system_metrics()` every 5 seconds via `spawn_blocking` and stores the result in `Arc<RwLock<SystemMetrics>>` inside `ProxyState`. Handlers then read the cached value cheaply without doing any blocking work.

The `ProxyState` struct lives in `crates/kronk-core/src/proxy/types.rs`. A new `start_metrics_task` function will be added to `crates/kronk-core/src/proxy/lifecycle.rs` (where other startup logic lives) or a new `src/proxy/metrics_task.rs` — place it wherever fits naturally after reading the file.

The background task is started in `crates/kronk-core/src/proxy/server/mod.rs` (or wherever `ProxyState` is created and the server is launched — read the file first).

**Files:**
- Modify: `crates/kronk-core/src/proxy/types.rs` — add `system_metrics: Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>>` field to `ProxyState`
- Modify: `crates/kronk-core/src/proxy/server/mod.rs` — initialize the field and spawn the background task
- Modify: `crates/kronk-core/src/proxy/mod.rs` — re-export anything new if needed

**What to implement:**

In `types.rs`, add to `ProxyState`:
```rust
pub system_metrics: Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>>,
```

`ProxyState` is constructed **only once** via a struct literal inside `ProxyState::new()` in `crates/kronk-core/src/proxy/state.rs`. That is the only place to add the new field — all other files call `ProxyState::new(...)` and will not break.

Initialize the field there with:
```rust
system_metrics: Arc::new(tokio::sync::RwLock::new(crate::gpu::SystemMetrics::default())),
```

Spawn the background task in `ProxyServer::new()` in `crates/kronk-core/src/proxy/server/mod.rs`, alongside the existing `start_idle_timeout_checker` call:
```rust
// Spawn background task to refresh system metrics every 5s
let metrics_arc = Arc::clone(&state.system_metrics);
tokio::spawn(async move {
    loop {
        let snapshot = match tokio::task::spawn_blocking(crate::gpu::collect_system_metrics).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("system metrics collection panicked: {}", e);
                crate::gpu::SystemMetrics::default()
            }
        };
        *metrics_arc.write().await = snapshot;
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
});
```

**Steps:**
- [ ] Read `crates/kronk-core/src/proxy/types.rs`, `crates/kronk-core/src/proxy/state.rs`, `crates/kronk-core/src/proxy/server/mod.rs`, and `crates/kronk-core/src/proxy/mod.rs` fully before making changes.
- [ ] Add `system_metrics` field to `ProxyState` in `types.rs`.
- [ ] Run `cargo build --package kronk-core` — the only struct literal error will be in `state.rs` inside `ProxyState::new()`. Add the field there.
- [ ] Add the background spawn task in `ProxyServer::new()` in `server/mod.rs`, alongside `start_idle_timeout_checker`.
- [ ] Run `cargo build --workspace`
  - Did it succeed? If not, fix remaining errors.
- [ ] Run `cargo test --workspace`
  - Did all pass?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did both succeed? If not, fix and re-run.
- [ ] Commit: `"feat: add cached system metrics background task to ProxyState"`

**Acceptance criteria:**
- [ ] `ProxyState` has `system_metrics: Arc<RwLock<SystemMetrics>>` field
- [ ] Background task spawns at server startup and refreshes every 5s
- [ ] `cargo build --workspace` succeeds
- [ ] All existing tests pass

---

## Task 3: Expose metrics in `/kronk/v1/system/health` endpoint

**Context:**
The `/kronk/v1/system/health` handler in `crates/kronk-core/src/proxy/kronk_handlers.rs` currently builds an ad-hoc `serde_json::json!({...})` response with `status`, `service`, `models_loaded`, and `vram`. We extend it to include the cached `SystemMetrics` fields. We also take this opportunity to replace the ad-hoc `json!({})` with a proper typed response struct for correctness and testability.

The handler currently calls `spawn_blocking(crate::gpu::query_vram)` — this must be **removed** since VRAM is now part of the cached `SystemMetrics` snapshot.

**Files:**
- Modify: `crates/kronk-core/src/proxy/kronk_handlers.rs`
- Modify: `crates/kronk-core/src/proxy/status.rs`

**What to implement:**

Add a typed response struct in `kronk_handlers.rs` (above the handler function). Add `use crate::gpu::VramInfo;` to imports if not already present:
```rust
#[derive(Debug, Serialize)]
pub struct SystemHealthResponse {
    pub status: &'static str,
    pub service: &'static str,
    pub models_loaded: usize,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    pub gpu_utilization_pct: Option<u8>,
    pub vram: Option<VramInfo>,
}
```

Rewrite `handle_kronk_system_health` to:
1. Read `state.models` (existing)
2. Read `state.system_metrics.read().await` to get the cached snapshot
3. Return `Json(SystemHealthResponse { ... })` — no `spawn_blocking`, no `query_vram` call here

Also update `crates/kronk-core/src/proxy/status.rs`: the `build_status_response` function currently calls `tokio::task::spawn_blocking(crate::gpu::query_vram)` to populate the `vram` field. Replace this with reading from `state.system_metrics` (pass `Arc<ProxyState>` or read the `RwLock` — follow the same pattern as the handler). This removes the blocking call from the status response path too.

**Steps:**
- [ ] Read `crates/kronk-core/src/proxy/kronk_handlers.rs` (especially the health handler and its imports) before making changes.
- [ ] Write a failing test in the `#[cfg(test)]` block at the bottom of `kronk_handlers.rs` (or in a test module): `test_system_health_response_serializes` — construct a `SystemHealthResponse` with known values, serialize to JSON with `serde_json::to_value`, assert the JSON contains `"cpu_usage_pct"`, `"ram_used_mib"`, `"ram_total_mib"`.
- [ ] Run `cargo test --package kronk-core test_system_health_response_serializes`
  - Did it fail? Good.
- [ ] Add `SystemHealthResponse` struct and rewrite the handler.
- [ ] Remove the `spawn_blocking(query_vram)` call — replace with reading from `state.system_metrics`.
- [ ] Run `cargo test --package kronk-core test_system_health_response_serializes`
  - Did it pass?
- [ ] Run `cargo build --workspace && cargo test --workspace`
  - Did both succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
- [ ] Commit: `"feat: expose CPU/RAM/GPU metrics in /kronk/v1/system/health endpoint"`

**Acceptance criteria:**
- [ ] `/kronk/v1/system/health` response includes `cpu_usage_pct`, `ram_used_mib`, `ram_total_mib`, `gpu_utilization_pct`, `vram`
- [ ] No `spawn_blocking` in the health handler — reads from cache only
- [ ] Response uses a typed struct, not ad-hoc `json!({})`
- [ ] All tests pass

---

## Task 4: Update web dashboard to display new metrics

**Context:**
The Leptos web dashboard in `crates/kronk-web/src/pages/dashboard.rs` fetches `/kronk/v1/system/health` and displays status, models loaded, and VRAM. The local `SystemHealth` deserialization struct and the render function both need updating to show CPU %, RAM, and GPU utilization %.

**Files:**
- Modify: `crates/kronk-web/src/pages/dashboard.rs`

**What to implement:**

Update the local `SystemHealth` struct (in `dashboard.rs`) to match the new response. Use `usize` for `models_loaded` to match the server-side type exactly:
```rust
#[derive(Deserialize, Clone)]
struct SystemHealth {
    status: String,
    service: String,
    models_loaded: usize,
    cpu_usage_pct: f32,
    ram_used_mib: u64,
    ram_total_mib: u64,
    gpu_utilization_pct: Option<u8>,
    vram: Option<VramInfo>,
}
```

Update the render section to display the new fields. Use a consistent format:
- CPU: `"CPU: {:.1}%"` (one decimal place)
- RAM: `"RAM: {} / {} MiB"` (used / total)
- GPU util: `"GPU: {}%"` (only shown when `Some`)
- VRAM: keep existing `"VRAM: {} / {} MiB"` display (only shown when `Some`)

**Steps:**
- [ ] Read `crates/kronk-web/src/pages/dashboard.rs` fully before making changes.
- [ ] Update the `SystemHealth` struct to add the four new fields.
- [ ] Update the render function to display CPU, RAM, and GPU utilization rows.
- [ ] Run `cargo build --package kronk-web`
  - Did it succeed? If not, fix and re-run.
- [ ] Run `cargo build --workspace`
  - Did it succeed?
- [ ] Run `cargo fmt --all && cargo clippy --workspace -- -D warnings`
  - Did both succeed?
- [ ] Commit: `"feat: display CPU, RAM, and GPU utilization in web dashboard"`

**Acceptance criteria:**
- [ ] Dashboard shows CPU usage %, RAM used/total MiB, GPU util % (when available), VRAM (when available)
- [ ] `cargo build --workspace` succeeds
- [ ] `cargo clippy --workspace -- -D warnings` passes
