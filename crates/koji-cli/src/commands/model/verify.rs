use anyhow::{anyhow, Context, Result};
use koji_core::config::Config;
use koji_core::db::{Connection, OpenResult};
use koji_core::models::ModelRegistry;

/// Result of verifying a single model's files.
#[derive(Debug)]
pub struct VerificationResult {
    /// Total number of files checked.
    pub total_files: usize,
    /// Number of files that passed verification (hash match).
    pub passed: usize,
    /// Number of files that failed verification (mismatch or missing).
    pub failed: usize,
    /// Number of files that could not be verified (no upstream hash).
    pub unverifiable: usize,
}

/// Verify all tracked files for a single model in the database.
///
/// This function is extracted from `cmd_verify` so it can be unit-tested
/// independently of I/O and CLI concerns. It iterates over the model's
/// tracked files, verifies each one against its stored LFS hash, and
/// accumulates pass/fail/unverifiable counts.
pub fn verify_files(
    conn: &Connection,
    model_id: i64,
    model_dir: &std::path::Path,
) -> Result<VerificationResult> {
    use koji_core::models::verify;

    let results = verify::verify_model(conn, model_id, "", model_dir)?;

    if results.is_empty() {
        return Ok(VerificationResult {
            total_files: 0,
            passed: 0,
            failed: 0,
            unverifiable: 0,
        });
    }

    let mut total_files: usize = 0;
    let mut total_ok: usize = 0;
    let mut total_unknown: usize = 0;
    let mut total_bad: usize = 0;

    for r in &results {
        total_files += 1;
        match r.ok {
            Some(true) => total_ok += 1,
            Some(false) => total_bad += 1,
            None => total_unknown += 1,
        }
    }

    Ok(VerificationResult {
        total_files,
        passed: total_ok,
        failed: total_bad,
        unverifiable: total_unknown,
    })
}

pub(super) async fn cmd_verify(config: &Config, model_filter: Option<String>) -> Result<()> {
    use koji_core::models::verify;

    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult {
        conn,
        needs_backfill: _,
    } = koji_core::db::open(&db_dir)?;

    let models_dir = config.models_dir()?;
    let configs_dir = config.configs_dir()?;
    let registry = ModelRegistry::new(models_dir.to_path_buf(), configs_dir.to_path_buf());

    let models: Vec<koji_core::models::InstalledModel> = match model_filter {
        Some(ref id) => {
            let found = registry
                .find(id)?
                .with_context(|| format!("Model '{}' not found.", id))?;
            vec![found]
        }
        None => registry.scan()?,
    };

    if models.is_empty() {
        println!("No installed models found.");
        return Ok(());
    }

    println!("Verifying {} model(s)...", models.len());
    println!();

    let mut any_failed = false;
    let mut total_files: usize = 0;
    let mut total_ok: usize = 0;
    let mut total_unknown: usize = 0;
    let mut total_bad: usize = 0;
    let mut hard_errors: Vec<String> = Vec::new();

    for model in &models {
        // Mirror cmd_rm: legacy/hand-edited cards may have an empty
        // card.model.source, in which case fall back to the model id so we
        // still hit the right rows in the model_files table.
        let repo_id: &str = if model.card.model.source.is_empty() {
            &model.id
        } else {
            &model.card.model.source
        };
        // Use the registry-resolved directory from the InstalledModel itself
        // rather than reconstructing the path — legacy/hand-edited cards may
        // live under a directory that doesn't match `models_dir/repo_id`.
        let model_dir = &model.dir;
        println!("{}", repo_id);

        let model_id = koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)
            .ok()
            .flatten()
            .map(|r| r.id);

        let results = match model_id {
            Some(id) => match verify::verify_model(&conn, id, repo_id, model_dir) {
                Ok(r) => r,
                Err(e) => {
                    println!("  verify error: {}", e);
                    any_failed = true;
                    hard_errors.push(format!("{}: {}", repo_id, e));
                    continue;
                }
            },
            None => {
                println!("  (no DB entry — skipping verification)");
                continue;
            }
        };

        if results.is_empty() {
            println!("  (no files tracked — run `koji model update --refresh` first)");
            continue;
        }

        for r in &results {
            total_files += 1;
            let (icon, label) = match r.ok {
                Some(true) => {
                    total_ok += 1;
                    ("✓", "ok".to_string())
                }
                Some(false) => {
                    total_bad += 1;
                    any_failed = true;
                    (
                        "✗",
                        r.error.clone().unwrap_or_else(|| "mismatch".to_string()),
                    )
                }
                None => {
                    total_unknown += 1;
                    (
                        "—",
                        r.error
                            .clone()
                            .unwrap_or_else(|| "no upstream hash".to_string()),
                    )
                }
            };
            println!("  {} {}  {}", icon, r.filename, label);
        }
        println!();
    }

    println!(
        "Summary: {} file(s) total — {} ok, {} failed, {} unverifiable.",
        total_files, total_ok, total_bad, total_unknown
    );

    let error_msg = if !hard_errors.is_empty() && total_bad == 0 {
        // Hard errors occurred but no file-level failures — report the hard errors.
        anyhow!(
            "Verification failed with errors: {}",
            hard_errors.join("; ")
        )
    } else if any_failed {
        let mut parts = vec![format!("{} files failed", total_bad)];
        if !hard_errors.is_empty() {
            parts.push(format!("({} hard error(s))", hard_errors.len()));
        }
        anyhow!("Verification failed: {}", parts.join(", "))
    } else {
        return Ok(());
    };
    Err(error_msg)
}

pub(super) async fn cmd_verify_existing(
    config: &Config,
    model_filter: Option<String>,
    verbose: bool,
) -> Result<()> {
    use koji_core::models::verify;

    let db_dir = koji_core::config::Config::config_dir()?;
    let OpenResult {
        conn,
        needs_backfill: _,
    } = koji_core::db::open(&db_dir)?;

    let models_dir = config.models_dir()?;

    // Load model configs from DB
    let model_configs = koji_core::db::load_model_configs(&conn)?;

    // Collect unique HF repo IDs from DB.
    // Entries without a `model` field (raw-args entries) are skipped.
    let mut repo_ids: Vec<String> = model_configs
        .values()
        .filter_map(|mc| mc.model.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    repo_ids.sort();

    let repo_ids: Vec<String> = match model_filter {
        Some(ref id) => {
            if repo_ids.contains(id) {
                vec![id.clone()]
            } else {
                anyhow::bail!("Model '{}' not found in config.", id);
            }
        }
        None => repo_ids,
    };

    // Warn about any entries that have no `model` field
    let skipped: Vec<&str> = model_configs
        .iter()
        .filter(|(_, mc)| mc.model.is_none())
        .map(|(name, _)| name.as_str())
        .collect();
    for name in &skipped {
        println!(
            "Skipping '{}': no HuggingFace repo ID in config (raw-args entry).",
            name
        );
    }

    if repo_ids.is_empty() {
        println!("No models with a HuggingFace repo ID found in config.");
        return Ok(());
    }

    println!(
        "Verifying {} model(s) and backfilling missing hashes...",
        repo_ids.len()
    );
    println!();

    let mut any_failed = false;
    let mut total_files: usize = 0;
    let mut total_ok: usize = 0;
    let mut total_unknown: usize = 0;
    let mut total_bad: usize = 0;
    let mut total_backfilled: usize = 0;
    let mut hard_errors: Vec<String> = Vec::new();

    for repo_id in &repo_ids {
        let repo_id: &str = repo_id.as_str();
        let model_dir = koji_core::models::repo_path(&models_dir, repo_id);

        println!("Model: {}", repo_id);

        // Look up model_id
        let model_id = match koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)? {
            Some(r) => r.id,
            None => {
                println!("  (no DB entry)");
                continue;
            }
        };

        // Check if any files need hash backfilling
        let records = match koji_core::db::queries::get_model_files(&conn, model_id) {
            Ok(r) => r,
            Err(e) => {
                println!("  Error reading database: {}", e);
                any_failed = true;
                hard_errors.push(format!("{}: {}", repo_id, e));
                continue;
            }
        };

        if records.is_empty() {
            println!(
                "  (no files tracked — run `koji model pull {}` first)",
                repo_id
            );
            println!();
            continue;
        }

        let needs_backfill = records.iter().any(|r| r.lfs_oid.is_none());

        if needs_backfill {
            // Count how many records need backfilling before we fetch
            let records_needing_backfill = records.iter().filter(|r| r.lfs_oid.is_none()).count();

            if verbose {
                println!(
                    "  Fetching metadata from HuggingFace to backfill {} missing hash(es)...",
                    records_needing_backfill
                );
            }

            // Always refresh metadata when needed, regardless of verbose flag
            match koji_core::models::update::refresh_metadata(&conn, &models_dir, repo_id).await {
                Ok(_) => {
                    // Re-fetch records to see how many were successfully backfilled
                    let updated_records =
                        match koji_core::db::queries::get_model_files(&conn, model_id) {
                            Ok(r) => r,
                            Err(e) => {
                                println!("  Error reading database: {}", e);
                                any_failed = true;
                                hard_errors.push(format!("{}: {}", repo_id, e));
                                continue;
                            }
                        };
                    // Count how many still need backfilling after the refresh
                    let still_needing_backfill = updated_records
                        .iter()
                        .filter(|r| r.lfs_oid.is_none())
                        .count();
                    // The difference is how many were successfully backfilled
                    let backfilled_count =
                        records_needing_backfill.saturating_sub(still_needing_backfill);
                    if verbose {
                        println!("  Backfilled {} missing hash(es)", backfilled_count);
                    }
                    total_backfilled += backfilled_count;
                }
                Err(e) => {
                    if verbose {
                        println!(
                            "  Warning: Failed to fetch metadata: {}. Proceeding with verification; files without hashes will be marked as unverifiable.",
                            e
                        );
                    }
                }
            }
        }

        let model_id = koji_core::db::queries::get_model_config_by_repo_id(&conn, repo_id)
            .ok()
            .flatten()
            .map(|r| r.id);

        let results = match model_id {
            Some(id) => match verify::verify_model(&conn, id, repo_id, &model_dir) {
                Ok(r) => r,
                Err(e) => {
                    println!("  verify error: {}", e);
                    any_failed = true;
                    hard_errors.push(format!("{}: {}", repo_id, e));
                    continue;
                }
            },
            None => {
                continue;
            }
        };
        for r in &results {
            total_files += 1;
            let (icon, label) = match r.ok {
                Some(true) => {
                    total_ok += 1;
                    if verbose {
                        (
                            "✓",
                            format!(
                                "ok ({}...)",
                                r.expected_sha
                                    .as_deref()
                                    .unwrap_or("unknown")
                                    .chars()
                                    .take(10)
                                    .collect::<String>()
                            ),
                        )
                    } else {
                        ("✓", "ok".to_string())
                    }
                }
                Some(false) => {
                    total_bad += 1;
                    any_failed = true;
                    if verbose {
                        (
                            "✗",
                            r.error.clone().unwrap_or_else(|| "mismatch".to_string()),
                        )
                    } else {
                        ("✗", "failed".to_string())
                    }
                }
                None => {
                    total_unknown += 1;
                    if verbose {
                        (
                            "—",
                            r.error
                                .clone()
                                .unwrap_or_else(|| "no upstream hash".to_string()),
                        )
                    } else {
                        ("—", "unverifiable".to_string())
                    }
                }
            };
            if verbose {
                println!("  {} {}  {}", icon, r.filename, label);
            }
        }
        println!();
    }

    // Build summary
    let mut summary_parts: Vec<String> = Vec::new();
    summary_parts.push(format!("{} file(s) total", total_files));
    summary_parts.push(format!("{} verified OK", total_ok));
    if total_bad > 0 {
        summary_parts.push(format!("{} failed", total_bad));
    }
    if total_unknown > 0 {
        summary_parts.push(format!("{} unverifiable", total_unknown));
    }
    if total_backfilled > 0 {
        summary_parts.push(format!("{} hashes backfilled", total_backfilled));
    }

    println!("Summary: {}", summary_parts.join(", "));
    println!();

    let error_msg = if !hard_errors.is_empty() && total_bad == 0 {
        anyhow!(
            "Verification failed with errors: {}",
            hard_errors.join("; ")
        )
    } else if any_failed {
        let mut parts = vec![format!("{} files failed", total_bad)];
        if !hard_errors.is_empty() {
            parts.push(format!("({} hard error(s))", hard_errors.len()));
        }
        anyhow!("Verification failed: {}", parts.join(", "))
    } else {
        return Ok(());
    };
    Err(error_msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Test that verify_files returns correct counts for a model with mixed results.
    #[test]
    fn test_verify_files_counts() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // Set up a model with 3 files: one good, one bad hash, one no upstream
        let (model_id, _repo_id) = setup_test_model_with_files(&conn, &tmp, "test/repo");

        let result = verify_files(&conn, model_id, tmp.path()).unwrap();
        assert_eq!(result.total_files, 3);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 1);
        assert_eq!(result.unverifiable, 1);
    }

    /// Test that verify_files returns empty counts when model has no tracked files.
    #[test]
    fn test_verify_files_no_files() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();
        let mc = koji_core::config::ModelConfig::default();
        let config_key = "empty--repo".to_string();
        let model_id = koji_core::db::save_model_config(&conn, &config_key, &mc).unwrap();

        let result = verify_files(&conn, model_id, std::path::Path::new("")).unwrap();
        assert_eq!(result.total_files, 0);
        assert_eq!(result.passed, 0);
        assert_eq!(result.failed, 0);
        assert_eq!(result.unverifiable, 0);
    }

    /// Test that verify_files returns all-pass when all hashes match.
    #[test]
    fn test_verify_files_all_pass() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // Set up a model with 3 files: good, bad, unknown
        let (model_id, _repo_id) = setup_test_model_with_files(&conn, &tmp, "test/allpass");

        // Override: only keep the good file in DB
        {
            use koji_core::db::queries::delete_model_file;
            delete_model_file(&conn, model_id, "bad.gguf").ok();
            delete_model_file(&conn, model_id, "unknown.gguf").ok();
        }

        let result = verify_files(&conn, model_id, tmp.path()).unwrap();
        assert_eq!(result.total_files, 1);
        assert_eq!(result.passed, 1);
        assert_eq!(result.failed, 0);
        assert_eq!(result.unverifiable, 0);
    }

    /// Test that verify_files returns correct counts after removing the good file.
    #[test]
    fn test_verify_files_all_fail() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // Set up a model with 3 files: good (pass), bad (fail), unknown (unverifiable)
        let (model_id, _repo_id) = setup_test_model_with_files(&conn, &tmp, "test/allfail");

        // Override: remove the good file from DB — leaves bad (fail) + unknown (unverifiable)
        {
            use koji_core::db::queries::delete_model_file;
            delete_model_file(&conn, model_id, "good.gguf").ok();
        }

        let result = verify_files(&conn, model_id, tmp.path()).unwrap();
        assert_eq!(result.total_files, 2);
        assert_eq!(result.passed, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.unverifiable, 1);
    }

    /// Test that verify_files handles missing files correctly.
    #[test]
    fn test_verify_files_missing_file() {
        let OpenResult { conn, .. } = koji_core::db::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();

        // Create a model with a file tracked in DB but missing on disk
        let mc = koji_core::config::ModelConfig::default();
        let config_key = "test--missing".to_string();
        let model_id = koji_core::db::save_model_config(&conn, &config_key, &mc).unwrap();

        let expected_hash = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        koji_core::db::queries::upsert_model_file(
            &conn,
            model_id,
            "test/missing",
            "missing.gguf",
            None,
            Some(expected_hash),
            Some(5),
        )
        .unwrap();

        let result = verify_files(&conn, model_id, tmp.path()).unwrap();
        assert_eq!(result.total_files, 1);
        assert_eq!(result.passed, 0);
        assert_eq!(result.failed, 1);
        assert_eq!(result.unverifiable, 0);
    }

    /// Create a model with 3 files: one correct hash, one wrong hash, one no upstream.
    /// Returns (model_id, repo_id).
    fn setup_test_model_with_files(
        conn: &Connection,
        tmp: &tempfile::TempDir,
        repo_id: &str,
    ) -> (i64, String) {
        const HELLO_SHA256: &str =
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

        let mc = koji_core::config::ModelConfig::default();
        let config_key = repo_id.to_lowercase().replace('/', "--");
        let model_id = koji_core::db::save_model_config(conn, &config_key, &mc).unwrap();

        // File with correct hash ("hello")
        {
            let path = tmp.path().join("good.gguf");
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello").unwrap();
            drop(f);
            koji_core::db::queries::upsert_model_file(
                conn,
                model_id,
                repo_id,
                "good.gguf",
                None,
                Some(HELLO_SHA256),
                Some(5),
            )
            .unwrap();
        }

        // File with wrong hash — same content but wrong expected hash
        {
            let path = tmp.path().join("bad.gguf");
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello").unwrap();
            drop(f);
            koji_core::db::queries::upsert_model_file(
                conn,
                model_id,
                repo_id,
                "bad.gguf",
                None,
                Some("deadbeef1234567890abcdef"),
                Some(5),
            )
            .unwrap();
        }

        // File with no upstream hash
        {
            let path = tmp.path().join("unknown.gguf");
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"hello").unwrap();
            drop(f);
            koji_core::db::queries::upsert_model_file(
                conn,
                model_id,
                repo_id,
                "unknown.gguf",
                None,
                None,
                Some(5),
            )
            .unwrap();
        }

        (model_id, repo_id.to_string())
    }
}
