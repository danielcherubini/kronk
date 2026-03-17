# KRONK Development Plan

## Planned
- [ ] **Log viewer** — `kronk logs` with `--follow`, log rotation, ProcessSupervisor file output ([plan](docs/superpowers/plans/2026-03-16-log-viewer.md))
- [ ] **Health check customization** — Per-profile health check URL, interval, timeout, retries ([plan](docs/superpowers/plans/2026-03-16-health-check-customization.md))
- [ ] **Parallel downloads** — Multi-connection Range downloads for GGUF files, ~3x speedup ([plan](docs/superpowers/plans/2026-03-16-parallel-downloads.md))
- [ ] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff ([plan](docs/superpowers/plans/2026-03-16-windows-service-polling.md))
- [ ] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions ([plan](docs/superpowers/plans/2026-03-16-windows-service-sid.md))
- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs ([plan](docs/superpowers/plans/2026-03-16-tui-dashboard.md))
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop) ([plan](docs/superpowers/plans/2026-03-16-system-tray.md))
- [ ] **Tauri GUI** — Lightweight desktop frontend for non-CLI users ([plan](docs/superpowers/plans/2026-03-16-tauri-gui.md))
