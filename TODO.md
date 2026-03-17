# KRONK Development Plan

## Planned
- [ ] **Multi-port support** — Per-profile port config, auto `--port` injection, per-port firewall rules
- [ ] **Log viewer** — `kronk logs` with `--follow`, log rotation, ProcessSupervisor file output
- [ ] **Health check customization** — Per-profile health check URL, interval, timeout, retries
- [ ] **Parallel downloads** — Multi-connection Range downloads for GGUF files, ~3x speedup
- [ ] **Windows service polling** — Replace fixed sleeps with proper SCM status polling and backoff
- [ ] **Windows service SID ACL** — Use installer's SID instead of IU for service permissions
- [ ] **TUI Dashboard** — `kronk-tui` crate with ratatui. War Room view: live VRAM, tokens/sec, temperature, logs
- [ ] **System tray** — Windows tray icon for quick service toggle (start/stop)
- [ ] **Tauri GUI** — Lightweight desktop frontend for non-CLI users
