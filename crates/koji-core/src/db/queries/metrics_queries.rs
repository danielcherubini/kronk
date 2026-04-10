//! System metrics database query functions.

use anyhow::{bail, Result};
use rusqlite::Connection;

/// One sample of system-level metrics, persisted in `system_metrics_history`.
#[derive(Debug, Clone)]
pub struct SystemMetricsRow {
    pub ts_unix_ms: i64,
    pub cpu_usage_pct: f32,
    pub ram_used_mib: i64,
    pub ram_total_mib: i64,
    pub gpu_utilization_pct: Option<i64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub models_loaded: i64,
}

/// Insert one sample and prune anything older than `cutoff_ms` in a single
/// transaction. Both operations succeed or fail together so a crash never
/// leaves the table half-pruned.
pub fn insert_system_metric(
    conn: &Connection,
    row: &SystemMetricsRow,
    cutoff_ms: i64,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO system_metrics_history
             (ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
              gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            row.ts_unix_ms,
            row.cpu_usage_pct as f64,
            row.ram_used_mib,
            row.ram_total_mib,
            row.gpu_utilization_pct,
            row.vram_used_mib,
            row.vram_total_mib,
            row.models_loaded,
        ),
    )?;
    tx.execute(
        "DELETE FROM system_metrics_history WHERE ts_unix_ms < ?1",
        [cutoff_ms],
    )?;
    tx.commit()?;
    Ok(())
}

/// Fetch all samples newer than `since_ms` (exclusive), oldest-first.
pub fn get_system_metrics_since(conn: &Connection, since_ms: i64) -> Result<Vec<SystemMetricsRow>> {
    let mut stmt = conn.prepare(
        "SELECT ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                 gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded
          FROM system_metrics_history
          WHERE ts_unix_ms > ?1
          ORDER BY ts_unix_ms ASC",
    )?;
    let rows = stmt.query_map([since_ms], |row| {
        Ok(SystemMetricsRow {
            ts_unix_ms: row.get(0)?,
            cpu_usage_pct: row.get(1)?,
            ram_used_mib: row.get(2)?,
            ram_total_mib: row.get(3)?,
            gpu_utilization_pct: row.get(4)?,
            vram_used_mib: row.get(5)?,
            vram_total_mib: row.get(6)?,
            models_loaded: row.get(7)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Fetch the most recent `limit` samples, oldest-first.
pub fn get_recent_system_metrics(conn: &Connection, limit: i64) -> Result<Vec<SystemMetricsRow>> {
    if limit < 0 {
        bail!("limit must be >= 0");
    }
    let mut stmt = conn.prepare(
        "SELECT ts_unix_ms, cpu_usage_pct, ram_used_mib, ram_total_mib,
                 gpu_utilization_pct, vram_used_mib, vram_total_mib, models_loaded
          FROM system_metrics_history
          ORDER BY ts_unix_ms DESC
          LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| {
        Ok(SystemMetricsRow {
            ts_unix_ms: row.get(0)?,
            cpu_usage_pct: row.get(1)?,
            ram_used_mib: row.get(2)?,
            ram_total_mib: row.get(3)?,
            gpu_utilization_pct: row.get(4)?,
            vram_used_mib: row.get(5)?,
            vram_total_mib: row.get(6)?,
            models_loaded: row.get(7)?,
        })
    })?;
    let mut rows: Vec<SystemMetricsRow> = rows.collect::<rusqlite::Result<_>>()?;
    rows.reverse(); // reverse to return oldest-first
    Ok(rows)
}
