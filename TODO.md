# KRONK Development Plan

## Planned
- [x] **Parallel downloads** — Multi-connection Range downloads for GGUF files, ~3x speedup ([PR #7](https://github.com/danielcherubini/kronk/pull/7))
- [ ] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff ([plan](docs/superpowers/plans/2026-03-16-windows-service-polling.md))
- [ ] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions ([plan](docs/superpowers/plans/2026-03-16-windows-service-sid.md))
- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs ([plan](docs/superpowers/plans/2026-03-16-tui-dashboard.md))
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop) ([plan](docs/superpowers/plans/2026-03-16-system-tray.md))
- [ ] **Tauri GUI** — Lightweight desktop frontend for non-CLI users ([plan](docs/superpowers/plans/2026-03-16-tauri-gui.md))
