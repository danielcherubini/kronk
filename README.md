# KRONK

> Oh yeah, it's all coming together.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)

A local AI service manager written in Rust. Kronk turns your `llama-server.exe` (or any LLM backend) into a proper, supervised system service with health checks, auto-restart, and easy configuration.

No wrappers. No NSSM. No batch files. Just a native Windows Service or systemd unit. After a one-time admin install, start and stop services as a regular user.

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

### Pull a model from HuggingFace

```bash
kronk model pull bartowski/OmniCoder-8B-GGUF
```

Kronk downloads all available quants, detects your GPU VRAM, and suggests optimal context sizes.

### Create a profile from an installed model

```bash
kronk model create my-profile --model bartowski/OmniCoder-8B-GGUF --use-case coding
```

Kronk auto-configures the profile with the selected quant and use-case preset.

### Run it

```bash
# Foreground (with live output)
kronk run

# Or install as a system service (run as admin / sudo)
kronk service install
kronk service start

# After that, no admin needed
kronk service stop
kronk service start
kronk status
```

Kronk supervises the process, streams logs, checks health, and restarts on crash.

---

## CLI

```
kronk run [--profile name]           Run a profile in the foreground
kronk status                         Show all profiles, health, and VRAM usage
kronk service install [--profile]    Install as a system service
kronk service start [--profile]      Start an installed service
kronk service stop [--profile]       Stop a running service
kronk service remove [--profile]     Remove an installed service
kronk add <name> <command...>        Create a profile from a raw command
kronk update <name> <command...>     Update an existing profile
kronk model pull <repo>              Pull GGUF models from HuggingFace
kronk model ls                       List installed models
kronk model create <name>            Create a profile from an installed model
kronk model rm <model>                Remove an installed model
kronk model scan                     Scan for untracked GGUF files
kronk model search <query>           Search HuggingFace for GGUF models
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

[profiles.model-profile]
backend = "llama_cpp"
model = "bartowski/OmniCoder-8B-GGUF"
quant = "Q4_K_M"
use-case = "coding"

[models.bartowski/OmniCoder-8B-GGUF]
dir = "~/.config/kronk/models/bartowski/OmniCoder-8B-GGUF"
[[models.bartowski/OmniCoder-8B-GGUF.quants]]
name = "Q4_K_M"
file = "OmniCoder-8B-Q4_K_M.gguf"
context = 8192

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 3000
health_check_interval_ms = 5000
```

You can define multiple backends and profiles. Switch between them with `--profile`.

Model cards are stored in `~/.config/kronk/models/<repo>/<model>/model.toml` and contain quant info, context settings, and sampling presets.

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
- [x] VRAM monitoring in status output
- [x] User-level service control (no admin after install)
- [x] Profile management from CLI
- [x] Model management (pull, search, create profiles)
- [x] GitHub Actions CI/CD with Windows installer + .deb + .rpm
- [ ] TUI Dashboard (ratatui)
- [ ] System tray integration

---

## License

MIT License — see [LICENSE](LICENSE) for details.
