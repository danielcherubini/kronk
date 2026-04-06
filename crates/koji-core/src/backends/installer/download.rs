use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use crate::models::download::parse_content_length;

/// Download a file from a URL to a destination path with progress bar.
pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
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
