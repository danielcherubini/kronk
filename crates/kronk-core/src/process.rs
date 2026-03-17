use anyhow::{Context, Result};
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::interval;

use crate::config::HealthCheck;
use crate::logging;

#[derive(Debug, Clone)]
pub enum ProcessEvent {
    Started,
    Ready,
    Output(String),
    Crashed(String),
    Restarting {
        attempt: u32,
        max: u32,
    },
    Stopped,
    HealthCheck {
        alive: bool,
        healthy: bool,
        uptime_secs: u64,
        restarts: u32,
    },
}

pub struct ProcessSupervisor {
    exe_path: String,
    args: Vec<String>,
    health_check: HealthCheck,
    max_restarts: u32,
    restart_delay_ms: u64,
    log_dir: Option<std::path::PathBuf>,
}

impl ProcessSupervisor {
    pub fn new(
        exe_path: String,
        args: Vec<String>,
        health_check: HealthCheck,
        max_restarts: u32,
        restart_delay_ms: u64,
    ) -> Self {
        Self {
            exe_path,
            args,
            health_check,
            max_restarts,
            restart_delay_ms,
            log_dir: None,
        }
    }

    pub fn with_log_dir(mut self, log_dir: std::path::PathBuf) -> Self {
        self.log_dir = Some(log_dir);
        self
    }

    /// Run the supervisor. Listens for shutdown on `shutdown_rx`.
    /// If `shutdown_rx` is None, listens for ctrl-c instead.
    pub async fn run(
        &self,
        tx: mpsc::UnboundedSender<ProcessEvent>,
        mut shutdown_rx: Option<mpsc::Receiver<()>>,
    ) -> Result<()> {
        let mut restart_count: u32 = 0;

        loop {
            let mut child = Command::new(&self.exe_path)
                .args(&self.args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .with_context(|| format!("Failed to spawn: {}", self.exe_path))?;

            let start_time = std::time::Instant::now();
            tx.send(ProcessEvent::Started).ok();

            // Open log file if log_dir is set
            let log_file = if let Some(ref log_dir) = self.log_dir {
                if let Ok(f) = logging::open_log(log_dir, "default") {
                    Some(Arc::new(Mutex::new(f)))
                } else {
                    None
                }
            } else {
                None
            };

            // Stream stdout
            let stdout = child.stdout.take();
            let tx_out = tx.clone();
            let log_file_out = log_file.clone();
            let stdout_handle = tokio::spawn(async move {
                if let Some(stdout) = stdout {
                    let reader = BufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tx_out.send(ProcessEvent::Output(line.clone())).ok();
                        if let Some(ref f) = log_file_out {
                            let _ = f.lock().unwrap().write_all((line + "\n").as_bytes());
                        }
                    }
                }
            });

            // Stream stderr
            let stderr = child.stderr.take();
            let tx_err = tx.clone();
            let log_file_err = log_file.clone();
            let stderr_handle = tokio::spawn(async move {
                if let Some(stderr) = stderr {
                    let reader = BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tx_err.send(ProcessEvent::Output(line.clone())).ok();
                        if let Some(ref f) = log_file_err {
                            let _ = f.lock().unwrap().write_all((line + "\n").as_bytes());
                        }
                    }
                }
            });

            // Health check loop
            let mut health_interval = interval(Duration::from_millis(
                self.health_check.interval_ms.unwrap_or(5000),
            ));
            let mut server_ready = false;
            let timeout = Duration::from_millis(self.health_check.timeout_ms.unwrap_or(3000));
            let http_client = reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .unwrap_or_default();

            enum ExitReason {
                ProcessExited(std::io::Result<std::process::ExitStatus>),
                Shutdown,
            }

            let exit_reason = loop {
                tokio::select! {
                    status = child.wait() => {
                        break ExitReason::ProcessExited(status);
                    }
                    _ = health_interval.tick() => {
                        let alive = child.try_wait().map(|s| s.is_none()).unwrap_or(false);
                        let healthy = if let Some(url) = &self.health_check.url {
                            http_client.get(url).send().await
                                .map(|r| r.status().is_success())
                                .unwrap_or(false)
                        } else {
                            alive
                        };

                        if healthy && !server_ready {
                            server_ready = true;
                            tx.send(ProcessEvent::Ready).ok();
                        }

                        tx.send(ProcessEvent::HealthCheck {
                            alive,
                            healthy,
                            uptime_secs: start_time.elapsed().as_secs(),
                            restarts: restart_count,
                        }).ok();
                    }
                    _ = async {
                        match &mut shutdown_rx {
                            Some(rx) => { rx.recv().await; },
                            None => { tokio::signal::ctrl_c().await.ok(); },
                        }
                    } => {
                        break ExitReason::Shutdown;
                    }
                }
            };

            // Clean up child and stream tasks
            stdout_handle.abort();
            stderr_handle.abort();

            match exit_reason {
                ExitReason::Shutdown => {
                    tracing::info!("Shutdown signal received, killing child process");
                    child.kill().await.ok();
                    // Wait for it to actually exit
                    child.wait().await.ok();
                    tx.send(ProcessEvent::Stopped).ok();
                    return Ok(());
                }
                ExitReason::ProcessExited(status) => match status {
                    Ok(s) => {
                        let msg = format!("Process exited with {}", s);
                        tx.send(ProcessEvent::Crashed(msg)).ok();
                    }
                    Err(e) => {
                        let msg = format!("Process error: {}", e);
                        tx.send(ProcessEvent::Crashed(msg)).ok();
                    }
                },
            }

            restart_count += 1;
            if restart_count > self.max_restarts {
                tracing::error!("Max restarts ({}) exceeded, giving up", self.max_restarts);
                tx.send(ProcessEvent::Stopped).ok();
                return Ok(());
            }

            tx.send(ProcessEvent::Restarting {
                attempt: restart_count,
                max: self.max_restarts,
            })
            .ok();

            tokio::time::sleep(Duration::from_millis(self.restart_delay_ms)).await;
        }
    }
}
