# Windows Service SID-Based Access Control Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broad Interactive Users (IU) SDDL ACE in the Windows service permission grant with the specific SID of the installing user, so only the installer (and Administrators/SYSTEM) can start/stop the service.

**Architecture:** Add a `get_current_user_sid` function that shells out to `whoami /user /fo csv /nh` to resolve the current user's SID string, then inject that SID into the SDDL ACE that `grant_user_control` applies via `sc sdset`. This avoids pulling in the `windows` crate for Win32 API calls and keeps the implementation simple and testable.

**Tech Stack:** Rust, `std::process::Command` (`whoami`), `sc sdset`, existing `anyhow` error handling

---

## File Structure

### Files to modify

| File | Changes |
|------|---------|
| `crates/kronk-core/src/platform/windows.rs` | Add `get_current_user_sid()`, update `grant_user_control()` to use SID instead of IU |

---

## Chunk 1: SID Resolution and SDDL Update

### Task 1: Add `get_current_user_sid` helper

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs`

- [ ] **Step 1: Write the SID resolution function**

Add this private helper function to `windows.rs`:

```rust
/// Resolve the SID of the current user via `whoami /user`.
/// Returns the SID string, e.g. "S-1-5-21-1234567890-1234567890-1234567890-1001".
fn get_current_user_sid() -> Result<String> {
    let output = std::process::Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()
        .context("Failed to run 'whoami /user' — is this a Windows system?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("whoami failed (exit {}): {}", output.status, stderr.trim());
    }

    // Output format: "DOMAIN\User","S-1-5-21-..."
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();

    // Parse CSV: split on "," and take the second field, strip quotes
    let sid = line
        .split(',')
        .nth(1)
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| s.starts_with("S-1-"))
        .with_context(|| format!("Failed to parse SID from whoami output: {}", line))?;

    Ok(sid)
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles with no errors (function is unused at this point — warning is OK)

- [ ] **Step 3: Commit**

```bash
git add crates/kronk-core/src/platform/windows.rs
git commit -m "feat: add get_current_user_sid helper for Windows"
```

### Task 2: Update `grant_user_control` to use the installer's SID

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs` (the `grant_user_control` function)

- [ ] **Step 1: Replace IU with the resolved SID in the SDDL**

Replace the existing `grant_user_control` function:

```rust
/// Grant Interactive Users (IU) permission to start, stop, and query the service.
/// This allows non-admin users to control the service after initial install.
fn grant_user_control(service_name: &str) -> Result<()> {
    // SDDL breakdown:
    //   SY = Local System: full control
    //   BA = Builtin Administrators: full control
    //   IU = Interactive Users: start (RP), stop (WP), query status (LC), query config (LO), read (CR)
    let sddl = format!(
        "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;RPWPLCLOCR;;;IU)"
    );

    let output = std::process::Command::new("sc")
        .args(["sdset", service_name, &sddl])
        .output()
        .context("Failed to run sc sdset")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "sc sdset {} failed (exit {}): {}",
            service_name,
            output.status,
            stderr.trim()
        );
    }

    Ok(())
}
```

With:

```rust
/// Grant the installing user permission to start, stop, and query the service.
/// Resolves the current user's SID and applies it via `sc sdset`, so only the
/// installer (plus SYSTEM and Administrators) can control the service.
fn grant_user_control(service_name: &str) -> Result<()> {
    let user_sid = get_current_user_sid()
        .context("Could not resolve current user SID for service permissions")?;

    // SDDL breakdown:
    //   SY  = Local System: full control
    //   BA  = Builtin Administrators: full control
    //   <SID> = Installing user: start (RP), stop (WP), query status (LC), query config (LO), read (CR)
    let sddl = format!(
        "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;RPWPLCLOCR;;;{})",
        user_sid
    );

    tracing::debug!("Setting service SDDL for '{}': {}", service_name, sddl);

    let output = std::process::Command::new("sc")
        .args(["sdset", service_name, &sddl])
        .output()
        .context("Failed to run sc sdset")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "sc sdset {} failed (exit {}): {}",
            service_name,
            output.status,
            stderr.trim()
        );
    }

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
git commit -m "security: use installer's SID instead of IU for service ACL"
```

### Task 3: Add fallback for SID resolution failure

**Files:**
- Modify: `crates/kronk-core/src/platform/windows.rs` (the `grant_user_control` function)

- [ ] **Step 1: Add graceful fallback to IU if SID resolution fails**

This ensures the service install doesn't hard-fail if `whoami` is unavailable (e.g. minimal Windows containers). Update the `grant_user_control` function's SID resolution to:

```rust
    let user_sid = match get_current_user_sid() {
        Ok(sid) => {
            tracing::info!("Granting service control to user SID: {}", sid);
            sid
        }
        Err(e) => {
            tracing::warn!(
                "Could not resolve current user SID ({}), falling back to Interactive Users (IU)",
                e
            );
            "IU".to_string()
        }
    };
```

Replace the existing `get_current_user_sid().context(...)` call with this match block.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p kronk-core`
Expected: Compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add crates/kronk-core/src/platform/windows.rs
git commit -m "fix: graceful fallback to IU if SID resolution fails"
```
