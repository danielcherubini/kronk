//! Server lifecycle management for llama-server.
//!
//! Spawns a `llama-server` process with the given args, waits for it to load
//! the model and become ready, then provides a `ServerHandle` that can be used
//! to make HTTP completion requests. Captures stderr for parsing spec-decoding
//! statistics (draft acceptance rate). Dropping the handle kills the server.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Arguments for starting a llama-server instance.
#[derive(Debug, Clone)]
pub struct ServerArgs {
    pub binary: PathBuf,
    pub model_path: PathBuf,
    pub port: u16,
    /// GPU layers (None = use server default).
    pub ngl: Option<u32>,
    /// Flash attention (default true).
    pub flash_attn: bool,
    /// Speculative decoding type (None = no spec decoding).
    pub spec_type: Option<super::SpecType>,
    pub spec_ngram_n: Option<u32>,
    pub spec_ngram_m: Option<u32>,
    pub spec_ngram_min_hits: Option<u32>,
    /// N-gram minimum match for n-gram-mod (maps to --spec-ngram-mod-n-min).
    pub spec_ngram_min: Option<u32>,
    /// N-gram maximum match for n-gram-mod (maps to --spec-ngram-mod-n-max).
    pub spec_ngram_max: Option<u32>,
    pub draft_max: Option<u32>,
    pub draft_min: Option<u32>,
    /// Spec draft NGL for MTP (maps to --spec-draft-ngl).
    pub spec_draft_ngl: Option<u32>,
    /// Context size (maps to -c). None = use server default.
    pub context_size: Option<u32>,
}

impl ServerArgs {
    /// Convert to a flat vector of CLI arguments for tokio::process::Command.
    #[allow(clippy::vec_init_then_push)]
    pub fn to_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        args.push("-m".to_string());
        args.push(self.model_path.to_string_lossy().to_string());

        args.push("--port".to_string());
        args.push(self.port.to_string());

        if let Some(ngl) = self.ngl {
            args.push("--n-gpu-layers".to_string());
            args.push(ngl.to_string());
        }

        args.push("-fa".to_string());
        args.push(if self.flash_attn { "on" } else { "off" }.to_string());

        // Disable web UI — we only need the API.
        args.push("--no-webui".to_string());

        // Context size.
        if let Some(ctx) = self.context_size {
            args.push("-c".to_string());
            args.push(ctx.to_string());
        }

        // Speculative decoding flags.
        if let Some(spec_type) = &self.spec_type {
            args.push("--spec-type".to_string());
            args.push(spec_type.as_str().to_string());

            // Type-specific n-gram flags (llama.cpp PR #22397).
            let (size_n_flag, size_m_flag, min_hits_flag) = spec_type.spec_ngram_flags();

            if !size_n_flag.is_empty() {
                if let Some(n) = self.spec_ngram_n {
                    args.push(size_n_flag.to_string());
                    args.push(n.to_string());
                }
            }
            if !size_m_flag.is_empty() {
                if let Some(m) = self.spec_ngram_m {
                    args.push(size_m_flag.to_string());
                    args.push(m.to_string());
                }
            }
            if !min_hits_flag.is_empty() {
                if let Some(hits) = self.spec_ngram_min_hits {
                    args.push(min_hits_flag.to_string());
                    args.push(hits.to_string());
                }
            }
            if let Some(dm) = self.draft_max {
                args.push("--spec-draft-n-max".to_string());
                args.push(dm.to_string());
            }
            if let Some(dmin) = self.draft_min {
                args.push("--spec-draft-n-min".to_string());
                args.push(dmin.to_string());
            }

            // spec-draft-ngl for MTP benchmarking — only valid for DraftMtp spec type
            if matches!(&self.spec_type, Some(super::SpecType::DraftMtp)) {
                if let Some(ngl) = self.spec_draft_ngl {
                    args.push("--spec-draft-ngl".to_string());
                    args.push(ngl.to_string());
                }
            }

            // Ngram-mod needs its own n-min and n-max flags (not covered by spec_ngram_flags).
            if matches!(spec_type, super::SpecType::NgramMod) {
                if let Some(nmin) = self.spec_ngram_min {
                    args.push("--spec-ngram-mod-n-min".to_string());
                    args.push(nmin.to_string());
                }
                if let Some(nmax) = self.spec_ngram_max {
                    args.push("--spec-ngram-mod-n-max".to_string());
                    args.push(nmax.to_string());
                }
            }
        }

        args
    }
}

/// Timing and usage data from a chat completion response.
#[derive(Debug, Clone)]
pub struct ChatTiming {
    pub predicted_per_second: f64,
    pub predicted_n: u32,      // completion_tokens
    pub draft_n: u32,          // total draft tokens proposed
    pub draft_n_accepted: u32, // draft tokens accepted
}

/// A running llama-server instance. Dropping this kills the server.
pub struct ServerHandle {
    child: Child,
    port: u16,
    /// Collected stderr lines for parsing spec-decoding statistics.
    stderr_lines: Arc<Mutex<Vec<String>>>,
}

impl ServerHandle {
    /// The base URL of the running server.
    pub fn base_url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Returns once the server has loaded the model and is ready to accept requests.
    /// Polls `/v1/models` until it returns successfully or the timeout expires.
    pub async fn wait_ready(&self, timeout_secs: u64) -> Result<()> {
        let url = format!("{}/v1/models", self.base_url());
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .context("Failed to build reqwest client")?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);

        loop {
            if tokio::time::Instant::now() >= deadline {
                bail!("llama-server did not become ready within {timeout_secs}s at {url}");
            }

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    // Server is ready.
                    return Ok(());
                }
                Ok(_resp) => {
                    // Still loading or not ready yet.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(_) => {
                    // Connection refused or network error — still starting.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    /// Make a completion request and extract the generation speed (tokens/s).
    ///
    /// Returns `Ok(predicted_per_second)` on success.
    pub async fn complete(&self, prompt: &str, max_tokens: u32) -> Result<f64> {
        #[derive(serde::Deserialize)]
        struct CompletionResponse {
            timings: Timings,
        }

        #[derive(serde::Deserialize)]
        struct Timings {
            #[serde(rename = "predicted_per_second")]
            predicted_per_second: f64,
        }

        #[derive(serde::Serialize)]
        struct Request<'a> {
            prompt: &'a str,
            #[serde(rename = "max_tokens")]
            max_tokens: u32,
            #[serde(rename = "cache_prompt")]
            cache_prompt: bool,
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build reqwest client")?;

        let request = Request {
            prompt,
            max_tokens,
            cache_prompt: true,
        };

        let url = format!("{}/v1/completions", self.base_url());
        let resp = client
            .post(&url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("HTTP request to {url} failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Server returned error {status}: {body}");
        }

        let completion: CompletionResponse = resp
            .json()
            .await
            .context("Failed to parse server JSON response")?;

        Ok(completion.timings.predicted_per_second)
    }

    /// Make a chat completion request and extract timing/usage data.
    ///
    /// POSTs to `/v1/chat/completions` with the given messages and returns
    /// timing and usage statistics.
    pub async fn chat_complete(
        &self,
        model: &str,
        messages: &[(&str, &str)],
        max_tokens: u32,
    ) -> Result<ChatTiming> {
        #[derive(serde::Deserialize)]
        struct ChatCompletionResponse {
            timings: ChatTimings,
            usage: ChatUsage,
        }

        #[derive(serde::Deserialize)]
        struct ChatTimings {
            #[serde(rename = "predicted_per_second")]
            predicted_per_second: f64,
            #[serde(rename = "draft_n")]
            draft_n: u32,
            #[serde(rename = "draft_n_accepted")]
            draft_n_accepted: u32,
        }

        #[derive(serde::Deserialize)]
        struct ChatUsage {
            #[serde(rename = "completion_tokens")]
            completion_tokens: u32,
        }

        #[derive(serde::Serialize)]
        struct Message<'a> {
            role: &'a str,
            content: &'a str,
        }

        #[derive(serde::Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            messages: Vec<Message<'a>>,
            #[serde(rename = "max_tokens")]
            max_tokens: u32,
            seed: u64,
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .context("Failed to build reqwest client")?;

        let chat_messages: Vec<Message<'_>> = messages
            .iter()
            .map(|(role, content)| Message { role, content })
            .collect();

        let request = ChatRequest {
            model,
            messages: chat_messages,
            max_tokens,
            seed: 42,
        };

        let url = format!("{}/v1/chat/completions", self.base_url());
        let resp = client
            .post(&url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("HTTP request to {url} failed"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Server returned error {status}: {body}");
        }

        let completion: ChatCompletionResponse = resp
            .json()
            .await
            .context("Failed to parse chat completion JSON response")?;

        Ok(ChatTiming {
            predicted_per_second: completion.timings.predicted_per_second,
            predicted_n: completion.usage.completion_tokens,
            draft_n: completion.timings.draft_n,
            draft_n_accepted: completion.timings.draft_n_accepted,
        })
    }

    /// Parse the draft acceptance rate from collected stderr lines.
    ///
    /// llama-server prints statistics like:
    /// `draft acceptance rate = 0.57576 (  171 accepted /   297 generated)`
    ///
    /// Returns `Some(rate)` if found, `None` otherwise.
    ///
    /// Uses `lock().await` (not `blocking_lock()`) to avoid deadlocking
    /// the tokio runtime when called from async context while the stderr
    /// reader task holds the lock.
    pub async fn parse_acceptance_rate(&self) -> Option<f64> {
        let lines = self.stderr_lines.lock().await;
        for line in lines.iter() {
            if let Some(start) = line.find("draft acceptance rate = ") {
                let after_eq = &line[start + "draft acceptance rate = ".len()..];
                if let Some(end) = after_eq.find(' ') {
                    if let Ok(rate) = after_eq[..end].parse::<f64>() {
                        return Some(rate);
                    }
                }
            }
        }
        None
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        // Best-effort kill. The process is already kill_on_drop.
        let _ = self.child.start_kill();
    }
}

/// Spawn a llama-server process with the given arguments.
///
/// Waits up to `timeout_secs` for the model to load. Returns a `ServerHandle`
/// that must be kept alive for the duration of benchmarking.
pub async fn spawn_server(args: &ServerArgs, timeout_secs: u64) -> Result<ServerHandle> {
    let arg_vec = args.to_args();

    let mut child = Command::new(&args.binary);
    child
        .args(&arg_vec)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::process::configure_backend_command(&mut child, &args.binary);

    let mut child = child
        .spawn()
        .with_context(|| format!("Failed to spawn {}", args.binary.display()))?;

    let stderr_lines = Arc::new(Mutex::new(Vec::new()));

    // Extract stderr before moving child into ServerHandle.
    let stderr = child.stderr.take();
    if let Some(stderr) = stderr {
        let lines = stderr_lines.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                lines.lock().await.push(line);
            }
        });
    }

    let handle = ServerHandle {
        child,
        port: args.port,
        stderr_lines,
    };

    handle
        .wait_ready(timeout_secs)
        .await
        .context("llama-server failed to load model and become ready")?;

    Ok(handle)
}
