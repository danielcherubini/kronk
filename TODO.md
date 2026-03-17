# KRONK Development Plan

## Completed

- [x] **Workspace scaffold** — 3-crate workspace (kronk-core, kronk-cli, kronk-mock)
- [x] **Config system** — TOML-based, dynamic HashMap backends/profiles, auto-generated defaults
- [x] **Process supervisor** — tokio-based, stdout/stderr streaming, HTTP health checks, auto-restart with backoff, ctrl-c handling
- [x] **Windows Service** — Native SCM integration via `windows-service` crate. kronk.exe is both CLI and service binary. No NSSM dependency
- [x] **Linux Service** — systemd user unit management (install/start/stop/remove/status)
- [x] **CLI commands** — run, stop, status, service (install/start/stop/remove), config (show/edit/path), add, update
- [x] **Profile management** — `kronk add <name> <command...>` creates profiles from raw command lines, `kronk update` modifies existing
- [x] **Firewall rules** — Auto-added on service install, removed on service remove (Windows)
- [x] **VRAM monitoring** — `kronk status` shows GPU VRAM usage via nvidia-smi
- [x] **User-level service control** — Interactive Users granted start/stop permissions, no admin needed after install
- [x] **Clean process shutdown** — Service stop properly kills child processes via shutdown channel + kill_on_drop
- [x] **Kronk personality** — Quotes throughout CLI ("Pull the lever!", "Wrong lever!", "Oh yeah, it's all coming together.")
- [x] **Inno Setup installer** — Windows installer that adds to PATH, creates Start Menu entries, cleans up on uninstall
- [x] **GitHub Actions release** — Triggers on version tags, builds Windows (exe + installer) and Linux (.deb + .rpm) packages
- [x] **Mock backend** — Testable fake LLM server with crash simulation and hang detection
- [x] **Use case system** — Per-profile use case presets (coding, chat, analysis, creative, custom) with sampling parameter auto-tuning and merge logic
- [x] **Model registry** — `~/.kronk/models/{company}/{model}/` with TOML model cards, `kronk model` subcommand (pull, ls, ps, create, rm, scan)
- [x] **HuggingFace pull** — `kronk model pull <repo>` downloads GGUFs with interactive quant selection, auto-resolves `-GGUF` repo suffix
- [x] **Community model cards** — Curated sampling presets fetched from GitHub on pull (e.g. Tesslate/OmniCoder-9B)
- [x] **3-layer sampling merge** — UseCase defaults → model card overrides → profile overrides, applied in all run/service paths
- [x] **Model-linked profiles** — `kronk model create` builds profiles from model cards with `--backend`, `--quant`, `--use-case` flags
- [x] **First public release** — v0.1.0 CI build running
- [x] **Model search** — `kronk model search <query>` to discover GGUF models from HuggingFace with download counts ([plan](docs/superpowers/plans/2026-03-16-model-search.md))
- [x] **Profile management** — `kronk profile` subcommand: ls, add, edit, rm with service safety checks ([plan](docs/superpowers/plans/2026-03-16-profile-management.md))

## Planned
- [ ] **Multi-port support** — Per-profile port config, auto `--port` injection, per-port firewall rules ([plan](docs/superpowers/plans/2026-03-16-multi-port.md))
- [ ] **Log viewer** — `kronk logs` with `--follow`, log rotation, ProcessSupervisor file output ([plan](docs/superpowers/plans/2026-03-16-log-viewer.md))
- [ ] **Health check customization** — Per-profile health check URL, interval, timeout, retries ([plan](docs/superpowers/plans/2026-03-16-health-check-customization.md))
- [ ] **Parallel downloads** — Multi-connection Range downloads for GGUF files, ~3x speedup ([plan](docs/superpowers/plans/2026-03-16-parallel-downloads.md))
- [ ] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff ([plan](docs/superpowers/plans/2026-03-16-windows-service-polling.md))
- [ ] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions ([plan](docs/superpowers/plans/2026-03-16-windows-service-sid.md))
- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs ([plan](docs/superpowers/plans/2026-03-16-tui-dashboard.md))
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop) ([plan](docs/superpowers/plans/2026-03-16-system-tray.md))
- [ ] **Tauri GUI** — Lightweight desktop frontend for non-CLI users ([plan](docs/superpowers/plans/2026-03-16-tauri-gui.md))
