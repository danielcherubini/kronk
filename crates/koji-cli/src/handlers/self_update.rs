//! Self-update command handler
//!
//! Checks for and installs updates to the koji binary from GitHub releases.

use anyhow::Result;

/// Handle the `koji self-update` command.
///
/// If `check` is true, only print version info without installing.
/// If `force` is true, download the latest release even if already up to
/// date.
pub async fn cmd_self_update(check: bool, force: bool) -> Result<()> {
    let current_version = env!("CARGO_PKG_VERSION");

    println!("Checking for updates...");

    let info = koji_core::self_update::check_for_update(current_version).await?;

    println!("Current version: v{}", info.current_version);
    println!("Latest version:  v{}", info.latest_version);

    if !info.update_available && !force {
        println!("\nAlready up to date!");
        return Ok(());
    }

    if check {
        if info.update_available {
            println!("\nUpdate available! Run `koji self-update` to install.");
        }
        return Ok(());
    }

    if info.update_available {
        println!("\nUpdating to v{}...", info.latest_version);
    } else {
        println!("\nForce-downloading v{}...", info.latest_version);
    }

    let result =
        koji_core::self_update::perform_update(current_version, |msg| println!("  {}", msg))
            .await?;

    println!(
        "\nSuccessfully updated from v{} to v{}!",
        result.old_version, result.new_version
    );
    println!("Please restart koji to use the new version.");

    Ok(())
}
