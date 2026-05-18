use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// Metadata extracted from a GGUF file header.
/// Only reads the header (~100KB), never loads tensor data.
#[derive(Debug, Clone, Default)]
pub struct GgufMetadata {
    pub architecture: Option<String>, // general.architecture (e.g. "llama")
    pub context_length: Option<u64>,  // {arch}.context_length
    pub embedding_length: Option<u64>, // {arch}.embedding_length
    pub block_count: Option<u64>,     // {arch}.block_count
    pub head_count: Option<u64>,      // {arch}.attention.head_count
    pub quantization: Option<String>, // from file_type mapping (e.g. "Q4_K_M")
    pub name: Option<String>,         // general.name
}

/// Parse GGUF metadata from a file on disk.
///
/// Returns `Err` only if the file cannot be read or is not a valid GGUF file.
/// Individual missing metadata keys are handled gracefully (fields are `None`).
pub fn parse_gguf_metadata(path: &Path) -> Result<GgufMetadata> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open GGUF file: {}", path.display()))?;
    let mut reader = BufReader::new(file);

    let gguf = gguf_parser::GgufFile::parse(&mut reader)
        .with_context(|| format!("Failed to parse GGUF header: {}", path.display()))?;

    Ok(GgufMetadata {
        architecture: gguf.architecture().map(|s| s.to_string()),
        context_length: gguf.context_length(),
        embedding_length: gguf.embedding_length(),
        block_count: gguf.block_count(),
        head_count: gguf.head_count(),
        quantization: gguf.quantization_name().map(|s| s.to_string()),
        name: gguf.name().map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_invalid_path() {
        let result = parse_gguf_metadata(Path::new("/nonexistent/file.gguf"));
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_non_gguf_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "this is not a GGUF file").unwrap();
        let result = parse_gguf_metadata(tmp.path());
        assert!(result.is_err());
    }
}
