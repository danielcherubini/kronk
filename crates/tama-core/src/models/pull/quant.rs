/// Try to infer the quantisation type from a GGUF filename.
/// Common patterns: "Model-Q4_K_M.gguf", "model.Q8_0.gguf", "model-q4_k_m.gguf"
pub fn infer_quant_from_filename(filename: &str) -> Option<String> {
    let stem = filename.strip_suffix(".gguf")?;

    // Ordered longest-first so "Q4_K_M" matches before "Q4_K"
    // Includes UD (Unsloth Dynamic) and APEX variants
    let quant_patterns = [
        // APEX semantic quants (must come before APEX standard patterns)
        "APEX-I-BALANCED",
        "APEX-I-QUALITY",
        "APEX-I-COMPACT",
        "APEX-I-MINI",
        // APEX IQ quants
        "APEX-IQ2_XXS",
        "APEX-IQ3_XXS",
        "APEX-IQ1_S",
        "APEX-IQ1_M",
        "APEX-IQ2_XS",
        "APEX-IQ2_S",
        "APEX-IQ2_M",
        "APEX-IQ3_XS",
        "APEX-IQ3_S",
        "APEX-IQ3_M",
        "APEX-IQ4_XS",
        "APEX-IQ4_NL",
        // APEX standard quants
        "APEX-Q2_K_S",
        "APEX-Q3_K_S",
        "APEX-Q3_K_M",
        "APEX-Q3_K_L",
        "APEX-Q4_K_S",
        "APEX-Q4_K_M",
        "APEX-Q4_K_L",
        "APEX-Q5_K_S",
        "APEX-Q5_K_M",
        "APEX-Q5_K_L",
        "APEX-Q6_K",
        "APEX-Q8_0",
        // UD semantic quants (must come before UD standard patterns)
        "UD-I-BALANCED",
        "UD-I-QUALITY",
        "UD-I-COMPACT",
        "UD-I-MINI",
        // Unsloth Dynamic (UD) IQ quants
        "UD-IQ2_XXS",
        "UD-IQ3_XXS",
        "UD-IQ1_S",
        "UD-IQ1_M",
        "UD-IQ2_XS",
        "UD-IQ2_S",
        "UD-IQ2_M",
        "UD-IQ3_XS",
        "UD-IQ3_S",
        "UD-IQ3_M",
        "UD-IQ4_XS",
        "UD-IQ4_NL",
        // Unsloth Dynamic (UD) standard quants
        "UD-Q2_K_S",
        "UD-Q3_K_S",
        "UD-Q3_K_M",
        "UD-Q3_K_L",
        "UD-Q4_K_S",
        "UD-Q4_K_M",
        "UD-Q4_K_L",
        "UD-Q5_K_S",
        "UD-Q5_K_M",
        "UD-Q5_K_L",
        "UD-Q2_K_XL",
        "UD-Q3_K_XL",
        "UD-Q4_K_XL",
        "UD-Q5_K_XL",
        "UD-Q6_K_XL",
        "UD-Q8_K_XL",
        "UD-Q2_K",
        "UD-Q3_K",
        "UD-Q4_K",
        "UD-Q5_K",
        "UD-Q6_K",
        "UD-Q4_0",
        "UD-Q4_1",
        "UD-Q5_0",
        "UD-Q5_1",
        "UD-Q6_0",
        "UD-Q8_0",
        "UD-Q8_1",
        // Standard quants
        "IQ2_XXS",
        "IQ3_XXS",
        "IQ1_S",
        "IQ1_M",
        "IQ2_XS",
        "IQ2_S",
        "IQ2_M",
        "IQ3_XS",
        "IQ3_S",
        "IQ3_M",
        "IQ4_XS",
        "IQ4_NL",
        "Q2_K_S",
        "Q3_K_S",
        "Q3_K_M",
        "Q3_K_L",
        "Q4_K_S",
        "Q4_K_M",
        "Q4_K_L",
        "Q5_K_S",
        "Q5_K_M",
        "Q5_K_L",
        "Q2_K_XL",
        "Q3_K_XL",
        "Q4_K_XL",
        "Q5_K_XL",
        "Q6_K_XL",
        "Q8_K_XL",
        "Q2_K",
        "Q3_K",
        "Q4_K",
        "Q5_K",
        "Q6_K",
        "Q4_0",
        "Q4_1",
        "Q5_0",
        "Q5_1",
        "Q6_0",
        "Q8_0",
        "Q8_1",
        "F16",
        "F32",
        "BF16",
    ];

    let stem_upper = stem.to_uppercase();
    for pattern in &quant_patterns {
        // Check for pattern preceded by a separator (-, ., _) or at start of string
        // This prevents false matches like "XQ4_K_M" matching "Q4_K_M"
        if stem_upper == *pattern
            || stem_upper.contains(&format!("-{}", pattern))
            || stem_upper.contains(&format!(".{}", pattern))
            || stem_upper.contains(&format!("_{}", pattern))
        {
            return Some(pattern.to_string());
        }
    }

    // No standard quant pattern found. Fall back to the last component
    // after splitting by `-` or `_`. For "Qwen3.5-35B-A3B-APEX-I-Balanced",
    // this returns "I-Balanced" instead of the full stem.
    stem.split(|c| ['-', '_'].contains(&c))
        .next_back()
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_quant_q4_k_m() {
        assert_eq!(
            infer_quant_from_filename("OmniCoder-8B-Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_q8_0() {
        assert_eq!(
            infer_quant_from_filename("model-Q8_0.gguf"),
            Some("Q8_0".to_string())
        );
    }

    #[test]
    fn test_infer_quant_non_standard_name() {
        // APEX semantic quants are now recognized
        // "Qwen3.5-35B-A3B-APEX-I-Balanced" -> "APEX-I-BALANCED"
        assert_eq!(
            infer_quant_from_filename("Qwen3.5-35B-A3B-APEX-I-Balanced.gguf"),
            Some("APEX-I-BALANCED".to_string())
        );
    }

    #[test]
    fn test_infer_quant_with_underscore() {
        assert_eq!(
            infer_quant_from_filename("model-Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
        // Returns the matched pattern, not the full suffix
        assert_eq!(
            infer_quant_from_filename("model-Q4_K_M_v2.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_lowercase() {
        assert_eq!(
            infer_quant_from_filename("model-q4_k_m.gguf"),
            Some("Q4_K_M".to_string())
        );
    }

    #[test]
    fn test_infer_quant_f16() {
        assert_eq!(
            infer_quant_from_filename("model-F16.gguf"),
            Some("F16".to_string())
        );
    }

    #[test]
    fn test_infer_quant_none() {
        // Returns last component when no pattern matches
        assert_eq!(
            infer_quant_from_filename("model.gguf"),
            Some("model".to_string())
        );
    }

    #[test]
    fn test_infer_quant_dot_separator() {
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-1B-Instruct.Q6_K.gguf"),
            Some("Q6_K".to_string())
        );
    }

    #[test]
    fn test_infer_quant_iq() {
        assert_eq!(
            infer_quant_from_filename("model-IQ4_NL.gguf"),
            Some("IQ4_NL".to_string())
        );
    }

    #[test]
    fn test_infer_quant_xl() {
        assert_eq!(
            infer_quant_from_filename("model-Q4_K_XL.gguf"),
            Some("Q4_K_XL".to_string())
        );
    }

    #[test]
    fn test_infer_quant_xl_lowercase() {
        assert_eq!(
            infer_quant_from_filename("model-q5_k_xl.gguf"),
            Some("Q5_K_XL".to_string())
        );
    }

    #[test]
    fn test_infer_quant_ud() {
        assert_eq!(
            infer_quant_from_filename("model-UD-Q4_K_XL.gguf"),
            Some("UD-Q4_K_XL".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-UD-Q4_K_M.gguf"),
            Some("UD-Q4_K_M".to_string())
        );
    }

    // ── APEX and UD semantic quant tests ──────────────────────────────────────

    #[test]
    fn test_infer_quant_apex_patterns() {
        // APEX IQ quants
        assert_eq!(
            infer_quant_from_filename("model-APEX-IQ2_XXS.gguf"),
            Some("APEX-IQ2_XXS".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-APEX-IQ3_XXS.gguf"),
            Some("APEX-IQ3_XXS".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-APEX-IQ4_NL.gguf"),
            Some("APEX-IQ4_NL".to_string())
        );
        // APEX standard quants
        assert_eq!(
            infer_quant_from_filename("model-APEX-Q4_K_M.gguf"),
            Some("APEX-Q4_K_M".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Llama-3.2-APEX-Q8_0.gguf"),
            Some("APEX-Q8_0".to_string())
        );
    }

    #[test]
    fn test_infer_quant_apex_semantic() {
        // APEX semantic quants (I-Balanced, I-Quality, etc.)
        // Note: function returns uppercase patterns
        assert_eq!(
            infer_quant_from_filename("gemma-4-26B-A4B-APEX-I-Balanced.gguf"),
            Some("APEX-I-BALANCED".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Qwen3.5-35B-A3B-APEX-I-Quality.gguf"),
            Some("APEX-I-QUALITY".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-APEX-I-Compact.gguf"),
            Some("APEX-I-COMPACT".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-APEX-I-Mini.gguf"),
            Some("APEX-I-MINI".to_string())
        );
    }

    #[test]
    fn test_infer_quant_ud_semantic() {
        // UD semantic quants
        // Note: function returns uppercase patterns
        assert_eq!(
            infer_quant_from_filename("gemma-4-26B-A4B-UD-I-Balanced.gguf"),
            Some("UD-I-BALANCED".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("Qwen3.5-35B-A3B-UD-I-Quality.gguf"),
            Some("UD-I-QUALITY".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-UD-I-Compact.gguf"),
            Some("UD-I-COMPACT".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-UD-I-Mini.gguf"),
            Some("UD-I-MINI".to_string())
        );
    }

    #[test]
    fn test_infer_quant_semantic_without_prefix() {
        // Semantic quants without APEX/UD prefix should fall back gracefully
        // Returns last component from original stem (preserves case)
        assert_eq!(
            infer_quant_from_filename("model-I-Balanced.gguf"),
            Some("Balanced".to_string())
        );
        assert_eq!(
            infer_quant_from_filename("model-Quality.gguf"),
            Some("Quality".to_string())
        );
    }
}
