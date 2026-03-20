<div align="center">

<img src="https://raw.githubusercontent.com/danielcherubini/kronk/main/icon.png" alt="Kronk" height="96" />

# KRONK

> Oh yeah, it's all coming together.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Crates.io](https://img.shields.io/crates/v/kronk.svg)](https://crates.io/crates/kronk)
[![Build Status](https://img.shields.io/github/actions/workflow/status/danielcherubini/kronk/ci.yml?label=CI&style=flat-square)](https://github.com/danielcherubini/kronk/actions)

</div>

A local AI server written in Rust. Kronk provides an OpenAI-compatible API on a single port, automatically managing backend lifecycles — starting models on demand, routing requests, and unloading idle models to save resources.

Think of it as your own local Ollama or LM Studio server, but for llama.cpp and ik_llama backends.

> [!TIP]
> Get up and running: `kronk model pull bartowski/OmniCoder-8B-GGUF && kronk serve`

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

### Pull a model from HuggingFace

```bash
kronk model pull bartowski/OmniCoder-8B-GGUF
```

Kronk downloads all available quants, detects your GPU VRAM, and suggests optimal context sizes.

### Start the server

```bash
kronk serve
```

That's it. Kronk starts an OpenAI-compatible server on `http://localhost:11434`. When a request comes in for a model, Kronk automatically starts the right backend, waits for it to be ready, and forwards the request.

```bash
curl http://localhost:11434/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "bartowski/OmniCoder-8B-GGUF",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

Models are unloaded after 5 minutes of inactivity (configurable with `--idle-timeout`).

### Install as a system service

```bash
# Install and start (run as admin / sudo)
kronk service install
kronk service start

# After that, no admin needed
kronk service stop
kronk service start
kronk status
```

> [!NOTE]
> For debugging individual backends, you can still use `kronk run <server-name>` to run a single server in the foreground.

---

## CLI

```text
kronk serve [--host H] [--port P] [--idle-timeout S]  Start the server
kronk status                                       Show status of all servers
kronk service install                              Install as a system service
kronk service start                                Start the service
kronk service stop                                 Stop the service
kronk service remove                               Remove the service
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
kronk logs [name]                                  View logs (defaults to proxy logs)
kronk run <name> [--ctx N]                         Run a single backend (for debugging)
```

### Backend Management

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
- **Linux/macOS:** Backends in `~/.config/kronk/backends/`
- **Windows:** Backends in `%APPDATA%\kronk\backends\`
- Version tracking in `~/.config/kronk/backend_registry.toml` (Linux/macOS) or `%APPDATA%\kronk\backend_registry.toml` (Windows)

### GPU Support

The installer detects your GPU and prompts you to select acceleration:

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

[servers.my-model]
backend = "llama_cpp"
model = "bartowski/OmniCoder-8B-GGUF"
quant = "Q4_K_M"
profile = "coding"
enabled = true

[proxy]
host = "0.0.0.0"
port = 11434
idle_timeout_secs = 300

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 3000
health_check_interval_ms = 5000
```

You can define multiple backends and servers. When `kronk serve` is running, request any configured model and it will be started automatically. Backend ports are auto-assigned — you don't need to configure them.

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

`kronk service install` automatically adds an inbound firewall rule for port 11434. `kronk service remove` cleans it up.

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

