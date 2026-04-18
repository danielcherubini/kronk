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

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory SQLite connection with the system_metrics_history table.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE system_metrics_history (
                ts_unix_ms INTEGER PRIMARY KEY,
                cpu_usage_pct REAL NOT NULL,
                ram_used_mib INTEGER NOT NULL,
                ram_total_mib INTEGER NOT NULL,
                gpu_utilization_pct INTEGER,
                vram_used_mib INTEGER,
                vram_total_mib INTEGER,
                models_loaded INTEGER NOT NULL
            )",
        )
        .unwrap();
        conn
    }

    /// Helper to create a test metrics row.
    fn make_row(ts: i64, cpu: f32, ram: i64) -> SystemMetricsRow {
        SystemMetricsRow {
            ts_unix_ms: ts,
            cpu_usage_pct: cpu,
            ram_used_mib: ram,
            ram_total_mib: 16384,
            gpu_utilization_pct: Some(75),
            vram_used_mib: Some(8192),
            vram_total_mib: Some(16384),
            models_loaded: 1,
        }
    }

    #[test]
    fn test_insert_system_metric() {
        let conn = test_conn();
        let row = make_row(1000, 45.5, 8192);
        insert_system_metric(&conn, &row, 0).unwrap();

        let metrics = get_system_metrics_since(&conn, 0).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].ts_unix_ms, 1000);
        assert!((metrics[0].cpu_usage_pct - 45.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_insert_system_metric_with_cutoff() {
        let conn = test_conn();
        // Insert an old row
        insert_system_metric(&conn, &make_row(100, 10.0, 1000), 500).unwrap();
        // Insert a new row
        insert_system_metric(&conn, &make_row(1000, 45.5, 8192), 500).unwrap();

        // Old row should have been pruned
        let metrics = get_system_metrics_since(&conn, 0).unwrap();
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].ts_unix_ms, 1000);
    }

    #[test]
    fn test_get_system_metrics_since_empty() {
        let conn = test_conn();
        let metrics = get_system_metrics_since(&conn, 0).unwrap();
        assert!(metrics.is_empty());
    }

    #[test]
    fn test_get_system_metrics_since_filter() {
        let conn = test_conn();
        insert_system_metric(&conn, &make_row(100, 10.0, 1000), 0).unwrap();
        insert_system_metric(&conn, &make_row(200, 20.0, 2000), 0).unwrap();
        insert_system_metric(&conn, &make_row(300, 30.0, 3000), 0).unwrap();

        let metrics = get_system_metrics_since(&conn, 150).unwrap();
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].ts_unix_ms, 200);
        assert_eq!(metrics[1].ts_unix_ms, 300);
    }

    #[test]
    fn test_get_system_metrics_since_ordered() {
        let conn = test_conn();
        insert_system_metric(&conn, &make_row(300, 30.0, 3000), 0).unwrap();
        insert_system_metric(&conn, &make_row(100, 10.0, 1000), 0).unwrap();
        insert_system_metric(&conn, &make_row(200, 20.0, 2000), 0).unwrap();

        let metrics = get_system_metrics_since(&conn, 0).unwrap();
        assert_eq!(metrics[0].ts_unix_ms, 100);
        assert_eq!(metrics[1].ts_unix_ms, 200);
        assert_eq!(metrics[2].ts_unix_ms, 300);
    }

    #[test]
    fn test_get_recent_system_metrics_empty() {
        let conn = test_conn();
        let metrics = get_recent_system_metrics(&conn, 10).unwrap();
        assert!(metrics.is_empty());
    }

    #[test]
    fn test_get_recent_system_metrics_limit() {
        let conn = test_conn();
        for i in 1..=5 {
            insert_system_metric(&conn, &make_row(i * 100, i as f32 * 10.0, i * 1000), 0).unwrap();
        }

        let metrics = get_recent_system_metrics(&conn, 3).unwrap();
        assert_eq!(metrics.len(), 3);
        // Should return the 3 most recent, oldest-first
        assert_eq!(metrics[0].ts_unix_ms, 300);
        assert_eq!(metrics[1].ts_unix_ms, 400);
        assert_eq!(metrics[2].ts_unix_ms, 500);
    }

    #[test]
    fn test_get_recent_system_metrics_ordered() {
        let conn = test_conn();
        insert_system_metric(&conn, &make_row(300, 30.0, 3000), 0).unwrap();
        insert_system_metric(&conn, &make_row(100, 10.0, 1000), 0).unwrap();
        insert_system_metric(&conn, &make_row(200, 20.0, 2000), 0).unwrap();

        let metrics = get_recent_system_metrics(&conn, 10).unwrap();
        // Should be ordered oldest-first
        assert_eq!(metrics[0].ts_unix_ms, 100);
        assert_eq!(metrics[1].ts_unix_ms, 200);
        assert_eq!(metrics[2].ts_unix_ms, 300);
    }

    #[test]
    fn test_get_recent_system_metrics_zero_limit() {
        let conn = test_conn();
        insert_system_metric(&conn, &make_row(100, 10.0, 1000), 0).unwrap();

        let metrics = get_recent_system_metrics(&conn, 0).unwrap();
        assert!(metrics.is_empty());
    }

    #[test]
    fn test_get_recent_system_metrics_negative_limit_error() {
        let conn = test_conn();
        let result = get_recent_system_metrics(&conn, -1);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("limit must be >= 0"));
    }

    #[test]
    fn test_system_metrics_row_with_null_gpu() {
        let conn = test_conn();
        let row = SystemMetricsRow {
            ts_unix_ms: 1000,
            cpu_usage_pct: 50.0,
            ram_used_mib: 8192,
            ram_total_mib: 16384,
            gpu_utilization_pct: None,
            vram_used_mib: None,
            vram_total_mib: None,
            models_loaded: 0,
        };
        insert_system_metric(&conn, &row, 0).unwrap();

        let metrics = get_system_metrics_since(&conn, 0).unwrap();
        assert_eq!(metrics.len(), 1);
        assert!(metrics[0].gpu_utilization_pct.is_none());
        assert!(metrics[0].vram_used_mib.is_none());
    }
}
