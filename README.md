# KRONK

> The Heavy-Lifting Henchman for Local AI

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![Build Status](https://img.shields.io/badge/build-passing-brightgreen.svg)](https://github.com/danielcherubini/kronk)

A high-performance, Rust-native cross-platform Service Orchestrator for local AI binaries. KRONK provides a system-level management layer that allows you to run models as persistent services with a power-user TUI and optional GUI.

---

## 🎯 Overview

KRONK bridges the gap between raw local AI binaries (like `ik_llama`, `llama.cpp`) and a polished, production-ready experience. Think of it as bringing the "Ollama experience" to specialized Windows binaries with optimization flags.

### What KRONK Does

- **Service Wrapping**: Run `ik_llama.exe` as a persistent Windows Service that survives user logout
- **Auto-Recovery**: Built-in process supervisor with exponential backoff restart logic
- **Cross-Platform**: Linux (systemd) and Windows (SCM) service integration from day one
- **Configurable Profiles**: TOML-based configuration for switching between "Speed" and "Precision" settings
- **War Room Dashboard**: Real-time TUI showing VRAM usage, tokens/sec, and live logs

---

## 🚀 Quick Start

### Prerequisites

- Rust 1.75+ ([install](https://rustup.rs/))
- A local AI binary (e.g., `ik_llama.exe`, `llama.cpp`)

### Installation

```bash
# Clone and build
git clone https://github.com/danielcherubini/kronk.git
cd kronk
cargo build --release

# Run the mock backend for testing
cargo run --bin kronk-mock -- --port 8080

# Run with a backend profile
cargo run --bin kronk -- run --backend mock
```

### Configuration

KRONK auto-creates a config file at `~/.config/kronk/config.toml`:

```toml
[general]
log_level = "info"
data_dir = "~/.local/share/kronk"

[backends.ik_llama]
path = "C:\\path\\to\\ik_llama.exe"
default_args = ["--CUDA_GRAPH_OPT=1", "-sm", "graph"]

[profiles.speed]
backend = "ik_llama"
args = ["--quant=Q4_K_M", "-t", "8"]

[profiles.precision]
backend = "ik_llama"
args = ["--quant=Q8_0", "-t", "4"]

[supervisor]
restart_policy = "always"
max_restarts = 10
restart_delay_ms = 2000
health_check_interval_ms = 5000
hang_timeout_ms = 30000
```

---

## 📦 Project Structure

```
kronk/
├── crates/
│   ├── kronk-core/      # Core library: config, process supervisor, platform abstraction
│   ├── kronk-cli/       # CLI binary with clap commands
│   └── kronk-mock/      # Testable mock LLM backend
├── SPEC.md              # Technical specification
└── README.md
```

### Crate Descriptions

| Crate | Purpose |
|-------|---------|
| `kronk-core` | Shared library with config loading, process supervision, and platform traits |
| `kronk-cli` | Command-line interface with subcommands for service management |
| `kronk-mock` | Mock backend for testing and development |

---

## 🛠️ CLI Commands

```bash
# Run a profile
kronk run --profile speed

# Manage services
kronk service install --profile speed
kronk service start --profile speed
kronk service stop --profile speed
kronk service remove --profile speed

# Check status
kronk status

# View/edit config
kronk config show
kronk config edit
```

---

## 🏗️ Architecture

### Core Components

```
┌─────────────────────────────────────────────────────────────┐
│                        KRONK Orchestrator                     │
├─────────────────────────────────────────────────────────────┤
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐  │
│  │   CLI Layer  │    │  Process     │    │   Platform   │  │
│  │  (clap)      │◄──►│ Supervisor   │◄──►│  Abstraction │  │
│  └──────────────┘    │ (tokio)      │    │  (systemd/   │  │
│                      └──────────────┘    │   SCM)       │  │
│                                         └──────────────┘  │
│                          ┌────────────────────────┐       │
│                          │   Config Management    │       │
│                          │   (TOML + serde)       │       │
│                          └────────────────────────┘       │
└─────────────────────────────────────────────────────────────┘
```

### Platform Support

- **Linux**: systemd user unit files
- **Windows**: Service Control Manager (SCM) via `windows-service` crate

---

## 🎨 The "War Room" Experience

KRONK is designed with a playful "Lever Lab" theme:

- **CLI**: "Pull the lever!" motif (`kronk pull <model>`)
- **TUI**: "War Room" dashboard with live VRAM/CPU visualization
- **Future**: System tray integration with golden lever icon

---

## 🔬 Under the Hood

### Process Supervision

The `ProcessSupervisor` uses tokio for non-blocking I/O and process management:

- Spawns child processes with captured stdout/stderr
- Health checks via periodic interval ticks
- Auto-restart with exponential backoff
- Event-driven architecture via `mpsc` channels

### Configuration

Config is loaded from `~/.config/kronk/config.toml` using `serde` and `toml`. Default config is auto-generated on first run.

---

## 📚 Development

### Building

```bash
cargo build --release
```

### Testing

```bash
# Build and run mock backend
cargo run --bin kronk-mock -- --port 8080 --crash-after 10

# Test CLI
cargo run --bin kronk -- run --backend mock
```

### Dependencies

Key crates:
- `tokio` - Async runtime and process management
- `clap` - CLI parsing
- `serde` / `toml` - Configuration
- `sysinfo` - System metrics (VRAM, CPU)
- `directories` - User directory discovery

---

## 🎯 Roadmap

- [ ] **TUI Dashboard** (`kronk-tui` crate with ratatui)
- [ ] **Real Backend Integration** (test with `ik_llama`)
- [ ] **System Tray** (Windows tray icon integration)
- [ ] **Tauri GUI** (Lightweight Windows frontend)

---

## 📄 License

MIT License — see [LICENSE](LICENSE) for details.

---

## 🙏 Acknowledgments

Inspired by the simplicity and elegance of Ollama. Built in Rust for performance and safety.