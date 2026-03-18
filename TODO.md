# KRONK Development Plan

## Completed
- [x] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff
- [x] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions

## Planned
- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop)
- [ ] **Tauri GUI** — Lightweight desktop frontend for non-CLI users
