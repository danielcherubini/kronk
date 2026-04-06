# Koji Development Plan

## Completed (All Major Plans Implemented)

> **Note:** All 25+ implementation plans in `docs/plans/` have been completed except for the VLLM Backend feature.

## Remaining

- [ ] **VLLM Backend** — Add vLLM as a first-class backend type with PyPI version checking ([plan](docs/plans/2026-03-30-vllm-backend.md))

## Implementation Status

All plans marked with ✅ in `docs/plans/` have been implemented and verified via git history.
- [x] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff
- [x] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions
- [x] **Rename Kronk to Koji** — Complete rename across README, crates, routes, service names
- [x] **Split Large Files** — Wave 1 & 2: Split CLI and core files into focused modules
- [x] **Split Server Handler** — Split handlers/server.rs and proxy/server.rs into submodules
- [x] **Split Windows Platform** — Split platform/windows.rs into install, service, firewall, permissions
- [x] **Code Quality Improvements** — Dead code cleanup, unused imports, formatting
- [x] **Fix Download Progress Bar** — Content-Length parsing, finish_and_clear fixes
- [x] **Preserve GGUF in Names** — Preserve -GGUF suffix in model IDs and paths
- [x] **Bench Command** — LLM inference benchmarking CLI command
- [x] **Status Command Redesign** — Unified status command with /status endpoint, removed model ps
- [x] **Server Add/Edit Flag Extraction** — Extract koji flags from args, validate model cards
- [x] **SQLite DB and Model Update** — SQLite database foundation with migration system
- [x] **DB Autobackfill and Process Tracking** — Active models table, backfill detection
- [x] **Backend Naming and Config Version Pinning** — Canonical backend names, version pin field
- [x] **Backend Registry to DB** — Migrate from TOML to SQLite, add migration v3
- [x] **System Metrics** — CPU%, RAM, GPU metrics with background collection task
- [x] **Web UI Redesign** — Dark theme, nav bar, sparkline charts, dashboard polish
- [x] **Unified Model Config** — Merge model cards into ModelConfig with unified fields
- [x] **Config Hot Reload** — Config sync from web UI to proxy without restart
- [x] **Grouped Args Formats** — shlex helpers, grouped args format, auto-migration
- [x] **Persist Dashboard Metrics** — SQLite persistence + SSE streaming for dashboard
- [x] **Dashboard Time Series Graphs** — Sparkline SVG charts for metrics visualization
- [x] **Proxy Shutdown** — Graceful shutdown method for ProxyState
- [x] **System Restart** — Process-level restart handler with graceful exit


