# System Tray Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.
> **Status:** TODO - GUI feature, not yet implemented

**Goal:** Add a Windows system tray icon that shows kronk status and lets users start/stop services without opening a terminal.

**Architecture:** New `kronk-tray` crate using the `tray-icon` and `muda` crates for cross-platform tray functionality (Windows primary, Linux optional). The tray runs a background event loop, polls service status periodically, and calls the existing `kronk-core::platform` functions for start/stop. Menu items are dynamically built from `Config::profiles`.

**Tech Stack:** Rust, `tray-icon` (tray icon + tooltip), `muda` (context menu), `winit` or manual event loop, `kronk-core` for config and platform ops, `tokio` for async health checks

---

## File Structure

### New files to create

| File | Responsibility |
|------|---------------|
| `crates/kronk-tray/Cargo.toml` | Crate manifest |
| `crates/kronk-tray/src/main.rs` | Entry point, event loop |
| `crates/kronk-tray/src/menu.rs` | Build context menu from profiles |
| `crates/kronk-tray/src/status.rs` | Poll service status, update menu/tooltip |
| `crates/kronk-tray/build.rs` | Embed icon resource (Windows) |
| `crates/kronk-tray/assets/kronk.ico` | Tray icon |

### Files to modify

| File | Changes |
|------|---------|
| `Cargo.toml` (workspace) | Add `kronk-tray` to workspace members, add `tray-icon` and `muda` deps |

---

## Chunk 1: Tray Crate Skeleton

### Task 1: Create crate and dependencies

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Create: `crates/kronk-tray/Cargo.toml`

- [ ] **Step 1: Add workspace member and dependencies**

In root `Cargo.toml`:

```toml
# Add to [workspace] members
"crates/kronk-tray",

# Add to [workspace.dependencies]
tray-icon = "0.19"
muda = "0.15"
image = { version = "0.25", default-features = false, features = ["png"] }
```

- [ ] **Step 2: Create `crates/kronk-tray/Cargo.toml`**

```toml
[package]
name = "kronk-tray"
version.workspace = true
edition.workspace = true

[dependencies]
kronk-core = { path = "../kronk-core" }
tray-icon.workspace = true
muda.workspace = true
image.workspace = true
tokio.workspace = true
anyhow.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true

[target.'cfg(windows)'.dependencies]
windows-service = "0.7"
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p kronk-tray`
Expected: Error — no src/main.rs yet

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/kronk-tray/Cargo.toml
git commit -m "feat: add kronk-tray crate skeleton"
```

### Task 2: Create minimal tray application

**Files:**
- Create: `crates/kronk-tray/src/main.rs`
- Create: `crates/kronk-tray/src/menu.rs`
- Create: `crates/kronk-tray/src/status.rs`

- [ ] **Step 1: Write main.rs with event loop**

```rust
// crates/kronk-tray/src/main.rs
mod menu;
mod status;

use anyhow::Result;
use kronk_core::config::Config;
use tray_icon::{TrayIcon, TrayIconBuilder};

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::load()?;
    let menu = menu::build_menu(&config)?;
    let icon = load_icon()?;

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Kronk - LLM Service Manager")
        .with_icon(icon)
        .build()?;

    // Run the event loop
    // On Windows this needs a message pump
    #[cfg(target_os = "windows")]
    {
        use tray_icon::menu::MenuEvent;
        let menu_rx = MenuEvent::receiver();
        loop {
            if let Ok(event) = menu_rx.recv() {
                menu::handle_event(&config, &event)?;
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // On other platforms, use a simple loop
        std::thread::park();
    }

    Ok(())
}

fn load_icon() -> Result<tray_icon::Icon> {
    // Load embedded PNG icon
    let icon_bytes = include_bytes!("../assets/kronk.png");
    let img = image::load_from_memory(icon_bytes)?;
    let rgba = img.into_rgba8();
    let (width, height) = rgba.dimensions();
    let icon = tray_icon::Icon::from_rgba(rgba.into_raw(), width, height)?;
    Ok(icon)
}
```

- [ ] **Step 2: Write menu.rs**

```rust
// crates/kronk-tray/src/menu.rs
use anyhow::Result;
use kronk_core::config::Config;
use muda::{Menu, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::menu::MenuEvent;

pub fn build_menu(config: &Config) -> Result<Menu> {
    let menu = Menu::new();

    // Add profile submenus
    for (name, _profile) in &config.profiles {
        let submenu = Submenu::new(name, true);
        submenu.append(&MenuItem::new(format!("Start {}", name), true, None))?;
        submenu.append(&MenuItem::new(format!("Stop {}", name), true, None))?;
        submenu.append(&PredefinedMenuItem::separator())?;
        submenu.append(&MenuItem::new("Status: checking...", false, None))?;
        menu.append(&submenu)?;
    }

    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::new("Open Config", true, None))?;
    menu.append(&MenuItem::new("Quit", true, None))?;

    Ok(menu)
}

pub fn handle_event(config: &Config, event: &MenuEvent) -> Result<()> {
    let id = event.id();
    tracing::debug!("Menu event: {:?}", id);
    // Match menu item IDs to actions
    // TODO: implement start/stop/quit handlers
    Ok(())
}
```

- [ ] **Step 3: Write status.rs**

```rust
// crates/kronk-tray/src/status.rs
use kronk_core::config::Config;

pub struct ProfileStatus {
    pub name: String,
    pub service_state: String,
    pub healthy: bool,
}

/// Poll status of all profiles.
pub fn poll_all(config: &Config) -> Vec<ProfileStatus> {
    config.profiles.keys().map(|name| {
        let service_name = Config::service_name(name);
        let state = {
            #[cfg(target_os = "windows")]
            { kronk_core::platform::windows::query_service(&service_name)
                .unwrap_or_else(|_| "UNKNOWN".to_string()) }
            #[cfg(target_os = "linux")]
            { kronk_core::platform::linux::query_service(&service_name)
                .unwrap_or_else(|_| "UNKNOWN".to_string()) }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            { let _ = &service_name; "N/A".to_string() }
        };

        ProfileStatus {
            name: name.clone(),
            service_state: state.clone(),
            healthy: state == "RUNNING",
        }
    }).collect()
}
```

- [ ] **Step 4: Create a placeholder icon**

Create `crates/kronk-tray/assets/kronk.png` — a simple 32x32 PNG icon. Can be generated or use a placeholder.

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p kronk-tray`
Expected: Compiles

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-tray/
git commit -m "feat: add system tray with profile menus and status polling"
```
