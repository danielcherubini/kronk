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
