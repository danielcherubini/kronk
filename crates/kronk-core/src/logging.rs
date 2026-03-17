use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const MAX_LOG_FILES: usize = 5;

/// Get the log file path for a profile.
pub fn log_path(logs_dir: &Path, profile: &str) -> PathBuf {
    logs_dir.join(format!("{}.log", profile))
}

/// Open (or create) a log file for appending. Rotates if over MAX_LOG_SIZE.
pub fn open_log(logs_dir: &Path, profile: &str) -> Result<File> {
    fs::create_dir_all(logs_dir)
        .with_context(|| format!("Failed to create logs directory: {}", logs_dir.display()))?;

    let path = log_path(logs_dir, profile);

    // Rotate if needed
    if path.exists() {
        let meta = fs::metadata(&path)?;
        if meta.len() > MAX_LOG_SIZE {
            rotate_logs(logs_dir, profile)?;
        }
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open log file: {}", path.display()))
}

/// Rotate log files: profile.log -> profile.1.log -> profile.2.log -> ...
fn rotate_logs(logs_dir: &Path, profile: &str) -> Result<()> {
    // Remove oldest
    let oldest = logs_dir.join(format!("{}.{}.log", profile, MAX_LOG_FILES));
    if oldest.exists() {
        fs::remove_file(&oldest)?;
    }

    // Shift existing numbered logs
    for i in (1..MAX_LOG_FILES).rev() {
        let from = logs_dir.join(format!("{}.{}.log", profile, i));
        let to = logs_dir.join(format!("{}.{}.log", profile, i + 1));
        if from.exists() {
            fs::rename(&from, &to)?;
        }
    }

    // Move current to .1
    let current = log_path(logs_dir, profile);
    let first = logs_dir.join(format!("{}.1.log", profile));
    if current.exists() {
        fs::rename(&current, &first)?;
    }

    Ok(())
}

/// Read the last N lines from a log file.
pub fn tail_lines(path: &Path, n: usize) -> Result<Vec<String>> {
    use std::io::{BufRead, BufReader};

    if !path.exists() {
        return Ok(vec![]);
    }

    let file =
        File::open(path).with_context(|| format!("Failed to open log file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let all_lines: Vec<String> = reader.lines().filter_map(|l| l.ok()).collect();

    if all_lines.len() <= n {
        Ok(all_lines)
    } else {
        Ok(all_lines[all_lines.len() - n..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_path() {
        let path = log_path(Path::new("/tmp/logs"), "default");
        assert_eq!(path, PathBuf::from("/tmp/logs/default.log"));
    }

    #[test]
    fn test_open_and_tail() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = open_log(tmp.path(), "test").unwrap();
        writeln!(f, "line 1").unwrap();
        writeln!(f, "line 2").unwrap();
        writeln!(f, "line 3").unwrap();
        drop(f);

        let lines = tail_lines(&log_path(tmp.path(), "test"), 2).unwrap();
        assert_eq!(lines, vec!["line 2", "line 3"]);
    }

    #[test]
    fn test_tail_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = tail_lines(&log_path(tmp.path(), "nonexistent"), 10).unwrap();
        assert!(lines.is_empty());
    }
}
