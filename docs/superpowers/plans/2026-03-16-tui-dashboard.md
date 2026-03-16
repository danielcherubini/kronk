# TUI Dashboard Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a War Room dashboard showing live VRAM, tokens/sec, temperature, and logs using ratatui.

**Architecture:** New `kronk-tui` crate that connects to `kronk-core` config and process supervisor. Uses ratatui for rendering, tokio for async event loop.

**Tech Stack:** Rust, tokio, ratatui, crossterm, anyhow

---

## File Structure

```text
kronk/
├── crates/
│   └── kronk-tui/              # NEW: TUI dashboard crate
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── lib.rs
│           ├── components/
│           │   ├── mod.rs
│           │   ├── dashboard.rs
│           │   ├── stats.rs
│           │   └── logs.rs
│       └── tests/
│           └── integration.rs
```

---

## Task 1: Create TUI Crate Skeleton

**Files:**
- Create: `crates/kronk-tui/Cargo.toml`

- [ ] **Step 1: Write Cargo.toml**

```toml
[package]
name = "kronk-tui"
version = "0.1.0"
edition = "2021"

[dependencies]
kronk-core = { path = "../kronk-core" }
ratatui = "0.29"
crossterm = "0.28"
tokio = { version = "1", features = ["full"] }
anyhow = "1.0"
```

- [ ] **Step 2: Commit**

```bash
git add crates/kronk-tui/Cargo.toml
git commit -m "feat: add TUI dashboard crate skeleton"
```

---

## Task 2: Write Failing Test

**Files:**
- Create: `crates/kronk-tui/tests/integration.rs`

- [ ] **Step 3: Write failing test**

```rust
// crates/kronk-tui/tests/integration.rs
use kronk_tui::run;

#[tokio::test]
async fn test_tui_starts() {
    // Should fail with "cannot find crate" or "no such module"
    run().await.unwrap();
}
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cd crates/kronk-tui && cargo test`
Expected: FAIL with compilation error

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-tui/tests/integration.rs
git commit -m "test: add TUI integration test"
```

---

## Task 3: Write Minimal Implementation

**Files:**
- Create: `crates/kronk-tui/src/lib.rs`
- Create: `crates/kronk-tui/src/main.rs`
- Create: `crates/kronk-tui/src/components/mod.rs`
- Create: `crates/kronk-tui/src/components/dashboard.rs`

- [ ] **Step 6: Write minimal implementation**

```rust
// crates/kronk-tui/src/lib.rs
use anyhow::Result;

pub async fn run() -> Result<()> {
    // TODO: Implement TUI
    Ok(())
}
```

```rust
// crates/kronk-tui/src/main.rs
use kronk_tui::run;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}
```

```rust
// crates/kronk-tui/src/components/mod.rs
pub mod dashboard;
pub mod stats;
pub mod logs;
```

```rust
// crates/kronk-tui/src/components/dashboard.rs
use ratatui::prelude::*;

pub struct DashboardWidget;

impl DashboardWidget {
    pub fn render(f: &mut Frame, area: Rect) {
        let chunk = f.render_widget(&DashboardWidget {}, area);
    }
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cd crates/kronk-tui && cargo test`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/kronk-tui/src/
git commit -m "feat: add TUI components skeleton"
```

---

## Task 4: Implement Full TUI

**Files:**
- Modify: `crates/kronk-tui/src/lib.rs`
- Modify: `crates/kronk-tui/src/main.rs`
- Modify: `crates/kronk-tui/src/components/dashboard.rs`
- Create: `crates/kronk-tui/src/components/stats.rs`
- Create: `crates/kronk-tui/src/components/logs.rs`

- [ ] **Step 9: Write full implementation**

```rust
// crates/kronk-tui/src/lib.rs
use anyhow::Result;
use ratatui::prelude::*;
use tokio::time::{sleep, Duration};
use crate::backend::Backend;

pub async fn run() -> Result<()> {
    let backend = Backend::new()?;
    let mut terminal = Terminal::new(crossterm::terminal::stdout())?;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    
    loop {
        terminal.draw(|f| ui(f, &backend))?;
        interval.tick().await;
    }
}

fn ui(f: &mut Frame, backend: &Backend) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(10),
            Constraint::Min(0),
        ])
        .split(f.size());
    
    f.render_widget(StatsWidget {}, chunks[0]);
    f.render_widget(LogsWidget {}, chunks[1]);
    f.render_widget(Clear {}, chunks[2]);
}
```

```rust
// crates/kronk-tui/src/components/stats.rs
use ratatui::prelude::*;

pub struct StatsWidget;

impl StatsWidget {
    pub fn render(f: &mut Frame, area: Rect) {
        let bg = Background(Color::Rgb(40, 44, 52));
        let chunk = f.render_widget(&StatsWidget {}, area, bg);
    }
}
```

```rust
// crates/kronk-tui/src/components/logs.rs
use ratatui::prelude::*;

pub struct LogsWidget;

impl LogsWidget {
    pub fn render(f: &mut Frame, area: Rect) {
        let bg = Background(Color::Rgb(40, 44, 52));
        let chunk = f.render_widget(&LogsWidget {}, area, bg);
    }
}
```

- [ ] **Step 10: Run test to verify it passes**

Run: `cd crates/kronk-tui && cargo run`
Expected: TUI window opens

- [ ] **Step 11: Commit**

```bash
git add crates/kronk-tui/src/
git commit -m "feat: implement full TUI dashboard with stats and logs"
```

---

## Task 5: Connect to Kronk Core

**Files:**
- Modify: `crates/kronk-tui/src/lib.rs`
- Create: `crates/kronk-tui/src/backend.rs`

- [ ] **Step 12: Write backend connection**

```rust
// crates/kronk-tui/src/backend.rs
use anyhow::Result;
use kronk_core::config::Config;

pub struct Backend {
    config: Config,
    active_profile: Option<String>,
}

impl Backend {
    pub fn new() -> Result<Self> {
        let config = Config::load()?;
        Ok(Self {
            config,
            active_profile: None,
        })
    }
    
    pub fn get_stats(&self) -> Option<Stats> {
        // TODO: Fetch stats from active profile
        None
    }
    
    pub fn get_logs(&self) -> Vec<String> {
        // TODO: Fetch logs from active profile
        vec![]
    }
}

pub struct Stats {
    pub vram: u64,
    pub tokens_per_sec: f64,
    pub temperature: f64,
}
```

- [ ] **Step 13: Update lib.rs to use backend**

```rust
// crates/kronk-tui/src/lib.rs
use anyhow::Result;
use ratatui::prelude::*;
use tokio::time::{sleep, Duration};
use crate::backend::Backend;

pub async fn run() -> Result<()> {
    let backend = Backend::new()?;
    let mut terminal = Terminal::new(crossterm::terminal::stdout())?;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    
    loop {
        terminal.draw(|f| ui(f, &backend))?;
        interval.tick().await;
    }
}

fn ui(f: &mut Frame, backend: &Backend) {
    // ... render with backend data
}
```

- [ ] **Step 14: Commit**

```bash
git add crates/kronk-tui/src/
git commit -m "feat: connect TUI to kronk-core config"
```

---

## Execution Order

1. Task 1: Create TUI Crate Skeleton
2. Task 2: Write Failing Test
3. Task 3: Write Minimal Implementation
4. Task 4: Implement Full TUI
5. Task 5: Connect to Kronk Core

**Plan complete and saved to `docs/superpowers/plans/2026-03-16-tui-dashboard.md`. Ready to execute?**