/// Find the LLVM bin directory containing clang-cl.
/// Searches well-known install locations on Windows.
#[cfg(target_os = "windows")]
pub(super) fn find_llvm_bin() -> Option<std::path::PathBuf> {
    let candidates = [
        r"C:\Program Files\LLVM\bin",
        r"C:\Program Files (x86)\LLVM\bin",
    ];
    for candidate in &candidates {
        let p = std::path::Path::new(candidate);
        if p.join("clang-cl.exe").exists() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Find the vcvarsall.bat script for MSVC environment setup.
/// Searches known Visual Studio Build Tools installation paths.
#[cfg(target_os = "windows")]
pub(super) fn find_vcvarsall() -> Option<std::path::PathBuf> {
    // VS year-named installs (2022, 2019, ...) and numeric (18, 17, ...)
    let vs_base = std::path::Path::new(r"C:\Program Files (x86)\Microsoft Visual Studio");
    let editions = ["BuildTools", "Enterprise", "Professional", "Community"];
    let subdirs = ["18", "2026", "17", "2022", "16", "2019"];

    for subdir in &subdirs {
        for edition in &editions {
            let candidate = vs_base
                .join(subdir)
                .join(edition)
                .join(r"VC\Auxiliary\Build\vcvarsall.bat");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
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

#[cfg(not(target_os = "windows"))]
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
