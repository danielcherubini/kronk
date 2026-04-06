# Status Command Redesign - Spec

**Date:** 2026-03-21
**Reviewed:** 2026-03-21
**Status:** DONE

## Problem

`koji status` and `koji model ps` have diverged in output format. Both are slow because they independently query health endpoints per model with 3s HTTP timeouts. The output shows a "Service" line that's no longer relevant (single proxy architecture), and VRAM is buried at the bottom. Neither command shows whether a model is actually loaded in the proxy or useful config details.

## Goals

1. **Unified command**: Remove `koji model ps`, keep only `koji status`
2. **Fast**: Single HTTP call to the running proxy instead of per-model health checks
3. **Informative**: Show model config (source, quant, profile, context) and runtime state (loaded, idle timeout)
4. **VRAM at top**: GPU memory displayed prominently
5. **TUI/GUI ready**: The proxy `/status` endpoint returns structured JSON usable by future UIs

## Design

### New Proxy Endpoint: `GET /status`

Returns a JSON payload with all status info in one call:

```json
{
  "vram": {
    "used_mib": 633,
    "total_mib": 10240
  },
  "idle_timeout_secs": 300,
  "metrics": {
    "total_requests": 142,
    "successful_requests": 138,
    "failed_requests": 4,
    "models_loaded": 3,
    "models_unloaded": 1
  },
  "models": {
    "nemotron": {
      "backend": "ik_llama",
      "backend_path": "D:\\AI\\ik_llama.cpp\\build\\bin\\llama-server.exe",
      "source": "nvidia/Nemotron-Mini-4B-Instruct",
      "quant": "Q4_K_M",
      "profile": "coding",
      "context_length": 8192,
      "enabled": true,
      "loaded": true,
      "backend_pid": 12345,
      "load_time_secs": 1742572800,
      "last_accessed_secs_ago": 32,
      "idle_timeout_remaining_secs": 268,
      "consecutive_failures": 0
    },
    "omnicoder": {
      "backend": "ik_llama",
      "backend_path": "D:\\AI\\ik_llama.cpp\\build\\bin\\llama-server.exe",
      "source": "Tesslate/OmniCoder-9B",
      "quant": "Q6_K",
      "profile": "coding",
      "context_length": 16384,
      "enabled": true,
      "loaded": false,
      "backend_pid": null,
      "load_time_secs": null,
      "last_accessed_secs_ago": null,
      "idle_timeout_remaining_secs": null,
      "consecutive_failures": null
    }
  }
}
```

### VRAM Query Strategy

`query_vram()` calls `nvidia-smi` synchronously. To avoid blocking the async runtime, the proxy must use `tokio::task::spawn_blocking` when calling it from the `/status` handler. This keeps the handler non-blocking while still providing fresh VRAM data.

The CLI also queries VRAM independently (for the proxy-down fallback case), which is fine since the CLI is not an async server.

### CLI Output Format

```
KOJI Status
------------------------------------------------------------
  VRAM:     633 / 10240 MiB

  Model:    nemotron
  Source:   nvidia/Nemotron-Mini-4B-Instruct
  Quant:    Q4_K_M
  Profile:  coding
  Context:  8192
  Backend:  ik_llama (D:\AI\ik_llama.cpp\build\bin\llama-server.exe)
  Loaded:   true (idle: 32s ago, unloads in 4m28s)

  Model:    omnicoder
  Source:   Tesslate/OmniCoder-9B
  Quant:    Q6_K
  Profile:  coding
  Context:  16384
  Backend:  ik_llama (D:\AI\ik_llama.cpp\build\bin\llama-server.exe)
  Loaded:   false
```

When the proxy is not running, fall back to config-only display (no loaded/idle info):

```
KOJI Status
------------------------------------------------------------
  VRAM:     633 / 10240 MiB

  Model:    nemotron
  Source:   nvidia/Nemotron-Mini-4B-Instruct
  Quant:    Q4_K_M
  Profile:  coding
  Context:  8192
  Backend:  ik_llama (D:\AI\ik_llama.cpp\build\bin\llama-server.exe)
  Loaded:   proxy not running
```

### Fallback Behavior

- VRAM query (`nvidia-smi`) is always done CLI-side as well, so it's available even when proxy is down
- If proxy is unreachable (connection refused/timeout), show config-only info with `Loaded: proxy not running`
- 500ms timeout on the proxy `/status` call so CLI stays snappy

### What Gets Removed

- `koji model ps` command entirely (from ModelCommands enum, model.rs dispatch, and main.rs)
- Service status querying from `cmd_status` (no more `query_service` / platform calls)
- Per-model HTTP health checks from `cmd_status`

### What Gets Added

- `GET /status` route on the proxy server
- `ProxyState::build_status_response()` method (uses `spawn_blocking` for VRAM)
- Updated `cmd_status` that calls the proxy `/status` endpoint with 500ms timeout
- Model config details (source, quant, profile, context) in status output

### Notes

- `ModelState::last_accessed` is `Instant` (monotonic, not serializable). Converted to `secs_ago` at response time via `Instant::now() - last_accessed`.
- `load_time` is `SystemTime`, serialized as Unix timestamp seconds.
- Proxy metrics included at top level for TUI/GUI monitoring use cases.
