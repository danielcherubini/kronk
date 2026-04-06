use anyhow::{Context, Result};

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

/// Grant the installing user permission to start, stop, and query the service.
/// Resolves the current user's SID and applies it via `sc sdset`, so only the
/// installer (plus SYSTEM and Administrators) can control the service.
pub(super) fn grant_user_control(service_name: &str) -> Result<()> {
    let user_sid = get_current_user_sid().with_context(|| {
        format!(
            "Failed to resolve current user SID for service '{}'",
            service_name
        )
    })?;

    tracing::info!("Granting service control to user SID: {}", user_sid);

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
