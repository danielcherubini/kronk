use std::collections::HashMap;

use koji_core::models::QuantInfo;

/// Return a naive ISO 8601 UTC timestamp for DB logging.
pub(super) fn manual_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = secs_to_datetime(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        y, mo, d, h, mi, s
    )
}

/// Convert Unix seconds to (year, month, day, hour, min, sec) UTC.
pub(super) fn secs_to_datetime(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let sec = secs % 60;
    let min = (secs / 60) % 60;
    let hour = (secs / 3600) % 24;
    let days = secs / 86400;
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days_in_year = if leap { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days_in_months: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u64;
    for &dim in &days_in_months {
        if remaining < dim {
            break;
        }
        remaining -= dim;
        month += 1;
    }
    (year, month, remaining + 1, hour, min, sec)
}

/// Generate a unique key for a quant entry, avoiding collisions in the map.
/// If `base_key` is already taken, appends the filename stem as a suffix.
pub(super) fn unique_quant_key(
    quants: &HashMap<String, QuantInfo>,
    base_key: &str,
    filename: &str,
) -> String {
    if !quants.contains_key(base_key) {
        return base_key.to_string();
    }
    // Use filename without .gguf extension as a unique fallback
    let stem = filename.strip_suffix(".gguf").unwrap_or(filename);
    let candidate = format!("{}:{}", base_key, stem);
    if !quants.contains_key(&candidate) {
        return candidate;
    }
    // Numeric suffix as last resort
    let mut i = 1;
    loop {
        let key = format!("{}-{}", base_key, i);
        if !quants.contains_key(&key) {
            return key;
        }
        i += 1;
    }
}

/// Format download count with K/M suffix.
pub(super) fn format_downloads(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── secs_to_datetime tests ────────────────────────────────────────────

    #[test]
    fn test_secs_to_datetime_epoch() {
        let (y, mo, d, h, mi, s) = secs_to_datetime(0);
        assert_eq!(y, 1970);
        assert_eq!(mo, 1);
        assert_eq!(d, 1);
        assert_eq!(h, 0);
        assert_eq!(mi, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_secs_to_datetime_midnight() {
        let (y, mo, d, h, mi, s) = secs_to_datetime(86400);
        assert_eq!(y, 1970);
        assert_eq!(mo, 1);
        assert_eq!(d, 2);
        assert_eq!(h, 0);
        assert_eq!(mi, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_secs_to_datetime_leap_year() {
        // Feb 29, 2024 at midnight UTC = 1709164800
        let (y, mo, d, h, mi, s) = secs_to_datetime(1709164800);
        assert_eq!(y, 2024);
        assert_eq!(mo, 2);
        assert_eq!(d, 29);
        assert_eq!(h, 0);
        assert_eq!(mi, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_secs_to_datetime_non_leap_year() {
        // Mar 1, 2023 at midnight UTC (not a leap year)
        let (y, mo, d, h, mi, s) = secs_to_datetime(1677628800);
        assert_eq!(y, 2023);
        assert_eq!(mo, 3);
        assert_eq!(d, 1);
        assert_eq!(h, 0);
        assert_eq!(mi, 0);
        assert_eq!(s, 0);
    }

    #[test]
    fn test_secs_to_datetime_year_boundary() {
        // Jan 1, 2025 at midnight UTC
        let (y, mo, d, _h, _mi, _s) = secs_to_datetime(1735689600);
        assert_eq!(y, 2025);
        assert_eq!(mo, 1);
        assert_eq!(d, 1);
    }

    #[test]
    fn test_secs_to_datetime_with_time() {
        // 14:30:45 on a given day
        let secs = 86400 + (14 * 3600) + (30 * 60) + 45;
        let (_y, _mo, _d, h, mi, s) = secs_to_datetime(secs);
        assert_eq!(h, 14);
        assert_eq!(mi, 30);
        assert_eq!(s, 45);
    }

    // ── unique_quant_key tests ────────────────────────────────────────────

    #[test]
    fn test_unique_quant_key_no_collision() {
        let quants: HashMap<String, QuantInfo> = HashMap::new();
        let key = unique_quant_key(&quants, "Q4_K_M", "model.gguf");
        assert_eq!(key, "Q4_K_M");
    }

    #[test]
    fn test_unique_quant_key_with_filename_stem() {
        let mut quants: HashMap<String, QuantInfo> = HashMap::new();
        quants.insert("Q4_K_M".to_string(), QuantInfo::default());
        let key = unique_quant_key(&quants, "Q4_K_M", "model.gguf");
        assert_eq!(key, "Q4_K_M:model");
    }

    #[test]
    fn test_unique_quant_key_numeric_suffix() {
        let mut quants: HashMap<String, QuantInfo> = HashMap::new();
        quants.insert("Q4_K_M".to_string(), QuantInfo::default());
        quants.insert("Q4_K_M:model".to_string(), QuantInfo::default());
        let key = unique_quant_key(&quants, "Q4_K_M", "model.gguf");
        assert_eq!(key, "Q4_K_M-1");
    }

    #[test]
    fn test_unique_quant_key_no_gguf_suffix() {
        let mut quants: HashMap<String, QuantInfo> = HashMap::new();
        quants.insert("Q4_K_M".to_string(), QuantInfo::default());
        let key = unique_quant_key(&quants, "Q4_K_M", "mmproj");
        assert_eq!(key, "Q4_K_M:mmproj");
    }

    // ── format_downloads tests ────────────────────────────────────────────

    #[test]
    fn test_format_downloads_zero() {
        assert_eq!(format_downloads(0), "0");
    }

    #[test]
    fn test_format_downloads_single() {
        assert_eq!(format_downloads(42), "42");
    }

    #[test]
    fn test_format_downloads_thousands() {
        assert_eq!(format_downloads(1_500), "1.5K");
    }

    #[test]
    fn test_format_downloads_thousands_exact() {
        assert_eq!(format_downloads(1_000), "1.0K");
    }

    #[test]
    fn test_format_downloads_millions() {
        assert_eq!(format_downloads(2_500_000), "2.5M");
    }

    #[test]
    fn test_format_downloads_millions_exact() {
        assert_eq!(format_downloads(1_000_000), "1.0M");
    }

    #[test]
    fn test_format_downloads_boundary_thousands() {
        // Just below 1K threshold
        assert_eq!(format_downloads(999), "999");
        // At 1K threshold
        assert_eq!(format_downloads(1_000), "1.0K");
    }

    #[test]
    fn test_format_downloads_boundary_millions() {
        // Just below 1M threshold (999999 / 1000 = 999.999 → "1000.0K")
        assert_eq!(format_downloads(999_999), "1000.0K");
        // At 1M threshold
        assert_eq!(format_downloads(1_000_000), "1.0M");
    }

    // ── manual_timestamp tests ────────────────────────────────────────────

    #[test]
    fn test_manual_timestamp_format() {
        let ts = manual_timestamp();
        // Should match ISO 8601 format: YYYY-MM-DDTHH:MM:SS.000Z
        assert!(ts.ends_with(".000Z"));
        assert!(ts.contains('T'));
        assert_eq!(ts.len(), 24);
    }

    #[test]
    fn test_manual_timestamp_year() {
        let ts = manual_timestamp();
        let year: u64 = ts[..4].parse().unwrap();
        assert!((2024..=2030).contains(&year));
    }
}
