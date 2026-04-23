//! Discovery helpers for the llama-cli binary.
//!
//! Kept separate from the orchestrator because they're pure filesystem /
//! path-string logic with no runtime dependencies, which makes them easy to
//! test in isolation.

use anyhow::{bail, Result};
use std::path::PathBuf;

/// Locate the llama-cli binary.
///
/// Search order:
/// 1. `LLAMA_CLI_PATH` environment variable
/// 2. `<backend_path>/bin/llama-cli` (or `llama-cli.exe` on Windows)
/// 3. `<backend_path>/build/bin/llama-cli`
/// 4. `<backend_path>/bin/release/llama-cli`
/// 5. Grandparent-relative: `<backend_path>/../tools/llama-cli/llama-cli`
/// 6. `PATH` lookup for `llama-cli`
///
/// Returns an error with a clear message listing searched paths if not found.
pub fn find_llama_cli(backend_path: &std::path::Path) -> Result<PathBuf> {
    let mut searched = Vec::new();

    // 1. Environment variable
    if let Ok(p) = std::env::var("LLAMA_CLI_PATH") {
        let p = PathBuf::from(&p);
        searched.push(p.clone());
        if p.exists() {
            return Ok(p);
        }
    }

    let cli_name = if cfg!(target_os = "windows") {
        "llama-cli.exe"
    } else {
        "llama-cli"
    };

    // 2. <backend_path>/bin/llama-cli
    let bin_path = backend_path.join("bin").join(cli_name);
    searched.push(bin_path.clone());
    if bin_path.exists() {
        return Ok(bin_path);
    }

    // 3. <backend_path>/build/bin/llama-cli
    let build_bin_path = backend_path.join("build").join("bin").join(cli_name);
    searched.push(build_bin_path.clone());
    if build_bin_path.exists() {
        return Ok(build_bin_path);
    }

    // 4. <backend_path>/bin/release/llama-cli
    let release_path = backend_path.join("bin").join("release").join(cli_name);
    searched.push(release_path.clone());
    if release_path.exists() {
        return Ok(release_path);
    }

    // 5. Grandparent-relative: <backend_path>/../tools/llama-cli/llama-cli
    let grandparent = backend_path.parent().and_then(|p| p.parent());
    if let Some(gp) = grandparent {
        let tools_path = gp.join("tools").join("llama-cli").join(cli_name);
        searched.push(tools_path.clone());
        if tools_path.exists() {
            return Ok(tools_path);
        }
    }

    // 6. PATH lookup
    for path_dir in std::env::split_paths(&std::env::var("PATH").unwrap_or_default()) {
        let candidate = path_dir.join(cli_name);
        searched.push(candidate.clone());
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let paths_list = searched
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");

    bail!(
        "llama-cli binary not found. Searched:\n{}\nInstall llama.cpp from source or set LLAMA_CLI_PATH env var.",
        paths_list
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Verifies that `find_llama_cli` returns an error when no binary exists anywhere.
    #[test]
    fn test_find_llama_cli_not_found() {
        let nonexistent = PathBuf::from("/nonexistent/path/backend");
        let result = find_llama_cli(&nonexistent);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("llama-cli binary not found"));
    }

    /// Verifies that `find_llama_cli` finds the binary in `<backend_path>/bin/llama-cli`.
    #[test]
    fn test_find_llama_cli_bin_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();
        let bin_dir = backend_path.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let cli_path = bin_dir.join(if cfg!(target_os = "windows") {
            "llama-cli.exe"
        } else {
            "llama-cli"
        });
        fs::write(&cli_path, "#!/bin/sh\necho mock").unwrap();

        let result = find_llama_cli(backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), cli_path);
    }

    /// Verifies that `find_llama_cli` finds the binary in `<backend_path>/build/bin/llama-cli`.
    #[test]
    fn test_find_llama_cli_build_bin_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();
        let build_bin_dir = backend_path.join("build").join("bin");
        fs::create_dir_all(&build_bin_dir).unwrap();
        let cli_path = build_bin_dir.join(if cfg!(target_os = "windows") {
            "llama-cli.exe"
        } else {
            "llama-cli"
        });
        fs::write(&cli_path, "#!/bin/sh\necho mock").unwrap();

        let result = find_llama_cli(backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), cli_path);
    }

    /// Verifies that `find_llama_cli` finds the binary in `<backend_path>/bin/release/llama-cli`.
    #[test]
    fn test_find_llama_cli_release_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();
        let release_dir = backend_path.join("bin").join("release");
        fs::create_dir_all(&release_dir).unwrap();
        let cli_path = release_dir.join(if cfg!(target_os = "windows") {
            "llama-cli.exe"
        } else {
            "llama-cli"
        });
        fs::write(&cli_path, "#!/bin/sh\necho mock").unwrap();

        let result = find_llama_cli(backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), cli_path);
    }

    /// Verifies that `find_llama_cli` finds the binary in grandparent tools directory.
    #[test]
    fn test_find_llama_cli_tools_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        // Create structure: <tmp>/llama.cpp/tools/llama-cli/llama-cli
        // and backend at: <tmp>/llama.cpp/build/bin/something
        let tools_dir = base.join("llama.cpp").join("tools").join("llama-cli");
        fs::create_dir_all(&tools_dir).unwrap();
        let cli_path = tools_dir.join(if cfg!(target_os = "windows") {
            "llama-cli.exe"
        } else {
            "llama-cli"
        });
        fs::write(&cli_path, "#!/bin/sh\necho mock").unwrap();

        // Backend path is somewhere in llama.cpp/build/bin/
        let backend_path = base.join("llama.cpp").join("build").join("bin");
        fs::create_dir_all(&backend_path).unwrap();

        let result = find_llama_cli(&backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), cli_path);
    }

    /// Verifies that `find_llama_cli` prefers env var over all other paths.
    #[test]
    fn test_find_llama_cli_env_var_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();

        // Create a binary in bin/ dir
        let bin_dir = backend_path.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let _bin_cli = bin_dir.join(if cfg!(target_os = "windows") {
            "llama-cli.exe"
        } else {
            "llama-cli"
        });
        fs::write(&_bin_cli, "#!/bin/sh\necho from-bin").unwrap();

        // Create a separate env var binary
        let env_dir = tmp.path().join("env_override");
        fs::create_dir_all(&env_dir).unwrap();
        let env_cli = env_dir.join(if cfg!(target_os = "windows") {
            "llama-cli.exe"
        } else {
            "llama-cli"
        });
        fs::write(&env_cli, "#!/bin/sh\necho from-env").unwrap();

        // Save and restore previous env var state to avoid test interference
        let prev = std::env::var_os("LLAMA_CLI_PATH");
        std::env::set_var("LLAMA_CLI_PATH", env_cli.to_str().unwrap());
        let result = find_llama_cli(backend_path);
        match prev {
            Some(val) => std::env::set_var("LLAMA_CLI_PATH", val),
            None => std::env::remove_var("LLAMA_CLI_PATH"),
        }

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), env_cli);
    }

    /// Verifies that error message lists searched paths.
    #[test]
    fn test_find_llama_cli_error_lists_paths() {
        let nonexistent = PathBuf::from("/nonexistent/path/backend");
        let result = find_llama_cli(&nonexistent);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Searched:"));
    }
}
