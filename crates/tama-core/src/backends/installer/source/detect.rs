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

/// Register a library path with the system dynamic linker (ldconfig).
///
/// Appends the path to `/etc/ld.so.conf.d/<conf_name>` if not already present,
/// then runs `ldconfig`. Requires root privileges. Returns Ok if successful,
/// Err if ldconfig fails or the file can't be written.
#[cfg(not(target_os = "windows"))]
pub(super) fn register_ldconfig_path(path: &str, conf_name: &str) -> std::io::Result<()> {
    use std::io::Write;

    let conf_path = format!("/etc/ld.so.conf.d/{}", conf_name);

    // Read existing paths, deduplicate
    let mut paths: Vec<String> = Vec::new();
    if let Ok(contents) = std::fs::read_to_string(&conf_path) {
        paths = contents
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
    }

    // Skip if already registered
    if paths.iter().any(|p| p == path) {
        return Ok(());
    }

    paths.push(path.to_string());

    // Write the config file
    let mut file = std::fs::File::create(&conf_path)?;
    for p in &paths {
        writeln!(file, "{}", p)?;
    }

    // Run ldconfig
    let status = std::process::Command::new("ldconfig").status()?;
    if !status.success() {
        return Err(std::io::Error::other("ldconfig failed"));
    }
    Ok(())
}
