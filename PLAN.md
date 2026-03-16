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
- [x] **Kronk personality** — Quotes throughout CLI ("Pull the lever!", "Wrong lever!", "Oh yeah, it's all coming together.")
- [x] **Inno Setup installer** — Windows installer that adds to PATH, creates Start Menu entries, cleans up on uninstall
- [x] **GitHub Actions release** — Triggers on version tags, builds Windows (exe + installer) and Linux (.deb + .rpm) packages
- [x] **Mock backend** — Testable fake LLM server with crash simulation and hang detection

## In Progress

- [ ] **First public release** — v0.1.0 CI build running

## Planned

- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop)
- [ ] **`kronk remove <profile>`** — Delete a profile from config
- [ ] **`kronk list`** — List all profiles with their backends and status
- [ ] **Multi-port support** — Different profiles on different ports, firewall rules per profile
- [ ] **Log viewer** — `kronk logs` to tail service log files
- [ ] **Health check customization** — Per-profile health check URLs and intervals
- [ ] **Model download** — `kronk pull <huggingface-url>` to download GGUF models
- [ ] **Tauri GUI** — Lightweight Windows frontend for non-CLI users
