//! Utility functions for the benchmarks page.

/// Format a Unix timestamp to "YYYY-MM-DD HH:MM" using js_sys (WASM-compatible).
pub fn format_timestamp(ts: i64) -> String {
    // Compute day offset from Unix timestamp (seconds since epoch)
    let secs = ts as u64;
    let days_since_epoch = (secs / 60 / 60 / 24) as i64;

    // Compute year, month, day from days since Unix epoch (1970-01-01)
    let mut days = days_since_epoch;
    let mut year: i64 = 1970;
    loop {
        let ydays = if is_leap_year(year) { 366i64 } else { 365i64 };
        if days < ydays {
            break;
        }
        days -= ydays;
        year += 1;
    }
    let leap = is_leap_year(year);
    let month_lengths: [i32; 12] = match leap {
        true => [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
        false => [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31],
    };
    let mut month_idx: i32 = 0;
    for (i, &ml) in month_lengths.iter().enumerate() {
        if days < ml as i64 {
            month_idx = i as i32;
            break;
        }
        days -= ml as i64;
    }
    let day = (days + 1) as i32;

    // Verify with js_sys Date to handle timezone correctly
    let date = js_sys::Date::new_with_year_month_day(year as u32, month_idx, day);
    let month = date.get_month() + 1;
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        month,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    )
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
