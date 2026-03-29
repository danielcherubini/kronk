//! HTTP streaming measurement client for benchmarking LLM inference.
//!
//! This module provides the `send_bench_request` function that sends a single
//! OpenAI-compatible chat completion request to a running llama-server and
//! measures timing from the SSE (Server-Sent Events) stream.

use std::time::Instant;

use anyhow::{bail, Context, Result};
use futures_util::StreamExt;

/// Sends a benchmark request to the LLM server and measures performance metrics.
///
/// This function sends a POST to `/v1/chat/completions` with streaming enabled,
/// then reads the SSE stream to measure:
/// - Time to first token (TTFT)
/// - Total number of generated tokens
/// - Total request time
/// - Prompt processing speed (tokens/sec)
/// - Token generation speed (tokens/sec)
pub async fn send_bench_request(
    base_url: &str,
    prompt_tokens: u32,
    max_tokens: u32,
) -> Result<crate::bench::RequestMeasurement> {
    // Build the request body
    let prompt = crate::bench::build_prompt(prompt_tokens);
    let request_body = serde_json::json!({
        "model": "benchmark",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant. Continue generating text without stopping."},
            {"role": "user", "content": prompt}
        ],
        "max_tokens": max_tokens,
        "stream": true
    });

    // Record request start time immediately before sending
    let request_start = Instant::now();

    // Create client with 300-second timeout for large prompts
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .context("Failed to create HTTP client")?;

    // Send the request
    let response = client
        .post(format!("{}/v1/chat/completions", base_url))
        .json(&request_body)
        .send()
        .await
        .context("Failed to send request")?;

    if !response.status().is_success() {
        bail!("Request failed with status: {}", response.status());
    }

    // Read the SSE stream and process lines incrementally
    let mut first_token_time: Option<Instant> = None;
    let mut generated_token_count = 0u32;
    let mut request_end_time: Option<Instant> = None;
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Failed to read response chunk")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process all complete lines in the buffer
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
            buffer.drain(..=newline_pos);

            if let Some(sse_data) = line.strip_prefix("data: ") {
                if sse_data == "[DONE]" {
                    request_end_time = Some(Instant::now());
                    break;
                }

                // Parse the SSE content
                if let Some(_content) = parse_sse_content(&line) {
                    generated_token_count += 1;

                    // This is the first chunk with content (TTFT)
                    if first_token_time.is_none() {
                        first_token_time = Some(Instant::now());
                    }
                }
            }
        }

        if request_end_time.is_some() {
            break;
        }
    }

    // Compute metrics
    let end_time = request_end_time.unwrap_or_else(Instant::now);
    let ttft_ms = if let (Some(first), Some(start)) = (first_token_time, Some(request_start)) {
        first.duration_since(start).as_secs_f64() * 1000.0
    } else {
        0.0
    };

    let total_ms = end_time.duration_since(request_start).as_secs_f64() * 1000.0;

    // Compute tokens per second, guarding against division by zero / Inf / NaN
    let pp_tokens_per_sec = if ttft_ms > 0.0 {
        prompt_tokens as f64 / (ttft_ms / 1000.0)
    } else {
        0.0
    };
    let tg_tokens_per_sec = if generated_token_count > 1 && total_ms > ttft_ms {
        (generated_token_count - 1) as f64 / ((total_ms - ttft_ms) / 1000.0)
    } else {
        0.0
    };

    // Check for errors
    if generated_token_count == 0 {
        bail!("No tokens generated — the model returned an empty response");
    }

    if first_token_time.is_none() {
        bail!("No tokens generated — the model returned an empty response");
    }

    Ok(crate::bench::RequestMeasurement {
        prompt_tokens,
        generated_tokens: generated_token_count,
        ttft_ms,
        total_ms,
        pp_tokens_per_sec,
        tg_tokens_per_sec,
    })
}

/// Parses an SSE line and extracts content from the JSON payload.
///
/// Returns `Some(content_string)` if the line has `data: ` prefix and the JSON
/// contains a non-empty `choices[0].delta.content`.
///
/// Returns `None` for:
/// - `data: [DONE]` lines
/// - Lines without `data: ` prefix
/// - Chunks with empty/missing content
/// - Blank lines
pub fn parse_sse_content(line: &str) -> Option<String> {
    if !line.starts_with("data: ") {
        return None;
    }

    let sse_data = &line[6..]; // Strip "data: " prefix

    if sse_data == "[DONE]" {
        return None;
    }

    if sse_data.is_empty() {
        return None;
    }

    // Parse as JSON
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(sse_data) {
        // Check for choices[0].delta.content or reasoning_content (thinking models e.g. Qwen3)
        let choices = json.get("choices");
        let choice = choices?.as_array().and_then(|arr| arr.first());
        let delta = choice?.as_object().and_then(|obj| obj.get("delta"))?;
        let delta_obj = delta.as_object()?;

        for key in &["content", "reasoning_content"] {
            if let Some(content_str) = delta_obj.get(*key).and_then(|v| v.as_str()) {
                if !content_str.is_empty() {
                    return Some(content_str.to_string());
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sse_content_with_token() {
        let line = r#"data: {"choices":[{"delta":{"content":"hello"}}]}"#;
        let result = parse_sse_content(line);
        assert_eq!(result, Some("hello".to_string()));
    }

    #[test]
    fn test_parse_sse_content_role_chunk() {
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#;
        let result = parse_sse_content(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_sse_content_done() {
        let line = "data: [DONE]";
        let result = parse_sse_content(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_sse_content_empty_line() {
        let line = "";
        let result = parse_sse_content(line);
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_sse_content_empty_content() {
        let line = r#"data: {"choices":[{"delta":{"content":""}}]}"#;
        let result = parse_sse_content(line);
        assert_eq!(result, None);
    }
}
