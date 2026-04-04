use std::sync::Arc;
use std::time::Instant;

use super::pull_jobs::PullJob;

/// State for a model backend.
#[derive(Debug, Clone)]
pub enum ModelState {
    /// Backend is starting up (placeholder during initialization)
    Starting {
        model_name: String,
        backend: String,
        backend_url: String,
        last_accessed: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
    },
    /// Backend is ready and accepting traffic
    Ready {
        model_name: String,
        backend: String,
        backend_pid: u32,
        backend_url: String,
        load_time: std::time::SystemTime,
        last_accessed: Instant,
        consecutive_failures: Arc<std::sync::atomic::AtomicU32>,
        failure_timestamp: Option<std::time::SystemTime>,
    },
    /// Backend failed to start
    Failed {
        model_name: String,
        backend: String,
        error: String,
    },
}

impl ModelState {
    pub fn model_name(&self) -> &str {
        match self {
            ModelState::Starting { model_name, .. } => model_name,
            ModelState::Ready { model_name, .. } => model_name,
            ModelState::Failed { model_name, .. } => model_name,
        }
    }

    pub fn backend(&self) -> &str {
        match self {
            ModelState::Starting { backend, .. } => backend,
            ModelState::Ready { backend, .. } => backend,
            ModelState::Failed { backend, .. } => backend,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, ModelState::Ready { .. })
    }

    pub fn backend_url(&self) -> Option<&str> {
        match self {
            ModelState::Ready { backend_url, .. } => Some(backend_url),
            _ => None,
        }
    }

    pub fn backend_pid(&self) -> Option<u32> {
        match self {
            ModelState::Ready { backend_pid, .. } => Some(*backend_pid),
            _ => None,
        }
    }

    pub fn consecutive_failures(&self) -> Option<&Arc<std::sync::atomic::AtomicU32>> {
        match self {
            ModelState::Starting {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            ModelState::Ready {
                consecutive_failures,
                ..
            } => Some(consecutive_failures),
            ModelState::Failed { .. } => None,
        }
    }

    pub fn load_time(&self) -> Option<std::time::SystemTime> {
        match self {
            ModelState::Ready { load_time, .. } => Some(*load_time),
            _ => None,
        }
    }

    pub fn last_accessed(&self) -> Option<Instant> {
        match self {
            ModelState::Ready { last_accessed, .. } => Some(*last_accessed),
            ModelState::Starting { last_accessed, .. } => Some(*last_accessed),
            ModelState::Failed { .. } => None,
        }
    }

    /// Check if the server has failed and the cooldown has elapsed.
    pub fn can_reload(&self, cooldown_seconds: u64) -> bool {
        match self {
            ModelState::Failed { .. } => false,
            ModelState::Starting {
                failure_timestamp, ..
            }
            | ModelState::Ready {
                failure_timestamp, ..
            } => failure_timestamp
                .map(|ts| {
                    std::time::SystemTime::now()
                        .duration_since(ts)
                        .map(|d| d.as_secs() >= cooldown_seconds)
                        .unwrap_or(false)
                })
                .unwrap_or(true),
        }
    }
}

/// Metrics for the proxy server.
#[derive(Debug, Default)]
pub struct ProxyMetrics {
    pub total_requests: std::sync::atomic::AtomicU64,
    pub successful_requests: std::sync::atomic::AtomicU64,
    pub failed_requests: std::sync::atomic::AtomicU64,
    pub models_loaded: std::sync::atomic::AtomicU64,
    pub models_unloaded: std::sync::atomic::AtomicU64,
}

/// Manages proxy state and model lifecycle.
#[derive(Clone)]
pub struct ProxyState {
    pub config: crate::config::Config,
    pub models: Arc<tokio::sync::RwLock<std::collections::HashMap<String, ModelState>>>,
    pub client: reqwest::Client,
    pub metrics: Arc<ProxyMetrics>,
    pub db_dir: Option<std::path::PathBuf>,
    pub pull_jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    pub system_metrics: Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>>,
}

impl ProxyState {
    /// Open a DB connection for a quick sync operation.
    /// Returns None if db_dir is not configured (e.g., in tests).
    pub fn open_db(&self) -> Option<rusqlite::Connection> {
        self.db_dir
            .as_ref()
            .and_then(|dir| crate::db::open(dir).ok().map(|r| r.conn))
    }
}
