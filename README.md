# KRONK

> Oh yeah, it's all coming together.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)

A local AI service manager written in Rust. Kronk turns your `llama-server.exe` (or any LLM backend) into a proper, supervised system service with health checks, auto-restart, and easy configuration.

No wrappers. No NSSM. No batch files. Just a native Windows Service or systemd unit.

---

## Quick Start

### Install

**Windows:** Download the installer from [Releases](https://github.com/danielcherubini/kronk/releases), or:

```bash
cargo install --git https://github.com/danielcherubini/kronk kronk
```

**Linux (Debian/Ubuntu):**
```bash
sudo dpkg -i kronk_*.deb
```

**Linux (Fedora/RHEL):**
```bash
sudo rpm -i kronk-*.rpm
```

### Add a profile from a command you already use

```bash
kronk add default llama-server.exe --host 0.0.0.0 -m model.gguf -ngl 999 -fa 1 -c 8192
```

Kronk figures out the backend from the binary path and saves everything to config.

### Run it

```bash
# Foreground (with live output)
kronk run

# Install as a system service (run as admin / sudo)
kronk service install
```

That's it. Kronk supervises the process, streams logs, checks health, and restarts on crash.

---

## CLI

```
kronk run [--profile name]           Run a profile in the foreground
kronk stop [--profile name]          Stop a running service
kronk status                         Show all profiles and health status
kronk service install [--profile]    Install as a system service
kronk service start [--profile]      Start an installed service
kronk service stop [--profile]       Stop a running service
kronk service remove [--profile]     Remove an installed service
kronk add <name> <command...>        Create a profile from a raw command
kronk update <name> <command...>     Update an existing profile
kronk config show                    Print current config
kronk config edit                    Open config in editor
kronk config path                    Show config file location
```

---

## Configuration

Kronk auto-generates a config on first run:

- **Windows:** `%APPDATA%\kronk\config\config.toml`
- **Linux:** `~/.config/kronk/config.toml`

```toml
[backends.llama_cpp]
path = "C:\\path\\to\\llama-server.exe"
health_check_url = "http://localhost:8080/health"

[profiles.default]
backend = "llama_cpp"
args = ["--host", "0.0.0.0", "-m", "model.gguf", "-ngl", "999", "-c", "8192"]

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 3000
health_check_interval_ms = 5000
```

You can define multiple backends and profiles. Switch between them with `--profile`.

---

## How It Works

### Process Supervision

Kronk spawns your LLM backend as a child process and watches it:

- Streams stdout/stderr in real-time
- Periodic HTTP health checks against your server
- Auto-restart with configurable backoff on crash
- Clean shutdown on ctrl-c or service stop

### Service Integration

- **Windows:** Native Service Control Manager via the `windows-service` crate. `kronk.exe` registers itself as a Windows Service — no NSSM or wrapper needed. Auto-starts on boot.
- **Linux:** Generates and manages systemd user units. `kronk service install` creates the unit file, enables it, and starts the service.

### Firewall (Windows)

`kronk service install` automatically adds an inbound firewall rule for port 8080. `kronk service remove` cleans it up.

---

## Project Structure

```
kronk/
├── crates/
│   ├── kronk-core/      # Config, process supervisor, platform abstraction
│   ├── kronk-cli/       # CLI binary (clap)
│   └── kronk-mock/      # Mock LLM backend for testing
├── installer/           # Inno Setup script (Windows installer)
├── .github/workflows/   # CI/CD release pipeline
├── SPEC.md              # Original technical specification
├── PLAN.md              # Development roadmap
└── README.md
```

---

## Building from Source

```bash
git clone https://github.com/danielcherubini/kronk.git
cd kronk
cargo build --release
```

The binary is at `target/release/kronk.exe` (Windows) or `target/release/kronk` (Linux).

---

## Roadmap

See [PLAN.md](PLAN.md) for the full development plan.

- [x] Native Windows Service (no NSSM)
- [x] Linux systemd support
- [x] Process supervision with health checks
- [x] Profile management from CLI
- [x] GitHub Actions CI/CD with Windows installer + .deb + .rpm
- [ ] TUI Dashboard (ratatui)
- [ ] System tray integration
- [ ] Model download (`kronk pull`)

---

## License

MIT License — see [LICENSE](LICENSE) for details.
