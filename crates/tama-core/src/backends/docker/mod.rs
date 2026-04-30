/// Docker backend management module.
///
/// Provides functions for starting, stopping, and monitoring Docker containers
/// that run inference backends (vLLM, llama.cpp, etc.).
pub mod db;
pub mod health;
pub mod install;
pub mod logs;
pub mod templates;

use std::path::PathBuf;

/// Docker backend configuration.
pub struct DockerBackend {
    pub name: String,
    pub compose_yaml: String,
    pub dockerfile: Option<String>,
    pub target_port: Option<u16>,
    pub config_dir: PathBuf,
}

impl DockerBackend {
    /// Returns the path to the compose YAML file.
    pub fn compose_path(&self) -> PathBuf {
        self.config_dir
            .join("docker")
            .join(&self.name)
            .join("docker-compose.yaml")
    }

    /// Returns the path to the Dockerfile, if one exists.
    pub fn dockerfile_path(&self) -> Option<PathBuf> {
        self.dockerfile.as_ref().map(|_| {
            self.config_dir
                .join("docker")
                .join(&self.name)
                .join("Dockerfile")
        })
    }

    /// Returns the container name (prefixed with `tama_`).
    pub fn container_name(&self) -> String {
        format!("tama_{}", self.name)
    }
}
