pub(super) fn hip_env_from_hipconfig_output(
    clang_dir_stdout: &str,
    hip_root_stdout: &str,
) -> Option<(String, String)> {
    let clang_dir = clang_dir_stdout.trim();
    let hip_root = hip_root_stdout.trim();
    if clang_dir.is_empty() || hip_root.is_empty() {
        return None;
    }
    Some((format!("{}/clang", clang_dir), hip_root.to_string()))
}

pub(super) fn detect_hip_env() -> Option<(String, String)> {
    // Runs `hipconfig -l` and `hipconfig -R`. Returns None if hipconfig is
    // unavailable, either call fails, or either stdout is empty.
    let clang_dir = std::process::Command::new("hipconfig")
        .arg("-l")
        .output()
        .ok()?;
    if !clang_dir.status.success() {
        return None;
    }
    let hip_root = std::process::Command::new("hipconfig")
        .arg("-R")
        .output()
        .ok()?;
    if !hip_root.status.success() {
        return None;
    }
    hip_env_from_hipconfig_output(
        &String::from_utf8_lossy(&clang_dir.stdout),
        &String::from_utf8_lossy(&hip_root.stdout),
    )
}

/// Detect the ROCm library directory via `hipconfig -R`.
///
/// Returns the path to the `lib/` subdirectory (e.g. `/opt/rocm/core-7.13/lib`).
pub(super) fn detect_rocm_lib_dir() -> Option<String> {
    let hip_root = std::process::Command::new("hipconfig")
        .arg("-R")
        .output()
        .ok()?;
    if !hip_root.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&hip_root.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    Some(format!("{}/lib", root))
}

/// Register the ROCm library path with the system dynamic linker (ldconfig).
///
/// Writes the lib path to `/etc/ld.so.conf.d/rocm.conf` and runs `ldconfig`.
/// Requires root privileges. Returns Ok if successful, Err if ldconfig fails
/// or the path is already registered.
#[cfg(not(target_os = "windows"))]
pub(super) fn register_rocm_ldconfig(lib_dir: &str) -> std::io::Result<()> {
    use std::io::Write;

    let conf_path = "/etc/ld.so.conf.d/rocm.conf";

    // Check if already registered
    if let Ok(contents) = std::fs::read_to_string(conf_path) {
        if contents.trim() == lib_dir {
            return Ok(());
        }
    }

    // Write the config file
    let mut file = std::fs::File::create(conf_path)?;
    writeln!(file, "{}", lib_dir)?;

    // Run ldconfig
    let status = std::process::Command::new("ldconfig")
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other("ldconfig failed"));
    }
    Ok(())
}
