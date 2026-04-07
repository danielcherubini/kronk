#[cfg(test)]
mod tests {
    use crate::config::QuantKind;

    #[test]
    fn test_quant_kind_detects_mmproj_files() {
        assert_eq!(
            QuantKind::from_filename("mmproj-F16.gguf"),
            QuantKind::Mmproj
        );
        assert_eq!(
            QuantKind::from_filename("mmproj-model-name.gguf"),
            QuantKind::Mmproj
        );
        assert_eq!(
            QuantKind::from_filename("mmproj-Q4_K_M.gguf"),
            QuantKind::Mmproj
        );
        // Case-insensitive
        assert_eq!(
            QuantKind::from_filename("MMPROJ-F16.GGUF"),
            QuantKind::Mmproj
        );
    }

    #[test]
    fn test_quant_kind_defaults_to_model_for_regular_quants() {
        assert_eq!(
            QuantKind::from_filename("model-Q4_K_M.gguf"),
            QuantKind::Model
        );
        assert_eq!(QuantKind::from_filename("mmproj.bin"), QuantKind::Model);
        assert_eq!(QuantKind::from_filename("model.gguf"), QuantKind::Model);
    }
}
