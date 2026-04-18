use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use tracing_subscriber::{fmt, EnvFilter};

const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const MAX_LOG_FILES: usize = 5;

pub fn init() {
    if let Err(e) = fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .try_init()
    {
        tracing::debug!("Tracing subscriber already initialized: {}", e);
    }
}

/// Initialize tracing to write to a file in addition to stdout.
///
/// Opens `logs_dir/koji.log` and configures the global tracing subscriber
/// to write there. Rotates the log if it exceeds MAX_LOG_SIZE.
pub fn init_with_file(logs_dir: &Path) -> Result<()> {
    use std::sync::{Arc, Mutex};

    let log_file = open_log(logs_dir, "koji")?;

    // Create a multi-writer that writes to both stdout and the file
    let multi_writer = MultiWriter {
        stdout: Arc::new(Mutex::new(std::io::stdout())),
        file: Arc::new(Mutex::new(log_file)),
    };

    let subscriber = fmt()
        .with_writer(Mutex::new(multi_writer))
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_ansi(false) // No ANSI codes in log file
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .context("Failed to set global tracing subscriber")?;

    Ok(())
}

/// A writer that writes to multiple destinations.
struct MultiWriter {
    stdout: std::sync::Arc<std::sync::Mutex<std::io::Stdout>>,
    file: std::sync::Arc<std::sync::Mutex<File>>,
}

impl std::io::Write for MultiWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Write to stdout
        if let Ok(mut out) = self.stdout.lock() {
            let _ = out.write_all(buf);
        }

        // Write to file
        if let Ok(mut f) = self.file.lock() {
            f.write_all(buf)?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if let Ok(mut out) = self.stdout.lock() {
            let _ = out.flush();
        }
        if let Ok(mut f) = self.file.lock() {
            f.flush()?;
        }
        Ok(())
    }
}

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
    let all_lines: Vec<String> = reader.lines().collect::<Result<Vec<String>, _>>()?;

    if all_lines.len() <= n {
        Ok(all_lines)
    } else {
        Ok(all_lines[all_lines.len() - n..].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_log_path() {
        let path = log_path(Path::new("/tmp/logs"), "default");
        assert_eq!(path, PathBuf::from("/tmp/logs/default.log"));
    }

    #[test]
    fn test_log_path_with_special_profile() {
        let path = log_path(Path::new("/tmp/logs"), "profile-with-dashes");
        assert_eq!(path, PathBuf::from("/tmp/logs/profile-with-dashes.log"));
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

    #[test]
    fn test_tail_more_lines_than_requested() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = open_log(tmp.path(), "test").unwrap();
        for i in 1..=10 {
            writeln!(f, "line {}", i).unwrap();
        }
        drop(f);

        // Request only 3 lines from a 10-line file
        let lines = tail_lines(&log_path(tmp.path(), "test"), 3).unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "line 8");
        assert_eq!(lines[1], "line 9");
        assert_eq!(lines[2], "line 10");
    }

    #[test]
    fn test_tail_fewer_lines_than_requested() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = open_log(tmp.path(), "test").unwrap();
        writeln!(f, "line 1").unwrap();
        writeln!(f, "line 2").unwrap();
        drop(f);

        // Request 10 lines from a 2-line file
        let lines = tail_lines(&log_path(tmp.path(), "test"), 10).unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[1], "line 2");
    }

    #[test]
    fn test_tail_zero_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = open_log(tmp.path(), "test").unwrap();
        writeln!(f, "line 1").unwrap();
        drop(f);

        let lines = tail_lines(&log_path(tmp.path(), "test"), 0).unwrap();
        assert!(lines.is_empty());
    }

    #[test]
    fn test_max_log_size_constant() {
        // Verify the max log size is 10 MB
        assert_eq!(MAX_LOG_SIZE, 10 * 1024 * 1024);
    }

    #[test]
    fn test_max_log_files_constant() {
        // Verify the max number of log files
        assert_eq!(MAX_LOG_FILES, 5);
    }
}
