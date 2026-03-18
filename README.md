<div align="center">

<img src="https://raw.githubusercontent.com/danielcherubini/kronk/main/icon.png" alt="Kronk" height="96" />

# KRONK

> Oh yeah, it's all coming together.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Crates.io](https://img.shields.io/crates/v/kronk.svg)](https://crates.io/crates/kronk)
[![Build Status](https://img.shields.io/github/actions/workflow/status/danielcherubini/kronk/ci.yml?label=CI&style=flat-square)](https://github.com/danielcherubini/kronk/actions)

</div>

A local AI service manager written in Rust. Kronk turns your `llama-server.exe` (or any LLM backend) into a proper, supervised system service with health checks, auto-restart, and easy configuration.

No wrappers. No NSSM. No batch files. Just a native Windows Service or systemd unit. After a one-time admin install, start and stop services as a regular user.

> [!TIP]
> Get up and running in minutes with `kronk add default llama-server.exe --host 0.0.0.0 -m model.gguf -ngl 999`

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

### Add a server from a command you already use

```bash
kronk add default llama-server.exe --host 0.0.0.0 -m model.gguf -ngl 999 -fa 1 -c 8192
```

Kronk figures out the backend from the binary path and saves everything to config.

### Pull a model from HuggingFace

```bash
kronk model pull bartowski/OmniCoder-8B-GGUF
```

Kronk downloads all available quants, detects your GPU VRAM, and suggests optimal context sizes.

### Create a server from an installed model

```bash
kronk model create my-server --model bartowski/OmniCoder-8B-GGUF --profile coding
```

Kronk auto-configures the server with the selected quant and sampling profile.

### Run it

```bash
# Foreground (with live output)
kronk run default

# Override context size (takes priority over model card)
kronk run default --ctx 8192

# Or install as a system service (run as admin / sudo)
kronk service install
kronk service start

# After that, no admin needed
kronk service stop
kronk service start
kronk status
```

Kronk supervises the process, streams logs, checks health, and restarts on crash.

> [!NOTE]
> After installing the service, you can run `kronk run <name>` in the foreground to monitor logs and debug issues.

---

## CLI

```text
kronk run <name> [--ctx N]                         Run a server in the foreground
kronk status                                       Show status of all servers
kronk service install [name]                       Install as a system service
kronk service start [name]                         Start an installed service
kronk service stop [name]                          Stop a running service
kronk service remove [name]                        Remove an installed service
kronk server ls                                    List all servers with status
kronk server add <name> <cmd...>                   Add a new server from a raw command
kronk server edit <name> <cmd...>                  Edit an existing server
kronk server rm <name>                             Remove a server
kronk profile list                                 List available sampling profiles
kronk profile set <server> <profile>               Set a server's sampling profile
kronk profile clear <server>                       Clear a server's profile
kronk profile add <name> [--temp ...] [--top-k ...]  Create a custom profile
kronk profile remove <name>                        Remove a custom profile
kronk model pull <repo>                            Pull a model from HuggingFace
kronk model ls                                     List installed models
kronk model ps                                     Show running model processes
kronk model create <name>                          Create a server from an installed model
kronk model rm <model>                             Remove an installed model
kronk model scan                                   Scan for untracked GGUF files
kronk model search <query>                         Search HuggingFace for GGUF models
kronk config show                                  Print the current configuration
kronk config edit                                  Open config file in editor
kronk config path                                  Show the config file path
kronk logs <name>                                  View backend logs (follow with -f)

## Backend Management

Kronk manages LLM backend installations (llama.cpp, ik_llama) with automatic version tracking and updates:

```bash
kronk backend install llama_cpp    # Install latest llama.cpp
kronk backend install ik_llama     # Install latest ik_llama (builds from source)
kronk backend install llama_cpp --version b8407  # Install specific version
kronk backend install llama_cpp --build    # Force build from source
kronk backend update <name>        # Update to latest version
kronk backend list                 # List installed backends
kronk backend remove <name>        # Remove a backend
kronk backend check-updates        # Check for updates
```

### Installation Details

- **llama.cpp**: Downloads pre-built binaries for your platform, or builds from source with GPU support
- **ik_llama**: Always builds from source (no pre-built binaries available)
- Backends are stored in `~/.config/kronk/backends/`
- Version tracking is stored in `~/.config/kronk/backend_registry.toml`

### GPU Support

The installer auto-detects your GPU and offers GPU-accelerated builds:

- **CUDA** (NVIDIA) — CUDA cores for faster inference
- **Vulkan** (AMD/Intel/NVIDIA) — Cross-platform GPU acceleration
- **Metal** (Apple Silicon) — macOS GPU acceleration
- **ROCm** (AMD) — AMD GPU support on Linux
- **CPU** — Fallback when no GPU is available

---

---

## Configuration

Kronk auto-generates a config on first run:

- **Windows:** `%APPDATA%\kronk\config\config.toml`
- **Linux:** `~/.config/kronk/config.toml`

```toml
[backends.llama_cpp]
path = "C:\\path\\to\\llama-server.exe"
health_check_url = "http://localhost:8080/health"

[servers.default]
backend = "llama_cpp"
args = ["--host", "0.0.0.0", "-m", "model.gguf", "-ngl", "999", "-c", "8192"]
profile = "coding"
enabled = true

[servers.model-server]
backend = "llama_cpp"
model = "bartowski/OmniCoder-8B-GGUF"
quant = "Q4_K_M"
profile = "coding"
enabled = true

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 3000
health_check_interval_ms = 5000
```

You can define multiple backends and servers. Switch between them with `kronk run <name>`.

Model cards are stored in `~/.config/kronk/configs.d/<company>--<model>.toml` and contain quant info, context settings, and sampling presets.

### Directory Layout

```text
~/.config/kronk/
├── config.toml              Main configuration
├── profiles.d/              Sampling presets (editable)
│   ├── coding.toml
│   ├── chat.toml
│   ├── analysis.toml
│   └── creative.toml
├── configs.d/               Model cards
│   └── bartowski--OmniCoder-8B.toml
├── models/                  GGUF model files
│   └── bartowski/OmniCoder-8B/*.gguf
└── logs/                    Service logs
```

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

## Architecture

```
kronk/
├── crates/
│   ├── kronk-core/      # Config, process supervisor, platform abstraction
│   ├── kronk-cli/       # CLI binary (clap)
│   └── kronk-mock/      # Mock LLM backend for testing
├── installer/           # Inno Setup script (Windows installer)
├── modelcards/          # Community model cards
├── .github/workflows/   # CI/CD release pipeline
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

See [TODO.md](TODO.md) for the full development plan:

- **Multi-port support** — Per-server port config, auto `--port` injection
- **Log viewer** — `kronk logs` with `--follow`, log rotation
- **Parallel downloads** — Multi-connection Range downloads for GGUF files
- **Windows service polling** — Replace fixed sleeps with proper SCM status polling
- **TUI Dashboard** — `kronk-tui` crate with ratatui
- **System tray** — Windows tray icon for quick service toggle
- **Tauri GUI** — Lightweight desktop frontend for non-CLI users

---

## Development

Kronk is built with modern Rust and follows these core crates:

- **kronk-core** — Core logic, process supervision, config management, platform abstractions
- **kronk-cli** — Command-line interface with clap, user prompts with inquire
- **kronk-mock** — Mock backend for testing and development

### Dependencies

Key dependencies include:

- `tokio` — Async runtime with process management
- `clap` — CLI parsing
- `serde` / `toml` — Configuration serialization
- `tracing` — Structured logging
- `reqwest` / `hf-hub` — HTTP client and HuggingFace integration
- `sysinfo` — System resource monitoring
- `indicatif` — Progress bars for downloads
- `directories` — Platform-specific config paths

