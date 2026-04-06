# Implementation Plans Overview

This directory contains implementation plans for the Koji project. Each plan documents a feature or refactor with clear goals, architecture, tasks, and verification steps.

## Status Legend

| Status | Meaning |
|--------|---------|
| ✅ **COMPLETED** | Fully implemented, verified via git history |
| 🚧 **IN PROGRESS** | Currently being worked on |
| 📋 **DRAFT** | Planning phase, not yet started |
| ❌ **NOT STARTED** | Planned but not yet begun |
| 🔁 **SUPERSEDED** | Replaced by another plan |

## Quick Stats

- **Total Plans**: 27
- **Completed**: 26 ✅
- **Remaining**: 1 🚧 (VLLM Backend)

> **Note**: The Koji Management API Spec (2026-04-03) was removed as it was a design document, not an implementation plan. The functionality it describes is already implemented via other plans.

## Completed Plans

### Core Infrastructure

| Plan | Description | Git References |
|------|-------------|----------------|
| [Rename Kronk to Koji](2026-04-06-rename-kronk-to-koji.md) | Complete rename across README, crates, routes, service names | `6d3a220`, `8281739`, `ab25016`, `bb8b734`, `d731eab` |
| [Split Large Files](2026-03-23-split-large-files.md) | Wave 1 & 2: Split CLI and core files into focused modules | `9915565`, `57b1fe2`, `3ee005e` |
| [Split Server Handler](2026-03-28-split-server-handler.md) | Split handlers/server.rs and proxy/server.rs into submodules | `a9b3a84`, `92c110f` |
| [Split Windows Platform](2026-03-28-split-windows-platform.md) | Split platform/windows.rs into install, service, firewall, permissions | `5d20835` |

### CLI & Commands

| Plan | Description | Git References |
|------|-------------|----------------|
| [Bench Command](2026-03-29-bench-command.md) | LLM inference benchmarking CLI command | `4bf65f7`, `5d54245`, `7549b2c` |
| [Status Command Redesign](2026-03-21-status-command-plan.md) | Unified status command with /status endpoint, removed model ps | `4de3b5a`, `b077271`, `7a49b44` |
| [Server Add/Edit Flag Extraction](2026-03-21-server-add-flag-extraction-plan.md) | Extract koji flags from args, validate model cards | `c8327c8`, `4de3b5a` |

### Database & Storage

| Plan | Description | Git References |
|------|-------------|----------------|
| [SQLite DB and Model Update](2026-03-30-sqlite-db-and-model-update.md) | SQLite database foundation with migration system | `e7e73e0`, `8d01ccb` |
| [DB Autobackfill and Process Tracking](2026-03-30-db-autobackfill-and-process-tracking.md) | Active models table, backfill detection | `fe9efcb`, `1fa1f9d` |
| [Backend Registry to DB](2026-04-04-backend-registry-to-db.md) | Migrate from TOML to SQLite, add migration v3 | `998256c`, `d9aa88f`, `e3565e9`, `e954552` |

### Backend Management

| Plan | Description | Git References |
|------|-------------|----------------|
| [Backend Naming and Version Pinning](2026-04-04-backend-naming-and-config-version-pinning.md) | Canonical backend names, version pin field | `bce6928`, `90898b4`, `211546d` |

### Metrics & Dashboard

| Plan | Description | Git References |
|------|-------------|----------------|
| [System Metrics](2026-04-04-system-metrics.md) | CPU%, RAM, GPU metrics with background collection task | `67029b2`, `2465a4d`, `11d9287` |
| [Persist Dashboard Metrics](2026-04-06-persist-dashboard-metrics.md) | SQLite persistence + SSE streaming for dashboard | `b657e22`, `8e6a5b5`, `fd12bf8`, `4c6d6e2`, `2892764` |
| [Dashboard Time Series Graphs](2026-04-06-dashboard-time-series-graphs.md) | Sparkline SVG charts for metrics visualization | `404f3be`, `6b651cf`, `9dc78d3`, `502e2f6` |

### Web UI

| Plan | Description | Git References |
|------|-------------|----------------|
| [Web UI Redesign](2026-04-04-web-ui-redesign.md) | Dark theme, nav bar, sparkline charts, dashboard polish | `734623d`, `d585ba4`, `9dc78d3`, `502e2f6` |
| [Config Hot Reload](2026-04-06-config-hot-reload.md) | Config sync from web UI to proxy without restart | `69cbb68`, `54298dc`, `219c749` |

### Configuration

| Plan | Description | Git References |
|------|-------------|----------------|
| [Unified Model Config](2026-04-05-unified-model-config.md) | Merge model cards into ModelConfig with unified fields | `95c8e01`, `13bc2d3`, `0be825a` |
| [Grouped Args Formats](2026-04-06-grouped-args-formats.md) | shlex helpers, grouped args format, auto-migration | `5c8fac1`, `3fbf27b`, `ae67a0b` |

### Code Quality

| Plan | Description | Git References |
|------|-------------|----------------|
| [Code Quality Improvements](2026-03-25-code-quality-improvements.md) | Dead code cleanup, unused imports, formatting | `a93e639`, `423ec0b` |
| [Fix Download Progress Bar](2026-03-27-fix-download-progress-bar.md) | Content-Length parsing, finish_and_clear fixes | `bc35068`, `bd9ea75`, `f052bba` |
| [Preserve GGUF in Names](2026-03-27-preserve-gguf-in-names.md) | Preserve -GGUF suffix in model IDs and paths | `c102bd0`, `58ad0b4` |

### Lifecycle & Shutdown

| Plan | Description | Git References |
|------|-------------|----------------|
| [Proxy Shutdown](2026-04-06-proxy-shutdown.md) | Graceful shutdown method for ProxyState | `6c83743`, `82ec8ab` |
| [System Restart](2026-04-06-system-restart.md) | Process-level restart handler with graceful exit | `3a1b7a0`, `eea20ef`, `ec0fc08`, `0fe3ab5` |

### Migration Tasks

| Plan | Description | Git References |
|------|-------------|----------------|
| [Migrate Profiles to Model Cards Tests](2026-03-24-migrate_profiles_to_model_cards_tests.md) | Tests integrated into unified model config | `95c8e01` |
| [Model Card Cleanup](2026-03-24-model-card-cleanup.md) | Part of unified model config | `95c8e01` |
| [Remove Profiles.d](2026-03-24-remove-profiles-d.md) | Part of unified model config | `95c8e01` |

## Remaining Work

Only **one major feature** remains to be implemented:

| Plan | Description | Status |
|------|-------------|--------|
| [VLLM Backend](2026-03-30-vllm-backend.md) | Add vLLM as a first-class backend type with PyPI version checking | 🚧 NOT STARTED |

## Superseded Plans

| Plan | Description | Status |
|------|-------------|--------|
| [Koji Web Control Plane](2026-04-03-koji-web-control-plane.md) | Core UI implemented, some features pending | ✅ PARTIALLY COMPLETED |
| [Dashboard Time Series Graphs](2026-04-06-dashboard-time-series-graphs.md) | Superseded by persist-dashboard-metrics | 🔁 SUPERSEDED |

## Related Documentation

- [README.md](../README.md) - Project overview
- [AGENTS.md](../AGENTS.md) - Development guide and conventions
- [TODO.md](../TODO.md) - High-level development plan
- [MIGRATION.md](../MIGRATION.md) - Migration guides

## How to Use This Directory

1. **Find a plan** - Browse by category or use the search function
2. **Read the plan** - Understand the goal, architecture, and tasks
3. **Check status** - See if it's completed, in progress, or remaining
4. **Verify implementation** - Follow git references to see commits
5. **Track remaining work** - Use TODO.md for high-level tracking

## Contributing

When implementing a new feature:

1. Create a new plan file in this directory with date prefix (YYYY-MM-DD)
2. Follow the template: Goal, Architecture, Tech Stack, Tasks
3. Mark tasks as `[ ]` (not started) or `[x]` (completed)
4. Link to related plans when applicable
5. Update this README with the new plan
6. Update TODO.md with the new task

## Related Files

- [`docs/openapi/koji-api.yaml`](../openapi/koji-api.yaml) - Machine-readable OpenAPI spec
- [`docs/openapi/openai-compat.yaml`](../openapi/openai-compat.yaml) - OpenAI-compatible API spec

---

**Last Updated**: 2026-04-06
