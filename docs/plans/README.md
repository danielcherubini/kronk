# Implementation Plans Overview

This directory contains implementation plans for the Tama project. Each plan documents a feature or refactor with clear goals, architecture, tasks, and verification steps.

## Status Legend

| Status | Meaning |
|--------|---------|
| ✅ **COMPLETED** | Fully implemented, verified via git history |
| 🚧 **IN PROGRESS** | Currently being worked on |
| 📋 **DRAFT** | Planning phase, not yet started |
| ❌ **NOT STARTED** | Planned but not yet begun |
| 🔁 **SUPERSEDED** | Replaced by another plan |

## Quick Stats

- **Total Plans**: 94
- **Completed**: 93 ✅
- **In Progress**: 1 🚧
- **Remaining**: 0

> **Note**: The Tama Management API Spec (2026-04-03) was removed as it was a design document, not an implementation plan. The functionality it describes is already implemented via other plans.

---

## Completed Plans

### In Progress

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Model Manager Centralization](2026-05-13-model-manager-centralization.md) | Centralize all model DB access into a single ModelManager struct, replacing 29+ scattered db::open() calls across web, CLI, and proxy | 🚧 IN PROGRESS |

### Recently Completed

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Backend Manager Centralization](2026-05-13-backend-manager-centralization.md) | Centralize all backend data access into a single BackendManager struct, replacing scattered db::queries calls and absorbing BackendRegistry | `e6b163c` ✅ COMPLETED |
| [Backend Config to Database](2026-05-13-backend-config-to-db.md) | Move backend config (default_args, health_check_url) from config.toml to SQLite backend_configs table, keyed by (name, gpu_variant) with unique DB id | #88 ✅ COMPLETED |
| [Startup Detection & Orphan Cleanup](2026-05-03-startup-detection-and-orphan-cleanup.md) | Fix startup detection (2-consecutive health checks) and orphaned child process cleanup on startup failure | `17baa64` ✅ COMPLETED |
| [Model Card Redesign](2026-05-03-model-card-redesign.md) | Shared ModelCard component with accent strip, badge pills, and icon actions; replaces ModelRow and inline rendering | `85c75a5` ✅ COMPLETED |
| [HF Metadata for Models](2026-05-03-hf-metadata.md) | Add 9 HF metadata columns, populate from HF API + README parsing, display architecture on model cards | `925efde` ✅ COMPLETED |
| [Backend GPU Variant Restructure](2026-05-04-backend-gpu-variant-restructure.md) | Restructure backend folders to type/variant/version, add gpu_variant to DB and queries, support multiple GPU variants per backend | #85 `18c5d18` ✅ COMPLETED |
| [Split pull.rs Into Submodules](2026-05-06-split-pull-module.md) | Split 1,693-line models/pull.rs into 5 focused modules: api.rs, download.rs, metadata.rs, quant.rs | `bb6c8f5` ✅ COMPLETED |
| [Split config/resolve/tests.rs](2026-05-06-split-resolve-tests.md) | Split 2,214-line test file into 4 topic-grouped modules | `bb6c8f5` ✅ COMPLETED |
| [Inference Stats Dashboard Cards](2026-05-06-inference-stats-dashboard.md) | Surface llama_cpp timings (Processing Speed, Gen Speed, Cache Hits, Spec Accept) as 4 sparkline stat cards | `4a88d10` ✅ COMPLETED |
| [Shared Activity Panel + SSE Core](2026-05-06-shared-activity-panel-and-sse-core.md) | Extract duplicated SSE reconnection logic into shared utility, create generic ActivityPanel UI shell | `ca711f2` ✅ COMPLETED |
| [Metrics Snapshot Stream](2026-05-07-metrics-snapshot-stream.md) | Replace delta SSE with full snapshot delivery every 2s, unify inference stats into same pipeline, eliminate frontend desync | #86 `309c895`, `5d920b7`, `aff3c15`, `b024266` ✅ COMPLETED |
| [Remove Windows Support](todo/2026-05-08-remove-windows-support.md) | Remove all Windows-specific code, CI, build targets, dependencies, and documentation | #87 `091b11f`, `5f6a1c4`, `91559b3`, `918e2dd`, `9d7dbf4`, `f1af925`, `8f30f52`, `3b8419f` ✅ COMPLETED |

### Draft

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Split Remaining Long Files](2026-05-06-split-remaining-files-spec.md) | Split args_building.rs (1,411), pull/download.rs (1,041), crud/mod.rs (1,007) | 📋 DRAFT |

### Completed Plans

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Process Health Monitor](2026-04-29-process-health-monitor.md) | Detect dead backend PIDs after Proxmox suspend/resume, auto-restart with max_restarts guard, catch stuck Starting states | #80 `1af210f`, `a19b4a2`, `02bd651`, `59cac4c` |

### Core Infrastructure

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [KV Unified Support](2026-04-24-kv-unified-support.md) | Add --kv-unified flag support for llama-server shared KV cache pools | #73 `b3e535a`, `ab3ea8a`, `341dd66`, `c48f4a9` |
| [Rename Kronk to Tama](2026-04-06-rename-kronk-to-tama.md) | Complete rename across README, crates, routes, service names | `6d3a220`, `8281739`, `ab25016`, `bb8b734`, `d731eab` |
| [Split Large Files (Wave 1 & 2)](2026-03-23-split-large-files.md) | Split CLI and core files into focused modules | #20 `9915565`, `57b1fe2`, `3ee005e` |
| [Split Large Files (Wave 3)](2026-04-10-split-large-files.md) | Split remaining large files into domain submodules | #48 `b1e2f7d`, `8705ad0`, `7c6d50c` |
| [Split Large Files (Wave 4)](2026-04-18-file-size-refactor.md) | Split remaining files >400 lines: model.rs, backends.rs, api.rs, gpu.rs, source.rs, backend.rs, model_editor/mod.rs | `69b7889` ✅ COMPLETED |
| [Split Server Handler](2026-03-28-split-server-handler.md) | Split handlers/server.rs and proxy/server.rs into submodules | `a9b3a84`, `92c110f` |
| [Split Windows Platform](2026-03-28-split-windows-platform.md) | Split platform/windows.rs into install, service, firewall, permissions | `5d20835` |

### CLI & Commands

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Bench Command](2026-03-29-bench-command.md) | LLM inference benchmarking CLI command | `4bf65f7`, `5d54245`, `7549b2c` |
| [Status Command Redesign](2026-03-21-status-command-plan.md) | Unified status command with /status endpoint, removed model ps | `4de3b5a`, `b077271`, `7a49b44` |
| [Server Add/Edit Flag Extraction](2026-03-21-server-add-flag-extraction-plan.md) | Extract tama flags from args, validate model cards | `c8327c8`, `4de3b5a` |
| [Self-Update](2026-04-12-self-update.md) | CLI `tama self-update` and web UI update button with GitHub release download | #56 `efd5459`, `0b47435`, `cc51c83`, `1bf5ee8`, `5587df1` |
| [Move Self-Update to Updates Center](2026-04-17-move-self-update-to-updates-center.md) | Move self-update UI from sidebar to /updates page, keep minimal version indicator in sidebar | #62 `fa2cc94` ✅ COMPLETED |

### Database & Storage

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [SQLite DB and Model Update](2026-03-30-sqlite-db-and-model-update.md) | SQLite database foundation with migration system | `e7e73e0`, `8d01ccb` |
| [DB Autobackfill and Process Tracking](2026-03-30-db-autobackfill-and-process-tracking.md) | Active models table, backfill detection | `fe9efcb`, `1fa1f9d` |
| [Backend Registry to DB](2026-04-04-backend-registry-to-db.md) | Migrate from TOML to SQLite, add migration v3 | `998256c`, `d9aa88f`, `e3565e9`, `e954552` |
| [Backup & Restore](2026-04-13-backup-restore.md) | Backup config + DB archive, restore with model re-download and backend re-install | `ad77da6`, `b225b8c`, `58f13b3`, `07643e9` ✅ COMPLETED |

### Backend Management

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Backend Naming and Version Pinning](2026-04-04-backend-naming-and-config-version-pinning.md) | Canonical backend names, version pin field | `bce6928`, `90898b4`, `211546d` |
| [Backends Install/Update UI](2026-04-08-backends-install-update-ui-spec.md) | Install, update, and check-updates for backends from web UI | #43 `f500c27`, `89f71ed`, `32ae3f6`, `9a70c1e` |
| [Fix Backend Default Args](2026-04-10-fix-backend-default-args-spec.md) | Fix default_args display bug and add page-level save button | #49 `aefe2fe`, `29b26fc`, `6bee43d` |
| [ROCm Build Flags](2026-04-14-rocm-build-flags.md) | Detect AMDGPU_TARGETS via rocminfo; add rocWMMA FA, FA_ALL_QUANTS, LLAMA_CURL; export HIPCXX/HIP_PATH | `e862ab6`, `69d492a`, `c99304a`, `7698a11` ✅ COMPLETED |
| [Backend Version Cards](2026-04-17-backend-version-cards.md) | Multiple backend versions with visual cards, activate/switch, version-specific remove | #61 |
| [TTS Backend Support](2026-04-21-tts-backend.md) | Add Kokoro and Piper TTS engines with OpenAI-compatible `/v1/audio/*` endpoints, SQLite config, CLI commands, web UI integration | #70 `26c6a9d`, `79ea29b`, `38b072c`, `4738059`, `e1f63e7`, `88de610`, `3bb5c42`, `8c0c91c`, `f0277eb`, `cd7acfc`, `2e4c7c6`, `8ebfaa6` ✅ COMPLETED |

### Model Management

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Unified Model Config](2026-04-05-unified-model-config.md) | Merge model cards into ModelConfig with unified fields | `95c8e01`, `13bc2d3`, `0be825a` |
| [Integrate hf-hub for Authenticated Parallel Downloads](2026-04-14-integrate-hf-hub-for-downloads.md) | Use hf-hub's authenticated client for gated/private repos, fix slow start | `eac40cb` |
| [Interactive Model Pull Wizard](2026-04-04-interactive-model-pull-wizard.md) | Multi-step HF pull wizard with SSE progress | `04d609d`, `abe6aff`, `1114a13` |
| [Pull Quant from Model Editor](2026-04-07-pull-quant-from-model-editor-spec.md) | Pull new quants via modal on model edit page | #39 `d39e3e4`, `4b2803b`, `113da31` |
| [mmproj Support](2026-04-07-mmproj-support-spec.md) | Vision projector file support in pull wizard and model config | #40 `0489cc0`, `d58aa67`, `492dd1a` |
| [API Name for Models](2026-04-09-api-name-for-models.md) | Use HF repo names as model identifiers in OpenAI API | #47 `d659b9f`, `8edb7d9`, `0cf3ef6` |
| [Model Grid Separation](2026-04-07-model-grid-separation.md) | Split model grid into loaded and unloaded sections | `43b5678`, `405632b`, `329be36` |
| [Quant File Deletion](2026-04-10-quant-file-deletion.md) | Delete GGUF files on quant removal, `tama model prune` command | #50 `a160eb3`, `f350293`, `f6461d1`, `78c3feb` |
| [Preserve GGUF in Names](2026-03-27-preserve-gguf-in-names.md) | Preserve -GGUF suffix in model IDs and paths | `c102bd0`, `58ad0b4` |
| [Num Parallel Slots](2026-04-20-num-parallel-slots.md) | Add num_parallel field to model configs that multiplies effective context length at inference time | #66
| [Updates Center Fix](2026-04-20-updates-center-fix.md) | Backend update progress (JobLogPanel), per-quant LFS hash checking, download queue integration, expandable quant UI with selection | #65
| [Migrate Profiles to Model Cards Tests](2026-03-24-migrate_profiles_to_model_cards_tests.md) | Tests integrated into unified model config | `95c8e01` |
| [Model Card Cleanup](2026-03-24-model-card-cleanup.md) | Part of unified model config | `95c8e01` |
| [Remove Profiles.d](2026-03-24-remove-profiles-d.md) | Part of unified model config | `95c8e01` |

### Web UI

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Web UI Redesign](2026-04-04-web-ui-redesign.md) | Dark theme, nav bar, sparkline charts, dashboard polish | `734623d`, `d585ba4`, `9dc78d3`, `502e2f6` |
| [Config Page Redesign](2026-04-07-config-page-redesign-spec.md) | Real functional config editor with editable forms | #41 `0504eef`, `f28c104`, `519e9a2` |
| [Model Editor Redesign](2026-04-10-model-editor-redesign.md) | Side-nav layout, consolidated state, modular structure | #51 `a7f1850`, `bdadc68`, `1666050` |
| [Collapsible Sidebar Navigation](2026-04-11-sidebar-navigation.md) | Replace topbar with collapsible left sidebar | #55 `9fa3e67`, `f5046a4`, `592a40c`, `d9af7ad` |
| [Dashboard Metrics Redesign](2026-04-11-dashboard-redesign.md) | Interactive sparkline cards with hover, history API | #54 `858bf61`, `34ce619`, `502e2f6` |
| [Pull Model Modal Refactor](2026-04-08-pull-model-modal-refactor.md) | Replace /pull page with modal on Models tab | #44 `0907a4e`, `ec3abc3`, `8dc2a8f` |
| [Pull Wizard Improvements](2026-04-14-pull-wizard-improvements.md) | Consolidate quant/vision selection, smart KV cache dropdown, APEX/UD support, HF cache cleanup | #58 `10a9d7f`, `603c403`, `3be54a8`, `db955e0`, `6af6423`, `ae1c8f1` |
| [Wizard & Cache Improvements](2026-04-14-wizard-cache-improvements.md) | Fix KV dropdown, add APEX/UD quant support, implement HF cache cleanup | #58 `3be54a8`, `db955e0`, `6af6423`, `ae1c8f1` |
| [Context Length Selector](2026-04-14-context-length-selector.md) | Shared component for context length input with dropdown and custom value fallback | #59 |
| [KV Cache Quantization Dropdowns](2026-04-27-kv-cache-quants.md) | Add K and V cache quantization dropdown selectors to model editor form, wired through all layers to llama-server CLI flags | #77 ✅ COMPLETED |
| [Dashboard: Show All Models + Pull Model + Check All](2026-04-30-dashboard-all-models.md) | Extend dashboard to show inactive models section, add Pull Model and Check all for updates buttons, hide Models from sidebar | #82 `75543f0`, `e273fa2`, `5d1794d`, `fc860f0`, `eec050f`, `bd969b7`, `4500d30` ✅ COMPLETED
| [Models Page Horizontal Layout](2026-04-30-models-page-horizontal-layout.md) | Replace models page vertical card grid with horizontal row layout matching dashboard | #81 `fe94160` ✅ COMPLETED |
| [Benchmarks Page](2026-04-19-benchmarks.md) | Web UI benchmarking page with llama-bench integration, SSE progress streaming, preset configs (Quick/VRAM Sweet Spot/Thread Scaling), and benchmark history | `dd869b8`–`4be90f7` ✅ COMPLETED |
| [Config Hot Reload](2026-04-06-config-hot-reload.md) | Config sync from web UI to proxy without restart | `69cbb68`, `54298dc`, `219c749` |
| [Tama Web Control Plane](2026-04-03-tama-web-control-plane.md) | Core UI — initial implementation | ✅ PARTIALLY COMPLETED |

### Metrics & Dashboard

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Inference Stats Dashboard Cards](2026-05-06-inference-stats-dashboard.md) | Surface llama_cpp timings (Processing Speed, Gen Speed, Cache Hits, Spec Accept) as 4 sparkline stat cards on the dashboard | `4a88d10` ✅ COMPLETED |
| [Fix Dashboard Stale Stats](2026-05-02-fix-dashboard-stale-stats.md) | Backfill metrics on SSE lag, tab visibility change, and SSE reconnect to prevent stale stats after browser idle | #84 `21f1a65` ✅ COMPLETED |
| [System Metrics](2026-04-04-system-metrics.md) | CPU%, RAM, GPU metrics with background collection task | `67029b2`, `2465a4d`, `11d9287` |
| [Persist Dashboard Metrics](2026-04-06-persist-dashboard-metrics.md) | SQLite persistence + SSE streaming for dashboard | `b657e22`, `8e6a5b5`, `fd12bf8`, `4c6d6e2`, `2892764` |
| [Dashboard Time Series Graphs](2026-04-06-dashboard-time-series-graphs.md) | Sparkline SVG charts for metrics visualization | `404f3be`, `6b651cf`, `9dc78d3`, `502e2f6` |
| [Dashboard Filter Loaded Models](2026-04-28-dashboard-filter-loaded-models.md) | Filter Active Models section to show only loaded (ready) models with proper empty-state UX | #78 `8a20bff` ✅ COMPLETED |

### Configuration

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Grouped Args Formats](2026-04-06-grouped-args-formats.md) | shlex helpers, grouped args format, auto-migration | `5c8fac1`, `3fbf27b`, `ae67a0b` |

### Lifecycle & Shutdown

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Proxy Shutdown](2026-04-06-proxy-shutdown.md) | Graceful shutdown method for ProxyState | `6c83743`, `82ec8ab` |
| [System Restart](2026-04-06-system-restart.md) | Process-level restart handler with graceful exit | `3a1b7a0`, `eea20ef`, `ec0fc08`, `0fe3ab5` |
| [Updates Center](2026-04-15-updates-center-plan.md) | Centralized `/updates` page with background checker, DB-cached results, and apply flows | `2099edb`, `29fb946`, `9db8ccf`, `e2bbec8` ✅ COMPLETED |

### Code Quality

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [Test Coverage Improvements](2026-04-18-core-test-coverage.md) | Add 98 unit tests across workspace — proxy, lifecycle, downloads, updates, API DTOs, CLI handlers | #63 `7180eb6` ✅ COMPLETED |
| [Code Quality Improvements](2026-03-25-code-quality-improvements.md) | Dead code cleanup, unused imports, formatting | `a93e639`, `423ec0b` |
| [Fix Download Progress Bar](2026-03-27-fix-download-progress-bar.md) | Content-Length parsing, finish_and_clear fixes | `bc35068`, `bd9ea75`, `f052bba` |
| [Fix Review Bugs](2026-04-20-fix-review-bugs.md) | Fix 40+ bugs from comprehensive code review: security vulnerabilities, data integrity, reactivity bugs, concurrency issues | #67 `8190b31` ✅ COMPLETED

### Discovery & Integration

| Plan | Description | PR / Git References |
|------|-------------|---------------------|
| [OpenCode Tama Plugin](2026-04-12-opencode-tama-plugin.md) | Auto-discover models via /v1/models, provide modalities and config | `f4530d6`, `dbf1e51`, `b1260e4` |
| [Proxy API Endpoints](2026-04-20-proxy-api-endpoints.md) | Add all missing llama.cpp-compatible API endpoints using wildcard forwarding | #68 `3e1d180` ✅ COMPLETED |
| [Max Loaded Models with LRU Eviction](2026-04-21-max-loaded-models.md) | Add `max_loaded_models` config field (default=1) that automatically evicts the least-recently-used model when capacity is reached | #69 ✅ COMPLETED |
| [Speculative Decoding Benchmark](2026-04-23-spec-decode-bench.md) | llama-cli based spec-decoding benchmark with sweep presets (ngram-simple/mod/map-k/k4v), delta vs baseline results table | #71 `dd9c1c1` ✅ COMPLETED |
| [Backend Log Viewing](2026-04-24-backend-log-viewing.md) | Grouped logs endpoint GET /tama/v1/logs returning all sources (tama + backends) in one response, Logs page with source dropdown selector, auto-refresh every 5s | #72 ✅ COMPLETED |

---

## Remaining Work

*(none)*

## Roadmap

Longer-term features that don't yet have implementation plans:

- **TUI Dashboard** — `tama-tui` crate with ratatui
- **System tray** — Windows tray icon for quick service toggle
- **Tauri GUI** — Lightweight desktop frontend for non-CLI users

## Superseded Plans

| Plan | Description | Status |
|------|-------------|--------|
| [Dashboard Time Series Graphs](2026-04-06-dashboard-time-series-graphs.md) | Superseded by persist-dashboard-metrics and dashboard-redesign | 🔁 SUPERSEDED |

## Early Drafts & Specs

These files are companion specs or early drafts that were absorbed into their associated implementation plans:

| File | Context |
|------|---------|
| [Dashboard Model Management Spec](2024-05-22-dashboard-model-management-spec.md) | Early 2024 spec, superseded by later plans |
| [Dashboard Model Management Plan](2024-05-22-dashboard-model-management-implementation-plan.md) | Early 2024 plan, superseded by later plans |
| [Status Command Spec](2026-03-21-status-command-spec.md) | Spec for status command redesign |
| [Server Add Flag Extraction Spec](2026-03-21-server-add-flag-extraction-spec.md) | Spec for flag extraction |
| [Config Page Implementation Plan](2026-04-07-config-page-implementation-plan.md) | Companion to config page spec |
| [mmproj Implementation Plan](2026-04-07-mmproj-support-plan.md) | Companion to mmproj spec |
| [Pull Quant from Model Editor Plan](2026-04-07-pull-quant-from-model-editor-plan.md) | Companion to pull-quant spec |
| [Backends Install/Update UI Plan](2026-04-08-backends-install-update-ui-plan.md) | Companion to backends spec |
| [Fix Backend Default Args Plan](2026-04-10-fix-backend-default-args-plan.md) | Companion to backend args spec |

---

## How to Use This Directory

1. **Find a plan** — Browse by category above
2. **Read the plan** — Understand the goal, architecture, and tasks
3. **Check status** — See if it's completed, in progress, or remaining
4. **Verify implementation** — Follow PR numbers or git references to see commits
5. **Track remaining work** — See "Remaining Work" section above

## Contributing

When implementing a new feature:

1. Create a new plan file in this directory with date prefix (YYYY-MM-DD)
2. Follow the template: Goal, Architecture, Tech Stack, Tasks
3. Mark tasks as `[ ]` (not started) or `[x]` (completed)
4. Link to related plans when applicable
5. Update this README with the new plan

## Related Files

- [`README.md`](../README.md) — Project overview
- [`AGENTS.md`](../AGENTS.md) — Development guide and conventions
- [`docs/openapi/tama-api.yaml`](../openapi/tama-api.yaml) — Machine-readable OpenAPI spec
- [`docs/openapi/openai-compat.yaml`](../openapi/openai-compat.yaml) — OpenAI-compatible API spec

---

**Last Updated**: 2026-05-14
