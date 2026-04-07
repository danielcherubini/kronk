#[cfg(test)]
mod tests {
    use crate::models::pull::is_mmproj_filename;

    #[test]
    fn test_is_mmproj_filename_positive() {
        assert!(is_mmproj_filename("mmproj-F16.gguf"));
        assert!(is_mmproj_filename("mmproj-model-name.gguf"));
        assert!(is_mmproj_filename("mmproj-Q4_K_M.gguf"));
        assert!(is_mmproj_filename("MMPROJ-F16.GGUF")); // case-insensitive
    }

    #[test]
    fn test_is_mmproj_filename_negative() {
        assert!(!is_mmproj_filename("model-Q4_K_M.gguf"));
        assert!(!is_mmproj_filename("mmproj.bin"));
        assert!(!is_mmproj_filename("model.gguf"));
    }
}
