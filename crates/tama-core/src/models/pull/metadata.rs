use std::time::Duration;

use reqwest;
use toml;

use crate::models::card::ModelCard;
use crate::models::pull::HfModelMetadata;

const MODELCARDS_BASE_URL: &str =
    "https://raw.githubusercontent.com/danielcherubini/tama/main/modelcards";

/// Parse a HuggingFace README markdown to extract model metadata.
///
/// This is a pure function — no I/O. Returns a `HfModelMetadata` with fields
/// populated from whatever could be extracted. Missing values are `None`.
///
/// # Patterns recognised
///
/// - **Total parameters**: "Number of Parameters: 35B", "Total Parameters | 25.2B"
/// - **Active parameters**: "3B activated", "Active Parameters | 3.8B"
/// - **Context length**: "Context Length: 262,144", "128K tokens" (K → ×1024)
/// - **Number of layers**: "Number of Layers: 40", "Layers | 30"
/// - **Architecture type**: inferred from active_params (MoE), Mamba text (Mamba2-Transformer MoE), or Dense
pub fn parse_readme_metadata(markdown: &str) -> HfModelMetadata {
    let mut meta = HfModelMetadata::default();

    // ── Total Parameters ────────────────────────────────────────────────────
    // Patterns: "Number of Parameters: 35B", "Total Parameters | 25.2B"
    let total_params = extract_param_value(markdown, &["Number of Parameters", "Total Parameters"]);
    meta.hf_total_params = total_params;

    // ── Active Parameters ───────────────────────────────────────────────────
    // Patterns: "3B activated", "Active Parameters | 3.8B"
    let active_params = extract_param_value(markdown, &["Active Parameters"]);
    if active_params.is_none() {
        // Also try the "X B/Y B activated" pattern (e.g. "3B/35B activated")
        if let Some(s) = markdown.lines().find_map(|line| {
            let lower = line.to_lowercase();
            if lower.contains("activated") || lower.contains("active parameters") {
                // Try to find a number before "activated"
                for word in line.split_whitespace() {
                    if let Some(val) = parse_param_shorthand(word) {
                        return Some(val);
                    }
                }
            }
            None
        }) {
            meta.hf_active_params = Some(s);
        }
    } else {
        meta.hf_active_params = active_params;
    }

    // ── Context Length ──────────────────────────────────────────────────────
    // Patterns: "Context Length: 262,144", "128K tokens", "4096"
    if meta.hf_context_length.is_none() {
        for line in markdown.lines() {
            let lower = line.to_lowercase();
            if lower.contains("context length") || lower.contains("context size") {
                // Strip the label and look for a number
                let after_colon = line.split(':').nth(1);
                if let Some(rest) = after_colon {
                    if let Some(val) = parse_context_length(rest.trim()) {
                        meta.hf_context_length = Some(val);
                        break;
                    }
                }
            }
        }
    }
    // Also try table-style: "Context Length | 131072"
    if meta.hf_context_length.is_none() {
        for line in markdown.lines() {
            let lower = line.to_lowercase();
            if (lower.contains("context length") || lower.contains("context size"))
                && line.contains('|')
            {
                let parts: Vec<&str> = line.split('|').collect();
                // Find the label column, then take the next one for the value
                for i in 0..parts.len() {
                    let part_lower = parts[i].to_lowercase();
                    if (part_lower.contains("context length")
                        || part_lower.contains("context size"))
                        && i + 1 < parts.len()
                    {
                        if let Some(val) = parse_context_length(parts[i + 1].trim()) {
                            meta.hf_context_length = Some(val);
                            break;
                        }
                    }
                }
                if meta.hf_context_length.is_some() {
                    break;
                }
            }
        }
    }

    // ── Number of Layers ────────────────────────────────────────────────────
    // Patterns: "Number of Layers: 40", "Layers | 30"
    for line in markdown.lines() {
        let lower = line.to_lowercase();
        if lower.contains("number of layers") || lower.contains("num layers") {
            if let Some(rest) = line.split(':').nth(1) {
                if let Some(val) = parse_u32(rest.trim()) {
                    meta.hf_num_layers = Some(val);
                    break;
                }
            }
            // Also try pipe-separated for table rows
            if line.contains('|') && meta.hf_num_layers.is_none() {
                let parts: Vec<&str> = line.split('|').collect();
                for i in 0..parts.len() {
                    let part_lower = parts[i].to_lowercase();
                    if (part_lower.contains("number of layers")
                        || part_lower.contains("num layers"))
                        && i + 1 < parts.len()
                    {
                        if let Some(val) = parse_u32(parts[i + 1].trim()) {
                            meta.hf_num_layers = Some(val);
                            break;
                        }
                    }
                }
            }
        } else if (lower.contains("layers") || lower.contains("depth")) && line.contains('|') {
            // Table row: "Layers | 30"
            let parts: Vec<&str> = line.split('|').collect();
            // Find the label column, then take the next one for the value
            for i in 0..parts.len() {
                let part_lower = parts[i].to_lowercase();
                if (part_lower.contains("layers") || part_lower.contains("depth"))
                    && i + 1 < parts.len()
                {
                    if let Some(val) = parse_u32(parts[i + 1].trim()) {
                        meta.hf_num_layers = Some(val);
                        break;
                    }
                }
            }
            if meta.hf_num_layers.is_some() {
                break;
            }
        }
    }

    // ── Architecture Type (inferred) ────────────────────────────────────────
    if meta.hf_active_params.is_some() {
        meta.hf_architecture_type = Some("MoE".to_string());
    } else if markdown.to_lowercase().contains("mamba") {
        meta.hf_architecture_type = Some("Mamba2-Transformer MoE".to_string());
    } else if markdown.to_lowercase().contains("dense") {
        meta.hf_architecture_type = Some("Dense".to_string());
    }
    // Otherwise leave as None — we can't infer the architecture type

    meta
}

/// Extract a parameter value like "35B" or "25.2B" from lines containing one of the given labels.
fn extract_param_value(markdown: &str, labels: &[&str]) -> Option<String> {
    for line in markdown.lines() {
        let lower = line.to_lowercase();
        for label in labels {
            let label_lower = label.to_lowercase();
            if lower.contains(&label_lower) {
                // Try colon-separated first: "Label: 35B"
                if let Some(rest) = line.split(':').nth(1) {
                    if let Some(val) = extract_first_param_shorthand(rest.trim()) {
                        return Some(val);
                    }
                }
                // Try pipe-separated: "Label | 35B" (skip the label column itself)
                if line.contains('|') {
                    let parts: Vec<&str> = line.split('|').collect();
                    // Find the part that matches the label, then take the next one
                    for i in 0..parts.len() {
                        if parts[i].to_lowercase().contains(&label_lower) && i + 1 < parts.len() {
                            if let Some(val) = extract_first_param_shorthand(parts[i + 1].trim()) {
                                return Some(val);
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extract the first valid param shorthand from a string, handling trailing text.
/// E.g., "35B" → "35B", "128K tokens" → "128K", "262,144" → "262144"
fn extract_first_param_shorthand(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Try to find a number (possibly with decimal) followed by an optional suffix
    // Patterns: "35B", "25.2B", "1T", "128K tokens", "262,144"

    // Check for suffix patterns (B, K, M, T) possibly followed by other text.
    // Iterate over the original string `s` and uppercase individual characters
    // for comparison — using byte indices from `s.to_uppercase()` would be
    // unsound since to_uppercase() can change string length for some Unicode chars.
    if let Some((i, ch)) = s.char_indices().find(|(_, c)| {
        let upper = c.to_ascii_uppercase();
        !(upper.is_ascii_digit() || upper == '.' || upper == ',')
    }) {
        if i > 0 {
            // Digits found at start
            let num_str = &s[..i];
            if (ch == 'B' || ch == 'b') && num_str.replace(',', "").trim().parse::<f64>().is_ok() {
                return Some(format!("{}B", num_str.trim()));
            } else if (ch == 'K' || ch == 'k')
                && num_str.replace(',', "").trim().parse::<f64>().is_ok()
            {
                return Some(format!("{}K", num_str.trim()));
            } else if (ch == 'M' || ch == 'm')
                && num_str.replace(',', "").trim().parse::<f64>().is_ok()
            {
                return Some(format!("{}M", num_str.trim()));
            } else if (ch == 'T' || ch == 't')
                && num_str.replace(',', "").trim().parse::<f64>().is_ok()
            {
                return Some(format!("{}T", num_str.trim()));
            }
        }
    }

    // Try plain number (possibly comma-separated)
    let cleaned = s.split_whitespace().next().unwrap_or(s).replace(',', "");
    if let Ok(n) = cleaned.parse::<u64>() {
        return Some(n.to_string());
    }

    None
}

/// Parse a shorthand parameter value like "35B", "25.2B", "3.8B", "1T".
fn parse_param_shorthand(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Check if it ends with a known suffix
    let (num_str, suffix) = if s.ends_with('T') || s.ends_with('t') {
        (&s[..s.len() - 1], "T")
    } else if s.ends_with('B') || s.ends_with('b') {
        (&s[..s.len() - 1], "B")
    } else if s.ends_with('M') || s.ends_with('m') {
        // Be careful: "262,144" doesn't end with m but "256M" does
        // Only match if it looks like a number + M
        let candidate = &s[..s.len() - 1];
        if candidate.parse::<f64>().is_ok() {
            (candidate, "M")
        } else {
            return None;
        }
    } else if s.ends_with('K') || s.ends_with('k') {
        let candidate = &s[..s.len() - 1];
        if candidate.parse::<f64>().is_ok() {
            (candidate, "K")
        } else {
            return None;
        }
    } else {
        // Plain number
        return s.parse::<u64>().ok().map(|n| n.to_string());
    };

    // Validate the numeric part
    if num_str.trim().parse::<f64>().is_ok() {
        Some(format!("{}{}", num_str.trim(), suffix))
    } else {
        None
    }
}

/// Parse a context length string like "262,144", "131072", "128K", "2M".
/// Handles trailing text like "128K tokens".
fn parse_context_length(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Extract the first token (number possibly with suffix)
    let first_token = s.split_whitespace().next().unwrap_or(s);
    let first_token = first_token.trim();

    // Handle K suffix: 128K → 131072
    if first_token.ends_with('K') || first_token.ends_with('k') {
        let num_str = &first_token[..first_token.len() - 1];
        if let Ok(n) = num_str.trim().parse::<u32>() {
            return Some(n * 1024);
        }
    }

    // Handle M suffix: 2M → 2097152
    if first_token.ends_with('M') || first_token.ends_with('m') {
        let num_str = &first_token[..first_token.len() - 1];
        if let Ok(n) = num_str.trim().parse::<u32>() {
            return Some(n * 1024 * 1024);
        }
    }

    // Handle comma-separated: "262,144"
    let cleaned = first_token.replace(',', "");
    if let Ok(n) = cleaned.parse::<u32>() {
        return Some(n);
    }

    None
}

/// Parse a u32 from a trimmed string (handles commas).
fn parse_u32(s: &str) -> Option<u32> {
    let cleaned = s.replace(',', "");
    cleaned.trim().parse::<u32>().ok()
}

/// Try to fetch a community model card from the tama repository.
///
/// Attempts several name variants derived from the repo_id:
/// 1. Exact: `{company}/{model}.toml` (e.g. `Tesslate/OmniCoder-9B-GGUF.toml`)
/// 2. Strip `-GGUF` suffix: `Tesslate/OmniCoder-9B.toml`
/// 3. Strip `-gguf` suffix (lowercase)
///
/// Returns `None` silently on network errors or 404s.
pub async fn fetch_community_card(repo_id: &str) -> Option<ModelCard> {
    let parts: Vec<&str> = repo_id.splitn(2, '/').collect();
    if parts.len() != 2 {
        return None;
    }
    let (company, model) = (parts[0], parts[1]);

    // Build candidate names: exact, then stripped variants
    let mut candidates = vec![model.to_string()];
    for suffix in ["-GGUF", "-gguf", "-Gguf"] {
        if let Some(stripped) = model.strip_suffix(suffix) {
            candidates.push(stripped.to_string());
        }
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;

    for name in &candidates {
        let url = format!("{}/{}/{}.toml", MODELCARDS_BASE_URL, company, name);
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    if let Ok(card) = toml::from_str::<ModelCard>(&body) {
                        return Some(card);
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests;
