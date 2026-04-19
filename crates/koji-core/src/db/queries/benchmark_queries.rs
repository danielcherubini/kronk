//! Benchmark history database query functions.

use anyhow::Result;
use rusqlite::{Connection, params};

/// Row from the benchmarks table.
#[derive(Debug, Clone)]
pub struct BenchmarkRow {
    pub id: i64,
    pub created_at: i64,
    pub model_id: String,
    pub display_name: Option<String>,
    pub quant: Option<String>,
    pub backend: String,
    pub engine: String,
    pub pp_sizes: String,   // JSON array string
    pub tg_sizes: String,   // JSON array string
    pub threads: Option<String>,  // JSON array string or null
    pub ngl_range: Option<String>,
    pub runs: u32,
    pub warmup: u32,
    pub results: String,    // JSON array string
    pub load_time_ms: Option<f64>,
    pub vram_used_mib: Option<i64>,
    pub vram_total_mib: Option<i64>,
    pub duration_seconds: f64,
    pub status: String,
}

/// Insert a benchmark result row. Returns the new row id.
pub fn insert_benchmark(
    conn: &Connection,
    model_id: &str,
    display_name: Option<&str>,
    quant: Option<&str>,
    backend: &str,
    engine: &str,
    pp_sizes_json: &str,
    tg_sizes_json: &str,
    threads_json: Option<&str>,
    ngl_range: Option<&str>,
    runs: u32,
    warmup: u32,
    results_json: &str,
    load_time_ms: Option<f64>,
    vram_used_mib: Option<i64>,
    vram_total_mib: Option<i64>,
    duration_seconds: f64,
    status: &str,
) -> Result<i64> {
    let tx = conn.unchecked_transaction()?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    tx.execute(
        "INSERT INTO benchmarks (
            created_at, model_id, display_name, quant, backend, engine,
            pp_sizes, tg_sizes, threads, ngl_range, runs, warmup,
            results, load_time_ms, vram_used_mib, vram_total_mib,
            duration_seconds, status
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            created_at,
            model_id,
            display_name,
            quant,
            backend,
            engine,
            pp_sizes_json,
            tg_sizes_json,
            threads_json,
            ngl_range,
            runs as i64,
            warmup as i64,
            results_json,
            load_time_ms,
            vram_used_mib,
            vram_total_mib,
            duration_seconds,
            status,
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// Fetch all benchmark entries ordered by created_at DESC.
pub fn list_benchmarks(conn: &Connection) -> Result<Vec<BenchmarkRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, created_at, model_id, display_name, quant, backend, engine,
                pp_sizes, tg_sizes, threads, ngl_range, runs, warmup,
                results, load_time_ms, vram_used_mib, vram_total_mib,
                duration_seconds, status
         FROM benchmarks
         ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(BenchmarkRow {
            id: row.get(0)?,
            created_at: row.get(1)?,
            model_id: row.get(2)?,
            display_name: row.get(3)?,
            quant: row.get(4)?,
            backend: row.get(5)?,
            engine: row.get(6)?,
            pp_sizes: row.get(7)?,
            tg_sizes: row.get(8)?,
            threads: row.get(9)?,
            ngl_range: row.get(10)?,
            runs: row.get::<_, i64>(11)? as u32,
            warmup: row.get::<_, i64>(12)? as u32,
            results: row.get(13)?,
            load_time_ms: row.get(14)?,
            vram_used_mib: row.get(15)?,
            vram_total_mib: row.get(16)?,
            duration_seconds: row.get(17)?,
            status: row.get(18)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Delete a benchmark entry by id.
pub fn delete_benchmark(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM benchmarks WHERE id = ?1", [id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create an in-memory SQLite connection with the benchmarks table.
    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE benchmarks (
                id                  INTEGER PRIMARY KEY AUTOINCREMENT,
                created_at          INTEGER NOT NULL,
                model_id            TEXT NOT NULL,
                display_name        TEXT,
                quant               TEXT,
                backend             TEXT NOT NULL,
                engine              TEXT NOT NULL DEFAULT 'llama_bench',
                pp_sizes            TEXT NOT NULL,
                tg_sizes            TEXT NOT NULL,
                threads             TEXT,
                ngl_range           TEXT,
                runs                INTEGER NOT NULL DEFAULT 3,
                warmup              INTEGER NOT NULL DEFAULT 1,
                results             TEXT NOT NULL,
                load_time_ms        REAL,
                vram_used_mib       INTEGER,
                vram_total_mib      INTEGER,
                duration_seconds    REAL,
                status              TEXT NOT NULL DEFAULT 'success'
            )",
        )
        .unwrap();
        conn
    }

    /// Helper to create test benchmark parameters.
    fn make_benchmark(
        model_id: &str,
        backend: &str,
        pp_sizes: &str,
        tg_sizes: &str,
        results: &str,
    ) -> (String, Option<String>, Option<String>, String, String, String, String, Option<String>, Option<String>, u32, u32, String, Option<f64>, Option<i64>, Option<i64>, f64, String) {
        (
            model_id.to_string(),
            Some("Test Model".to_string()),
            Some("Q4_K_M".to_string()),
            backend.to_string(),
            "llama_bench".to_string(),
            pp_sizes.to_string(),
            tg_sizes.to_string(),
            Some("[4,8]".to_string()),
            None,
            3,
            1,
            results.to_string(),
            Some(1500.0),
            Some(4096),
            Some(8192),
            30.5,
            "success".to_string(),
        )
    }

    #[test]
    fn test_insert_benchmark_returns_id() {
        let conn = test_conn();
        let (model_id, display_name, quant, backend, engine, pp_sizes, tg_sizes, threads, ngl_range, runs, warmup, results, load_time_ms, vram_used, vram_total, duration, status) =
            make_benchmark("qwen7b", "llama_cpp", "[512,1024]", "[128,256]", "[{\"pp\":100}]");

        let id = insert_benchmark(
            &conn,
            &model_id,
            display_name.as_deref(),
            quant.as_deref(),
            &backend,
            &engine,
            &pp_sizes,
            &tg_sizes,
            threads.as_deref(),
            ngl_range.as_deref(),
            runs,
            warmup,
            &results,
            load_time_ms,
            vram_used,
            vram_total,
            duration,
            &status,
        )
        .unwrap();

        assert_eq!(id, 1);
    }

    #[test]
    fn test_list_benchmarks_empty() {
        let conn = test_conn();
        let benchmarks = list_benchmarks(&conn).unwrap();
        assert!(benchmarks.is_empty());
    }

    #[test]
    fn test_list_benchmarks_returns_inserted() {
        let conn = test_conn();
        let (model_id, display_name, quant, backend, engine, pp_sizes, tg_sizes, threads, ngl_range, runs, warmup, results, load_time_ms, vram_used, vram_total, duration, status) =
            make_benchmark("qwen7b", "llama_cpp", "[512,1024]", "[128,256]", "[{}]");

        insert_benchmark(
            &conn,
            &model_id,
            display_name.as_deref(),
            quant.as_deref(),
            &backend,
            &engine,
            &pp_sizes,
            &tg_sizes,
            threads.as_deref(),
            ngl_range.as_deref(),
            runs,
            warmup,
            &results,
            load_time_ms,
            vram_used,
            vram_total,
            duration,
            &status,
        )
        .unwrap();

        let benchmarks = list_benchmarks(&conn).unwrap();
        assert_eq!(benchmarks.len(), 1);
        assert_eq!(benchmarks[0].model_id, "qwen7b");
        assert_eq!(benchmarks[0].backend, "llama_cpp");
        assert_eq!(benchmarks[0].display_name, Some("Test Model".to_string()));
    }

    #[test]
    fn test_delete_benchmark() {
        let conn = test_conn();
        let (model_id, display_name, quant, backend, engine, pp_sizes, tg_sizes, threads, ngl_range, runs, warmup, results, load_time_ms, vram_used, vram_total, duration, status) =
            make_benchmark("qwen7b", "llama_cpp", "[512]", "[128]", "[{}]");

        let id = insert_benchmark(
            &conn,
            &model_id,
            display_name.as_deref(),
            quant.as_deref(),
            &backend,
            &engine,
            &pp_sizes,
            &tg_sizes,
            threads.as_deref(),
            ngl_range.as_deref(),
            runs,
            warmup,
            &results,
            load_time_ms,
            vram_used,
            vram_total,
            duration,
            &status,
        )
        .unwrap();

        delete_benchmark(&conn, id).unwrap();

        let benchmarks = list_benchmarks(&conn).unwrap();
        assert!(benchmarks.is_empty());
    }

    #[test]
    fn test_list_benchmarks_ordered_desc() {
        let conn = test_conn();
        // Insert multiple benchmarks with explicit timestamps to control order
        conn.execute_batch(
            "INSERT INTO benchmarks (created_at, model_id, backend, pp_sizes, tg_sizes, results, duration_seconds, status)
             VALUES (1000, 'model_a', 'llama_cpp', '[512]', '[128]', '[{}]', 10.0, 'success');",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO benchmarks (created_at, model_id, backend, pp_sizes, tg_sizes, results, duration_seconds, status)
             VALUES (3000, 'model_c', 'llama_cpp', '[512]', '[128]', '[{}]', 10.0, 'success');",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO benchmarks (created_at, model_id, backend, pp_sizes, tg_sizes, results, duration_seconds, status)
             VALUES (2000, 'model_b', 'llama_cpp', '[512]', '[128]', '[{}]', 10.0, 'success');",
        )
        .unwrap();

        let benchmarks = list_benchmarks(&conn).unwrap();
        assert_eq!(benchmarks.len(), 3);
        assert_eq!(benchmarks[0].model_id, "model_c"); // created_at=3000
        assert_eq!(benchmarks[1].model_id, "model_b"); // created_at=2000
        assert_eq!(benchmarks[2].model_id, "model_a"); // created_at=1000
    }

    #[test]
    fn test_insert_benchmark_with_nulls() {
        let conn = test_conn();
        let (model_id, _, _, backend, engine, pp_sizes, tg_sizes, _, _, runs, warmup, results, _, _, _, duration, status) =
            make_benchmark("qwen7b", "llama_cpp", "[512]", "[128]", "[{}]");

        // Insert with None for optional fields
        let id = insert_benchmark(
            &conn,
            &model_id,
            None,          // display_name
            None,          // quant
            &backend,
            &engine,
            &pp_sizes,
            &tg_sizes,
            None,          // threads
            None,          // ngl_range
            runs,
            warmup,
            &results,
            None,          // load_time_ms
            None,          // vram_used
            None,          // vram_total
            duration,
            &status,
        )
        .unwrap();

        assert_eq!(id, 1);

        let benchmarks = list_benchmarks(&conn).unwrap();
        assert_eq!(benchmarks.len(), 1);
        assert!(benchmarks[0].display_name.is_none());
        assert!(benchmarks[0].quant.is_none());
    }
}
