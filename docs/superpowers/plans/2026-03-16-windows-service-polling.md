# Windows Service Lifecycle Polling Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fixed `thread::sleep` calls in the Windows service management code with proper status polling loops, ensuring reliable service stop/delete/create transitions without race conditions.

**Architecture:** Replace the three fixed-sleep patterns in `windows.rs` (`install_service` and `remove_service`) with a reusable `wait_for_state` helper that polls `service.query_status()` with exponential backoff until the desired state is reached or a timeout expires. For the delete-then-recreate flow, retry `create_service` in a loop to handle `ERROR_SERVICE_MARKED_FOR_DELETE`. Drop service handles promptly after delete so the SCM can finalize removal.

**Tech Stack:** Rust, `windows-service` crate 0.7, `std::thread::sleep`, `std::time::{Duration, Instant}`

---

## File Structure

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/platform/windows.rs` | Add `wait_for_state` helper, refactor `install_service` and `remove_service` to use polling |

---

## Chunk 1: Polling Helper and Service Lifecycle Refactor

### Task 1: Add `wait_for_state` polling helper

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs`

- [ ] **Step 1: Write the `wait_for_state` helper function**

Add this private helper at the top of the impl section (after the imports, before `install_service`):

```rust
use std::time::{Duration, Instant};

/// Poll a service until it reaches the desired state, or timeout.
/// Uses exponential backoff starting at 100ms, capped at 2s per poll.
fn wait_for_state(
    service: &windows_service::service::Service,
    desired: ServiceState,
    timeout: Duration,
) -> Result<()> {
    let start = Instant::now();
    let mut interval = Duration::from_millis(100);
    let max_interval = Duration::from_secs(2);

    loop {
        let status = service.query_status()
            .context("Failed to query service status while waiting")?;
        if status.current_state == desired {
            return Ok(());
        }
        if start.elapsed() > timeout {
            anyhow::bail!(
                "Timed out waiting for service to reach {:?} (current: {:?})",
                desired,
                status.current_state,
            );
        }
        std::thread::sleep(interval);
        interval = (interval * 2).min(max_interval);
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p kronk-core --target x86_64-pc-windows-msvc` (or `cargo check -p kronk-core` on a Windows machine)
Expected: Compiles with no errors (warnings OK)

### Task 2: Refactor `install_service` to use polling

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs:17-28`

- [ ] **Step 1: Replace fixed sleeps with polling in `install_service`**

Replace the existing service-removal block:

```rust
// Remove existing service if present
if let Ok(existing) = manager.open_service(service_name, ServiceAccess::ALL_ACCESS) {
    let status = existing.query_status()?;
    if status.current_state != ServiceState::Stopped {
        existing.stop()?;
        // Wait briefly for stop
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    existing.delete()?;
    // Brief wait for SCM to process deletion
    std::thread::sleep(std::time::Duration::from_millis(500));
}
```

With:

```rust
// Remove existing service if present
if let Ok(existing) = manager.open_service(service_name, ServiceAccess::ALL_ACCESS) {
    let status = existing.query_status()?;
    if status.current_state != ServiceState::Stopped {
        existing.stop()?;
        wait_for_state(&existing, ServiceState::Stopped, Duration::from_secs(30))
            .with_context(|| format!("Service '{}' did not stop in time", service_name))?;
    }
    existing.delete()?;
    // Drop the handle so SCM can finalize deletion
    drop(existing);

    // Wait for SCM to fully process the deletion by retrying open
    let delete_start = Instant::now();
    let delete_timeout = Duration::from_secs(10);
    loop {
        match manager.open_service(service_name, ServiceAccess::QUERY_STATUS) {
            Ok(_) => {
                // Service still exists — SCM hasn't finalized yet
                if delete_start.elapsed() > delete_timeout {
                    anyhow::bail!(
                        "Timed out waiting for SCM to delete service '{}'",
                        service_name
                    );
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(_) => break, // Service gone — proceed
        }
    }
}
```

- [ ] **Step 2: Add the `Duration` and `Instant` imports if not already present**

Make sure the top of the file has:

```rust
use std::time::{Duration, Instant};
```

Remove any existing `use std::time::Duration` or bare `std::time::Duration::from_*` calls and use the imported names instead.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/platform/windows.rs
git commit -m "fix: replace fixed sleeps with polling in install_service"
```

### Task 3: Refactor `remove_service` to use polling

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs` (the `remove_service` function)

- [ ] **Step 1: Replace fixed sleep in `remove_service`**

Replace:

```rust
pub fn remove_service(service_name: &str) -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open Service Control Manager — run as Administrator")?;

    let service = manager
        .open_service(service_name, ServiceAccess::STOP | ServiceAccess::DELETE)
        .with_context(|| format!("Service '{}' not found", service_name))?;

    // Try to stop first
    let _ = service.stop();
    std::thread::sleep(std::time::Duration::from_secs(1));

    service.delete().context("Failed to delete service")?;

    // Remove firewall rule
    remove_firewall_rule(service_name).ok();

    Ok(())
}
```

With:

```rust
pub fn remove_service(service_name: &str) -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("Failed to open Service Control Manager — run as Administrator")?;

    let service = manager
        .open_service(
            service_name,
            ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
        )
        .with_context(|| format!("Service '{}' not found", service_name))?;

    // Stop if running, then wait for it to actually stop
    let status = service.query_status()?;
    if status.current_state != ServiceState::Stopped {
        let _ = service.stop();
        wait_for_state(&service, ServiceState::Stopped, Duration::from_secs(30))
            .with_context(|| format!("Service '{}' did not stop in time", service_name))?;
    }

    service.delete().context("Failed to delete service")?;

    // Remove firewall rule
    remove_firewall_rule(service_name).ok();

    Ok(())
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles with no errors

- [ ] **Step 3: Run full workspace check**

Run: `cargo check --workspace`
Expected: Compiles with no errors

- [ ] **Step 4: Commit**

```bash
git add crates/kronk-core/src/platform/windows.rs
git commit -m "fix: replace fixed sleep with polling in remove_service"
```
