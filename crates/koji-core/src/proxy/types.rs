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
    pub config: Arc<tokio::sync::RwLock<crate::config::Config>>,
    pub models: Arc<tokio::sync::RwLock<std::collections::HashMap<String, ModelState>>>,
    pub client: reqwest::Client,
    pub metrics: Arc<ProxyMetrics>,
    pub db_dir: Option<std::path::PathBuf>,
    pub pull_jobs: Arc<tokio::sync::RwLock<std::collections::HashMap<String, PullJob>>>,
    pub system_metrics: Arc<tokio::sync::RwLock<crate::gpu::SystemMetrics>>,
    /// Set of destination paths currently being downloaded. Used to prevent
    /// concurrent downloads writing to the same temp files, which would silently
    /// corrupt the assembled output.
    pub in_flight_downloads: Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
    pub metrics_tx: tokio::sync::broadcast::Sender<crate::gpu::MetricSample>,
}

impl ProxyState {
    /// Open a DB connection for a quick sync operation.
    /// Returns None if db_dir is not configured (e.g., in tests).
    pub fn open_db(&self) -> Option<rusqlite::Connection> {
        self.db_dir
            .as_ref()
            .and_then(|dir| crate::db::open(dir).ok().map(|r| r.conn))
    }

    /// Gracefully shut down the proxy state.
    ///
    /// This method is called during a hard restart to clean up resources:
    /// - Closes the metrics broadcast channel to stop metrics streaming
    /// - Clears all loaded models from the models map
    /// - Clears active pull jobs
    /// - Clears in-flight downloads
    pub async fn shutdown(&self) {
        // Close the metrics broadcast channel to stop the metrics stream
        let _ = self.metrics_tx.send(crate::gpu::MetricSample {
            ts_unix_ms: 0,
            cpu_usage_pct: 0.0,
            ram_used_mib: 0,
            ram_total_mib: 0,
            gpu_utilization_pct: None,
            vram: None,
            models_loaded: 0,
        });

        // Clear all loaded models
        let mut models = self.models.write().await;
        // Kill backend processes before clearing
        for entry in models.iter() {
            if let ModelState::Ready { backend_pid, .. } = entry.1 {
                let _ = crate::proxy::process::kill_process(*backend_pid).await;
            }
        }
        models.clear();

        // Clear active pull jobs
        let mut pull_jobs = self.pull_jobs.write().await;
        pull_jobs.clear();

        // Clear in-flight downloads
        let mut in_flight = self.in_flight_downloads.lock().await;
        in_flight.clear();
    }
}
