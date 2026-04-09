//! Local SHA-256 verification of downloaded GGUF files.
//!
//! HuggingFace tracks every LFS-backed file with a SHA-256 hash (`lfs.sha256`).
//! We store this hash in `model_files.lfs_oid` at pull time. This module re-hashes
//! the on-disk file and compares it to the stored HF hash, detecting corruption,
//! incomplete downloads, or tampering.
//!
//! ## Design
//! - Hashing is CPU-bound and synchronous. Callers that run inside an async context
//!   must wrap these calls in `tokio::task::spawn_blocking`.
//! - The hasher streams the file in 8 MiB chunks and invokes a progress callback
//!   after each chunk, so SSE consumers can show a byte-level progress bar.
//! - Files without an upstream `lfs_oid` (small non-LFS files, or legacy pulls
//!   that didn't capture the hash) cannot be verified and are marked with
//!   `verified_ok = None` + `verify_error = "no upstream hash"`.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::db::queries::{get_model_files, update_verification, ModelFileRecord};

/// Chunk size for streaming reads during hashing.
/// 8 MiB trades off syscall overhead vs. progress-update granularity.
const HASH_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Result of verifying a single file.
#[derive(Debug, Clone)]
pub struct FileVerification {
    pub filename: String,
    /// Hash that was expected (from HF's LFS metadata).
    /// `None` when no upstream hash was stored — cannot be verified.
    pub expected_sha: Option<String>,
    /// Hash computed from the local file.
    /// `None` when the local file was missing or unreadable.
    pub actual_sha: Option<String>,
    /// Overall outcome: Some(true) match, Some(false) mismatch/missing, None n/a.
    pub ok: Option<bool>,
    /// Human-readable detail (mismatch, missing file, no upstream hash, etc.).
    pub error: Option<String>,
}

/// Compute the SHA-256 of a file as a lowercase hex string.
///
/// Blocking: intended to be called from `tokio::task::spawn_blocking` when used
/// from async contexts. Reads the file in 8 MiB chunks and calls `progress` with
/// the cumulative bytes hashed after each chunk.
pub fn sha256_file(path: &Path, mut progress: impl FnMut(u64)) -> Result<String> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open file for hashing: {}", path.display()))?;
    let mut reader = BufReader::with_capacity(HASH_CHUNK_SIZE, file);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_CHUNK_SIZE];
    let mut total: u64 = 0;

    loop {
        let n = reader
            .read(&mut buf)
            .with_context(|| format!("Failed to read file during hashing: {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
        progress(total);
    }

    let digest = hasher.finalize();
    Ok(hex_encode(&digest))
}

/// Verify a single file against its expected HF LFS SHA-256.
///
/// Returns a `FileVerification` describing the outcome:
/// - Expected hash missing → `ok = None`, error = `"no upstream hash"`.
/// - File missing on disk → `ok = Some(false)`, error = `"file not found"`.
/// - Hashes differ → `ok = Some(false)`, error = `"hash mismatch: expected … got …"`.
/// - Hashes match → `ok = Some(true)`.
///
/// Never returns `Err` for verification-level failures (missing file, I/O error,
/// mismatch) — those become `ok = Some(false)` or `None` in the result, so the
/// caller can write a row without aborting an entire verify-all batch. Returns
/// `Err` only for truly unrecoverable errors.
pub fn verify_one(filename: &str, path: &Path, expected_lfs_oid: Option<&str>) -> FileVerification {
    let expected = match expected_lfs_oid {
        Some(e) if !e.is_empty() => e.to_string(),
        _ => {
            return FileVerification {
                filename: filename.to_string(),
                expected_sha: None,
                actual_sha: None,
                ok: None,
                error: Some("no upstream hash".to_string()),
            };
        }
    };

    if !path.exists() {
        return FileVerification {
            filename: filename.to_string(),
            expected_sha: Some(expected),
            actual_sha: None,
            ok: Some(false),
            error: Some(format!("file not found: {}", path.display())),
        };
    }

    match sha256_file(path, |_| {}) {
        Ok(actual) => {
            let ok = actual.eq_ignore_ascii_case(&expected);
            FileVerification {
                filename: filename.to_string(),
                expected_sha: Some(expected.clone()),
                actual_sha: Some(actual.clone()),
                ok: Some(ok),
                error: if ok {
                    None
                } else {
                    Some(format!(
                        "hash mismatch: expected {} got {}",
                        short(&expected),
                        short(&actual)
                    ))
                },
            }
        }
        Err(e) => FileVerification {
            filename: filename.to_string(),
            expected_sha: Some(expected),
            actual_sha: None,
            ok: Some(false),
            error: Some(format!("hash error: {}", e)),
        },
    }
}

/// Verify every tracked file for a repo against its stored LFS hash, writing
/// results to the `model_files` verification columns.
///
/// Blocking (DB + hashing). Wrap in `spawn_blocking` from async callers.
/// Runs files **sequentially** to keep disk I/O predictable on HDDs.
pub fn verify_model(
    conn: &Connection,
    repo_id: &str,
    model_dir: &Path,
) -> Result<Vec<FileVerification>> {
    let records = get_model_files(conn, repo_id)?;
    let mut results = Vec::with_capacity(records.len());

    for rec in records {
        let result = verify_record(&rec, model_dir);
        write_verification(conn, repo_id, &result)?;
        results.push(result);
    }

    Ok(results)
}

/// Verify a single `ModelFileRecord` against its on-disk file in `model_dir`.
pub fn verify_record(rec: &ModelFileRecord, model_dir: &Path) -> FileVerification {
    let path: PathBuf = model_dir.join(&rec.filename);
    verify_one(&rec.filename, &path, rec.lfs_oid.as_deref())
}

/// Persist a verification result into the `model_files` verification columns.
pub fn write_verification(
    conn: &Connection,
    repo_id: &str,
    result: &FileVerification,
) -> Result<()> {
    update_verification(
        conn,
        repo_id,
        &result.filename,
        result.ok,
        result.error.as_deref(),
    )
}

/// Lowercase-hex encode a digest. Avoids pulling in the `hex` crate.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// First 10 characters of a hash, for error messages.
fn short(s: &str) -> String {
    s.chars().take(10).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::queries::upsert_model_file;
    use crate::db::{open_in_memory, OpenResult};
    use std::io::Write;

    /// SHA-256 of the ASCII string "hello" — verified against a reference
    /// implementation so this test is canon.
    const HELLO_SHA256: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    fn write_tmp(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        f.write_all(contents).unwrap();
        p
    }

    /// `sha256_file` computes the SHA-256 of a known-input file and matches
    /// the reference digest for the ASCII string "hello".
    #[test]
    fn test_sha256_file_hello() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "hello.txt", b"hello");
        let mut progress_calls = 0_u32;
        let hash = sha256_file(&path, |_| progress_calls += 1).unwrap();
        assert_eq!(hash, HELLO_SHA256);
        assert!(progress_calls >= 1);
    }

    /// `sha256_file` correctly hashes an empty file (returns SHA-256 of empty string).
    #[test]
    fn test_sha256_file_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "empty.bin", b"");
        let hash = sha256_file(&path, |_| {}).unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// `verify_one` returns `ok = Some(true)` when the expected hash matches
    /// the computed hash of the file contents.
    #[test]
    fn test_verify_one_match() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "hello.txt", b"hello");
        let result = verify_one("hello.txt", &path, Some(HELLO_SHA256));
        assert_eq!(result.ok, Some(true));
        assert!(result.error.is_none());
        assert_eq!(result.actual_sha.as_deref(), Some(HELLO_SHA256));
    }

    /// `verify_one` returns `ok = Some(false)` with a mismatch error when the
    /// expected hash doesn't match the computed hash.
    #[test]
    fn test_verify_one_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "hello.txt", b"hello");
        let result = verify_one("hello.txt", &path, Some("wronghash"));
        assert_eq!(result.ok, Some(false));
        assert!(result.error.as_deref().unwrap().contains("mismatch"));
    }

    /// `verify_one` returns `ok = None` when no upstream hash is stored — the
    /// file cannot be verified, but this is not a failure.
    #[test]
    fn test_verify_one_no_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(tmp.path(), "hello.txt", b"hello");
        let result = verify_one("hello.txt", &path, None);
        assert_eq!(result.ok, None);
        assert_eq!(result.error.as_deref(), Some("no upstream hash"));
    }

    /// `verify_one` returns `ok = Some(false)` when the file is missing from disk.
    #[test]
    fn test_verify_one_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.gguf");
        let result = verify_one("nope.gguf", &missing, Some(HELLO_SHA256));
        assert_eq!(result.ok, Some(false));
        assert!(result.error.as_deref().unwrap().contains("not found"));
    }

    /// `verify_model` loops through stored files, writes verification results to
    /// the DB, and returns one `FileVerification` per tracked file.
    #[test]
    fn test_verify_model_writes_results_to_db() {
        let OpenResult { conn, .. } = open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let repo = "test/repo";

        // File with correct hash
        write_tmp(tmp.path(), "good.gguf", b"hello");
        upsert_model_file(&conn, repo, "good.gguf", None, Some(HELLO_SHA256), Some(5)).unwrap();

        // File with wrong stored hash
        write_tmp(tmp.path(), "bad.gguf", b"hello");
        upsert_model_file(&conn, repo, "bad.gguf", None, Some("deadbeef"), Some(5)).unwrap();

        // File with no upstream hash
        write_tmp(tmp.path(), "unknown.gguf", b"hello");
        upsert_model_file(&conn, repo, "unknown.gguf", None, None, Some(5)).unwrap();

        let results = verify_model(&conn, repo, tmp.path()).unwrap();
        assert_eq!(results.len(), 3);

        // Re-read from DB and assert the verification columns were written.
        let files = get_model_files(&conn, repo).unwrap();
        let good = files.iter().find(|f| f.filename == "good.gguf").unwrap();
        assert_eq!(good.verified_ok, Some(true));
        assert!(good.last_verified_at.is_some());

        let bad = files.iter().find(|f| f.filename == "bad.gguf").unwrap();
        assert_eq!(bad.verified_ok, Some(false));
        assert!(bad.verify_error.as_deref().unwrap().contains("mismatch"));

        let unknown = files.iter().find(|f| f.filename == "unknown.gguf").unwrap();
        assert_eq!(unknown.verified_ok, None);
        assert_eq!(unknown.verify_error.as_deref(), Some("no upstream hash"));
    }
}
