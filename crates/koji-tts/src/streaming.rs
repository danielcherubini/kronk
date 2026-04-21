//! Streaming utilities for TTS audio output.

use base64::Engine;

use crate::config::AudioChunk;

/// Convert an AudioChunk to an SSE-formatted string.
///
/// Format: `event: audio\ndata: <base64>\n\n` (or with `event: end` for final chunks).
pub fn audio_chunk_to_sse(chunk: &AudioChunk) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(&chunk.data);
    if chunk.is_final {
        format!("event: audio\ndata: {}\n\nevent: end\n\n", encoded)
    } else {
        format!("event: audio\ndata: {}\n\n", encoded)
    }
}

/// Convert an error to an SSE-formatted error string.
pub fn error_chunk_to_sse(error: &anyhow::Error) -> String {
    format!("event: error\ndata: {}\n\n", error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioChunk;

    /// Test that audio_chunk_to_sse formats non-final chunks correctly.
    #[test]
    fn test_audio_chunk_to_sse_non_final() {
        let chunk = AudioChunk {
            data: vec![0x01, 0x02],
            is_final: false,
        };
        let sse = audio_chunk_to_sse(&chunk);
        assert!(sse.starts_with("event: audio\n"));
        assert!(sse.contains("data: "));
        assert!(!sse.contains("event: end"));
    }

    /// Test that audio_chunk_to_sse formats final chunks with 'end' event.
    #[test]
    fn test_audio_chunk_to_sse_final() {
        let chunk = AudioChunk {
            data: vec![0x03, 0x04],
            is_final: true,
        };
        let sse = audio_chunk_to_sse(&chunk);
        assert!(sse.starts_with("event: audio\n"));
        assert!(sse.contains("event: end\n"));
    }

    /// Test that error_chunk_to_sse formats correctly.
    #[test]
    fn test_error_chunk_to_sse() {
        let err = anyhow::anyhow!("test error message");
        let sse = error_chunk_to_sse(&err);
        assert!(sse.starts_with("event: error\n"));
        assert!(sse.contains("data: "));
        assert!(sse.contains("test error message"));
    }

    /// Test that SSE formatting includes base64 encoding.
    #[test]
    fn test_sse_includes_base64() {
        let chunk = AudioChunk {
            data: vec![0xFF, 0xFE, 0xFD],
            is_final: true,
        };
        let sse = audio_chunk_to_sse(&chunk);
        // Base64 of [0xFF, 0xFE, 0xFD] should be "//79"
        assert!(sse.contains("//79"));
    }
}
