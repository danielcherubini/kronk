//! Discovery helpers for the llama-server binary.
//!
//! Kept separate from the orchestrator because they're pure filesystem /
//! path-string logic with no runtime dependencies, which makes them easy to
//! test in isolation.

use anyhow::{bail, Result};
use std::path::PathBuf;

/// Locate the llama-server binary.
///
/// Search order:
/// 1. `LLAMA_SERVER_PATH` environment variable
/// 2. `<backend_path>/llama-server` (binary directly in backend dir — our layout)
/// 3. `<backend_path>/bin/llama-server`
/// 4. `<backend_path>/build/bin/llama-server`
/// 5. `PATH` lookup for `llama-server`
///
/// Returns an error with a clear message listing searched paths if not found.
pub fn find_llama_server(backend_path: &std::path::Path) -> Result<PathBuf> {
    let mut searched = Vec::new();

    // 1. Environment variable
    if let Ok(p) = std::env::var("LLAMA_SERVER_PATH") {
        let p = PathBuf::from(&p);
        searched.push(p.clone());
        if p.exists() {
            return Ok(p);
        }
    }

    let server_name = if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    };

    // 2. <backend_path>/llama-server (our primary layout)
    let root_path = backend_path.join(server_name);
    searched.push(root_path.clone());
    if root_path.exists() {
        return Ok(root_path);
    }

    // 3. <backend_path>/bin/llama-server
    let bin_path = backend_path.join("bin").join(server_name);
    searched.push(bin_path.clone());
    if bin_path.exists() {
        return Ok(bin_path);
    }

    // 4. <backend_path>/build/bin/llama-server
    let build_bin_path = backend_path.join("build").join("bin").join(server_name);
    searched.push(build_bin_path.clone());
    if build_bin_path.exists() {
        return Ok(build_bin_path);
    }

    // 5. PATH lookup
    for path_dir in std::env::split_paths(&std::env::var("PATH").unwrap_or_default()) {
        let candidate = path_dir.join(server_name);
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
        "llama-server binary not found. Searched:\n{}\nInstall llama.cpp from source or set LLAMA_SERVER_PATH env var.",
        paths_list
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn server_name() -> &'static str {
        if cfg!(target_os = "windows") {
            "llama-server.exe"
        } else {
            "llama-server"
        }
    }

    #[test]
    fn test_find_llama_server_not_found() {
        let nonexistent = PathBuf::from("/nonexistent/path/backend");
        let result = find_llama_server(&nonexistent);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("llama-server binary not found"));
    }

    #[test]
    fn test_find_llama_server_root_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();
        let server_path = backend_path.join(server_name());
        fs::write(&server_path, "#!/bin/sh\necho mock").unwrap();

        let result = find_llama_server(backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), server_path);
    }

    #[test]
    fn test_find_llama_server_bin_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();
        let bin_dir = backend_path.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let server_path = bin_dir.join(server_name());
        fs::write(&server_path, "#!/bin/sh\necho mock").unwrap();

        let result = find_llama_server(backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), server_path);
    }

    #[test]
    fn test_find_llama_server_build_bin_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();
        let build_bin_dir = backend_path.join("build").join("bin");
        fs::create_dir_all(&build_bin_dir).unwrap();
        let server_path = build_bin_dir.join(server_name());
        fs::write(&server_path, "#!/bin/sh\necho mock").unwrap();

        let result = find_llama_server(backend_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), server_path);
    }

    #[test]
    fn test_find_llama_server_env_var_priority() {
        let tmp = tempfile::tempdir().unwrap();
        let backend_path = tmp.path();

        // Create a binary in bin/ dir
        let bin_dir = backend_path.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let _bin_server = bin_dir.join(server_name());
        let bin_path = bin_dir.join(server_name());
        fs::write(&bin_path, "#!/bin/sh\necho from-bin").unwrap();

        // Create a separate env var binary
        let env_dir = tmp.path().join("env_override");
        fs::create_dir_all(&env_dir).unwrap();
        let env_path = env_dir.join(server_name());
        fs::write(&env_path, "#!/bin/sh\necho from-env").unwrap();

        // Save and restore previous env var state to avoid test interference
        let prev = std::env::var_os("LLAMA_SERVER_PATH");
        std::env::set_var("LLAMA_SERVER_PATH", env_path.to_str().unwrap());
        let result = find_llama_server(backend_path);
        match prev {
            Some(val) => std::env::set_var("LLAMA_SERVER_PATH", val),
            None => std::env::remove_var("LLAMA_SERVER_PATH"),
        }

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), env_path);
    }

    #[test]
    fn test_find_llama_server_error_lists_paths() {
        let nonexistent = PathBuf::from("/nonexistent/path/backend");
        let result = find_llama_server(&nonexistent);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Searched:"));
    }
}
