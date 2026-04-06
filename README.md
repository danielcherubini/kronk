<div align="center">

# Koji

> A local AI server with automatic backend management.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Build Status](https://img.shields.io/github/actions/workflow/status/danielcherubini/koji/ci.yml?label=CI&style=flat-square)](https://github.com/danielcherubini/koji/actions)

</div>

A local AI server written in Rust. Koji provides an OpenAI-compatible API on a single port, automatically managing backend lifecycles — starting models on demand, routing requests, and unloading idle models to save resources.

Think of it as your own local Ollama or LM Studio server, but for llama.cpp and ik_llama backends.

> [!TIP]
> Get up and running: `koji model pull bartowski/OmniCoder-8B-GGUF && koji serve`

---

## Quick Start

### Install

**Windows:** Download the installer from [Releases](https://github.com/danielcherubini/koji/releases), or:

```bash
cargo install --git https://github.com/danielcherubini/koji koji
```

**Linux (Debian/Ubuntu):**

```bash
sudo dpkg -i koji_*.deb
```

**Linux (Fedora/RHEL):**

```bash
sudo rpm -i koji-*.rpm
```

### Pull a model from HuggingFace

```bash
koji model pull bartowski/OmniCoder-8B-GGUF
```

Koji downloads all available quants, detects your GPU VRAM, and suggests optimal context sizes.

### Start the server

```bash
koji serve
```

That's it. Koji starts an OpenAI-compatible server on `http://localhost:11434`. When a request comes in for a model, Koji automatically starts the right backend, waits for it to be ready, and forwards the request.

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
koji service install
koji service start

# After that, no admin needed
koji service stop
koji service start
koji status
```

> [!NOTE]
> For debugging individual backends, you can still use `koji run <server-name>` to run a single server in the foreground.

---

## Web Control Plane

Koji includes a web-based control plane UI for managing models, viewing logs, and editing config from a browser.

```bash
# 1. Build the frontend (requires trunk: cargo install trunk)
cd crates/koji-web && trunk build --release && cd ../..

# 2. Start the web server (port 11435 by default)
cargo run --package koji-web --features ssr

# Or via the CLI (with web-ui feature):
cargo run --package koji --features web-ui -- web --port 11435

# 3. Open http://localhost:11435
```

The web UI proxies all `/koji/v1/` requests to the running Koji proxy (default `http://127.0.0.1:11434`).

The web server starts automatically alongside the proxy when using `koji service start`.

---

## CLI

```text
koji serve [--host H] [--port P] [--idle-timeout S]  Start the server
koji status                                       Show status of all servers
koji service install                              Install as a system service
koji service start                                Start the service
koji service stop                                 Stop the service
koji service remove                               Remove the service
koji model pull <repo>                            Pull a model from HuggingFace
koji model ls                                     List installed models
koji model ps                                     Show running model processes
koji model create <name>                          Create a server from an installed model
koji model rm <model>                             Remove an installed model
koji model scan                                   Scan for untracked GGUF files
koji model search <query>                         Search HuggingFace for GGUF models
koji config show                                  Print the current configuration
koji config edit                                  Open config file in editor
koji config path                                  Show the config file path
koji logs [name]                                  View logs (defaults to proxy logs)
koji run <name> [--ctx N]                         Run a single backend (for debugging)
```

### Backend Management

Koji manages LLM backend installations (llama.cpp, ik_llama) with automatic version tracking and updates:

```bash
koji backend install llama_cpp    # Install latest llama.cpp
koji backend install ik_llama     # Install latest ik_llama (builds from source)
koji backend install llama_cpp --version b8407  # Install specific version
koji backend install llama_cpp --build    # Force build from source
koji backend update <name>        # Update to latest version
koji backend list                 # List installed backends
koji backend remove <name>        # Remove a backend
koji backend check-updates        # Check for updates
```

### Installation Details

- **llama.cpp**: Downloads pre-built binaries for your platform, or builds from source with GPU support
- **ik_llama**: Always builds from source (no pre-built binaries available)
- **Linux/macOS:** Backends in `~/.config/koji/backends/`
- **Windows:** Backends in `%APPDATA%\koji\backends\`
- Version tracking in `~/.config/koji/backend_registry.toml` (Linux/macOS) or `%APPDATA%\koji\backend_registry.toml` (Windows)

### GPU Support

The installer detects your GPU and prompts you to select acceleration:

- **CUDA** (NVIDIA) — CUDA cores for faster inference
- **Vulkan** (AMD/Intel/NVIDIA) — Cross-platform GPU acceleration
- **Metal** (Apple Silicon) — macOS GPU acceleration
- **ROCm** (AMD) — AMD GPU support on Linux
- **CPU** — Fallback when no GPU is available

---

## Configuration

Koji auto-generates a config on first run:

- **Windows:** `%APPDATA%\koji\config\config.toml`
- **Linux:** `~/.config/koji/config.toml`

```toml
[backends.llama_cpp]
path = "C:\\path\\to\\llama-server.exe"
health_check_url = "http://localhost:8080/health"

[models.my-model]
backend = "llama_cpp"
model = "bartowski/OmniCoder-8B-GGUF"
quant = "Q4_K_M"
profile = "coding"
enabled = true

[proxy]
host = "0.0.0.0"
port = 11434
idle_timeout_secs = 300
startup_timeout_secs = 120

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 3000
health_check_interval_ms = 5000
```

The `[models.*]` key (e.g. `my-model`) is the alias used by clients in `"model": "my-model"`. You can define multiple models. When `koji serve` is running, request any enabled model and its backend will start automatically. Backend ports are auto-assigned — you don't need to configure them.

Model cards are stored in `~/.config/koji/configs/<company>--<model>.toml` and contain quant info, context settings, and sampling presets.

### Directory Layout

```text
~/.config/koji/
├── config.toml              Main configuration
├── profiles/              Sampling presets (editable)
│   ├── coding.toml
│   ├── chat.toml
│   ├── analysis.toml
│   └── creative.toml
├── configs/               Model cards
│   └── bartowski--OmniCoder-8B.toml
├── models/                  GGUF model files
│   └── bartowski/OmniCoder-8B/*.gguf
└── logs/                    Service logs
```

> [!NOTE]
> On first run after upgrading from kronk, Koji automatically renames
> `~/.config/kronk` (or `%APPDATA%\kronk`) to `~/.config/koji` (or
> `%APPDATA%\koji`) and renames the `kronk.db` database to `koji.db`.

---

## How It Works

1. `koji serve` starts an OpenAI-compatible API server on a single port (default 11434)
2. When a request arrives with `"model": "my-model"`, koji looks up the config key in `[models.*]`
3. If the backend isn't running, koji auto-assigns a free port, starts the backend with the right GGUF file, and waits for it to become healthy
4. The request is forwarded to the backend and the response is streamed back
5. After `idle_timeout_secs` of inactivity, the backend is shut down to free resources

### Service Integration

- **Windows:** Native Service Control Manager via the `windows-service` crate. `koji service install` registers koji as a Windows Service that auto-starts on boot. No NSSM or wrappers needed.
- **Linux:** Generates and manages systemd user units. `koji service install` creates the unit file, enables it, and starts the service.

### Firewall (Windows)

`koji service install` automatically adds an inbound firewall rule for port 11434. `koji service remove` cleans it up.

---

## Architecture

```
koji/
├── crates/
│   ├── koji-core/       # Config, process supervisor, platform abstraction
│   ├── koji-cli/        # CLI binary (clap)
│   ├── koji-mock/       # Mock LLM backend for testing
│   └── koji-web/        # Leptos web control plane
├── installer/           # Inno Setup script (Windows installer)
├── modelcards/          # Community model cards
├── .github/workflows/   # CI/CD release pipeline
└── README.md
```

---

## Building from Source

```bash
git clone https://github.com/danielcherubini/koji.git
cd koji
cargo build --release
```

The binary is at `target/release/koji.exe` (Windows) or `target/release/koji` (Linux).

---

## Roadmap

- **TUI Dashboard** — `koji-tui` crate with ratatui
- **System tray** — Windows tray icon for quick service toggle
- **Tauri GUI** — Lightweight desktop frontend for non-CLI users

---

## Development

Koji is built with modern Rust and follows these core crates:

- **koji-core** — Core logic, process supervision, config management, platform abstractions
- **koji-cli** — Command-line interface with clap, user prompts with inquire
- **koji-mock** — Mock backend for testing and development
- **koji-web** — Leptos WASM frontend and SSR server for the web control plane

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
