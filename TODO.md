# KRONK Development Plan

## Completed
- [x] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff
- [x] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions

## Planned
- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs ([plan](docs/superpowers/plans/2026-03-16-tui-dashboard.md))
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop) ([plan](docs/superpowers/plans/2026-03-16-system-tray.md))
- [ ] **Tauri GUI** — Lightweight desktop frontend for non-CLI users ([plan](docs/superpowers/plans/2026-03-16-tauri-gui.md))
