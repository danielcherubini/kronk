# Log Viewer Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `kronk logs` command that tails backend output for a profile, supporting both live foreground streaming and reading from persistent log files for services.

**Architecture:** Two modes: (1) For foreground `kronk run`, stdout/stderr are already streamed. (2) For services, add a log directory (`~/.kronk/logs/`) where `ProcessSupervisor` writes rotated log files. `kronk logs` reads the latest log file and optionally follows it with `--follow`. Log rotation keeps the last N files by size.

**Tech Stack:** Rust, `std::fs`, `std::io::BufRead`, `tokio::fs`, `notify` (file watcher) or polling, existing `ProcessSupervisor`

---

## File Structure

### New files to create

| File | Responsibility |
|------|---------------|
| `crates/kronk-core/src/logging.rs` | Log file path resolution, rotation, writer |

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/lib.rs` | Add `pub mod logging;` |
| `crates/kronk-core/src/config.rs` | Add `logs_dir` to `General`, add `logs_dir()` method |
| `crates/kronk-core/src/process.rs` | Pipe stdout/stderr to log files via `logging` module |
| `crates/kronk-cli/src/main.rs` | Add `Logs` command variant with `--profile`, `--follow`, `--lines` flags |

---

## Chunk 1: Log File Infrastructure

### Task 1: Add log directory config

**Files:**
- Modify: `crates/kronk-core/src/config.rs`

- [ ] **Step 1: Add `logs_dir` to `General` struct**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    pub log_level: String,
    #[serde(default)]
    pub models_dir: Option<String>,
    #[serde(default)]
    pub logs_dir: Option<String>,
}
```

- [ ] **Step 2: Add `logs_dir()` method to `Config`**

```rust
/// Resolve the logs directory path.
/// Uses `general.logs_dir` if set, otherwise defaults to `~/.kronk/logs/`.
pub fn logs_dir(&self) -> Result<PathBuf> {
    if let Some(ref dir) = self.general.logs_dir {
        Ok(PathBuf::from(dir))
    } else {
        let home = directories::UserDirs::new()
            .context("Failed to determine home directory")?;
        Ok(home.home_dir().join(".kronk").join("logs"))
    }
}
```

- [ ] **Step 3: Update `Config::default()` to include `logs_dir: None`**

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-core/src/config.rs
git commit -m "feat: add logs_dir config field"
```

### Task 2: Create logging module

**Files:**
- Create: `crates/kronk-core/src/logging.rs`
- Modify: `crates/kronk-core/src/lib.rs`

- [ ] **Step 1: Write the logging module**

```rust
use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const MAX_LOG_FILES: usize = 5;

/// Get the log file path for a profile.
pub fn log_path(logs_dir: &Path, profile: &str) -> PathBuf {
    logs_dir.join(format!("{}.log", profile))
}

/// Open (or create) a log file for appending. Rotates if over MAX_LOG_SIZE.
pub fn open_log(logs_dir: &Path, profile: &str) -> Result<File> {
    fs::create_dir_all(logs_dir)
        .with_context(|| format!("Failed to create logs directory: {}", logs_dir.display()))?;

    let path = log_path(logs_dir, profile);

    // Rotate if needed
    if path.exists() {
        let meta = fs::metadata(&path)?;
        if meta.len() > MAX_LOG_SIZE {
            rotate_logs(logs_dir, profile)?;
        }
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open log file: {}", path.display()))
}

/// Rotate log files: profile.log -> profile.1.log -> profile.2.log -> ...
fn rotate_logs(logs_dir: &Path, profile: &str) -> Result<()> {
    // Remove oldest
    let oldest = logs_dir.join(format!("{}.{}.log", profile, MAX_LOG_FILES));
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }

    // Shift existing numbered logs
    for i in (1..MAX_LOG_FILES).rev() {
        let from = logs_dir.join(format!("{}.{}.log", profile, i));
        let to = logs_dir.join(format!("{}.{}.log", profile, i + 1));
        if from.exists() {
            fs::rename(&from, &to)?;
        }
    }

    // Move current to .1
    let current = log_path(logs_dir, profile);
    let first = logs_dir.join(format!("{}.1.log", profile));
    if current.exists() {
        fs::rename(&current, &first)?;
    }

    Ok(())
}

/// Read the last N lines from a log file.
pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    use std::io::{BufRead, BufReader};

    if !path.exists() {
        return Ok(vec![]);
    }

    let file = File::open(path)
        .with_context(|| format!("Failed to open log file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();

    if all_lines.len() <= n {
        Ok(all_lines)
    } else {
        Ok(all_lines[all_lines.len() - n..].to_vec())
    }
}
```

- [ ] **Step 2: Add `pub mod logging;` to `lib.rs`**

- [ ] **Step 3: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_path() {
        let path = log_path(Path::new("/tmp/logs"), "default");
        assert_eq!(path, PathBuf::from("/tmp/logs/default.log"));
    }

    #[test]
    fn test_open_and_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = open_log(tmp.path(), "test").unwrap();
        writeln!(f, "line 1").unwrap();
        writeln!(f, "line 2").unwrap();
        writeln!(f, "line 3").unwrap();
        drop(f);

        let lines = tail_lines(&log_path(tmp.path(), "test"), 2).unwrap();
        assert_eq!(lines, vec!["line 2", "line 3"]);
    }

    #[test]
    fn test_tail_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = tail_lines(&log_path(tmp.path(), "nonexistent"), 10).unwrap();
        assert!(lines.is_empty());
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p kronk-core -- logging`
Expected: All 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/kronk-core/src/logging.rs crates/kronk-core/src/lib.rs
git commit -m "feat: add logging module with log rotation and tail"
```

### Task 3: Pipe ProcessSupervisor output to log files

**Files:**
- Modify: `crates/kronk-core/src/process.rs`

- [ ] **Step 1: Add optional log file to ProcessSupervisor**

Add a `log_dir: Option<PathBuf>` field and a `with_log_dir` builder method. In the `run` method, when `log_dir` is set, open a log file via `logging::open_log` and write each stdout/stderr line to both the event channel and the log file.

- [ ] **Step 2: Update callers in main.rs**

Pass `config.logs_dir().ok()` when constructing `ProcessSupervisor` for service runs.

- [ ] **Step 3: Verify it compiles and tests pass**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/process.rs crates/kronk-cli/src/main.rs
git commit -m "feat: pipe supervisor output to log files"
```

---

## Chunk 2: `kronk logs` CLI Command

### Task 4: Add `Logs` command to CLI

**Files:**
- Modify: `crates/kronk-cli/src/main.rs`

- [ ] **Step 1: Add `Logs` variant to `Commands`**

```rust
/// View backend logs for a profile
Logs {
    /// Profile name (default: "default")
    #[arg(short, long, default_value = "default")]
    profile: String,
    /// Follow log output (like tail -f)
    #[arg(short, long)]
    follow: bool,
    /// Number of lines to show (default: 50)
    #[arg(short = 'n', long, default_value = "50")]
    lines: usize,
},
```

- [ ] **Step 2: Implement `cmd_logs`**

```rust
async fn cmd_logs(config: &Config, profile: &str, follow: bool, lines: usize) -> Result<()> {
    let logs_dir = config.logs_dir()?;
    let log_path = kronk_core::logging::log_path(&logs_dir, profile);

    if !log_path.exists() {
        println!("No logs found for profile '{}'.", profile);
        println!();
        println!("Logs are created when running as a service.");
        println!("For foreground: kronk run --profile {}", profile);
        return Ok(());
    }

    // Print last N lines
    let tail = kronk_core::logging::tail_lines(&log_path, lines)?;
    for line in &tail {
        println!("{}", line);
    }

    if follow {
        // Poll for new content
        use tokio::time::{interval, Duration};
        use std::io::{BufRead, BufReader, Seek, SeekFrom};

        let mut file = std::fs::File::open(&log_path)?;
        file.seek(SeekFrom::End(0))?;
        let mut reader = BufReader::new(file);
        let mut tick = interval(Duration::from_millis(250));

        loop {
            tick.tick().await;
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                print!("{}", line);
                line.clear();
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 3: Wire the match arm**

```rust
Commands::Logs { profile, follow, lines } => cmd_logs(&config, &profile, follow, lines).await,
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check --workspace`
Expected: Compiles

- [ ] **Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/kronk-cli/src/main.rs
git commit -m "feat: add 'kronk logs' command with --follow and --lines"
```
