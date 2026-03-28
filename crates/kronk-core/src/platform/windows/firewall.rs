use anyhow::{Context, Result};

/// Add a Windows Firewall rule to allow inbound TCP on the given port.
pub fn add_firewall_rule(name: &str, port: u16) -> Result<()> {
    let rule_name = format!("Kronk: {}", name);

    // Remove existing rule if present
    std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={}", rule_name),
        ])
        .output()
        .ok();

    let status = std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={}", rule_name),
            "dir=in",
            "action=allow",
            "protocol=TCP",
            &format!("localport={}", port),
        ])
        .output()
        .context("Failed to run netsh")?;

    if !status.status.success() {
        anyhow::bail!("Failed to add firewall rule");
    }

    Ok(())
}

/// Remove a firewall rule by service name.
pub fn remove_firewall_rule(name: &str) -> Result<()> {
    let rule_name = format!("Kronk: {}", name);
    std::process::Command::new("netsh")
        .args([
            "advfirewall",
            "firewall",
            "delete",
            "rule",
            &format!("name={}", rule_name),
        ])
        .output()
        .context("Failed to run netsh")?;
    Ok(())
}
