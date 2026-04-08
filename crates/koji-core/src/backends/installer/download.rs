use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::backends::ProgressSink;
use crate::models::download::parse_content_length;

/// Download a file from a URL to a destination path.
///
/// When `progress.is_some()`, skips `indicatif` and emits throttled progress lines
/// via the sink (format: "downloaded {hsz_done} / {hsz_total} ({pct}%)").
///
/// When `progress.is_none()`, preserves the existing `indicatif` TTY bar behavior.
pub async fn download_file(
    url: &str,
    dest: &Path,
    progress: Option<&Arc<dyn ProgressSink>>,
) -> Result<()> {
    let client = Client::builder()
        .user_agent("koji-backend-manager")
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(30))
        .build()?;

    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download from {}", url))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_size = parse_content_length(response.headers()).unwrap_or(0);

    if let Some(sink) = progress {
        // Web path: no indicatif, emit throttled progress lines
        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();
        let mut last_emit = tokio::time::Instant::now();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // Throttle emissions to ~1 per 250ms
            if downloaded % (1024 * 1024) < chunk.len() as u64
                || last_emit.elapsed() >= Duration::from_millis(250)
            {
                let pct = if total_size > 0 {
                    (downloaded as f64 / total_size as f64) * 100.0
                } else {
                    0.0
                };
                let done_mb = downloaded as f64 / 1_048_576.0;
                let total_mb = total_size as f64 / 1_048_576.0;
                let msg = format!(
                    "downloaded {:.1} MiB / {:.1} MiB ({:.0}%)",
                    done_mb, total_mb, pct
                );
                sink.log(&msg);
                last_emit = tokio::time::Instant::now();
            }
        }

        // Flush to ensure all data is written to disk before returning
        file.flush().await?;
        Ok(())
    } else {
        // CLI path: preserve indicatif TTY bar
        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut file = tokio::fs::File::create(dest).await?;
        let mut downloaded = 0u64;
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        // Flush to ensure all data is written to disk before returning
        file.flush().await?;

        pb.finish_and_clear();
        Ok(())
    }
}
