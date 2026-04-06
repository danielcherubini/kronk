//! Logs command handler
//!
//! Handles `koji logs <server>` for viewing server logs.

use anyhow::Result;
use koji_core::config::Config;
use koji_core::logging;
use std::io::BufRead;

/// View server logs
pub async fn cmd_logs(config: &Config, name: &str, follow: bool, lines: usize) -> Result<()> {
    let logs_dir = config.logs_dir()?;
    let log_path = logging::log_path(&logs_dir, name);

    if !log_path.exists() {
        println!("No logs found for '{}'.", name);
        println!();
        println!("Logs are created when running as a service.");
        println!("Install the service: koji service install");
        return Ok(());
    }

    // Print last N lines
    let tail = logging::tail_lines(&log_path, lines)?;
    for line in &tail {
        println!("{}", line);
    }

    if follow {
        // Poll for new content
        let mut file = std::fs::File::open(&log_path)?;
        use std::io::Seek;
        file.seek(std::io::SeekFrom::End(0))?;
        let mut reader = std::io::BufReader::new(file);
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));

        loop {
            tick.tick().await;
            let mut line = String::new();
            while reader.read_line(&mut line)? > 0 {
                print!("{}", line);
                line.clear();
            }
        }
    }

    Ok(())
}
