//! Utility functions for the benchmarks page.

/// Format a Unix timestamp (seconds since epoch) as a local-time
/// "YYYY-MM-DD HH:MM" string using `js_sys::Date`.
///
/// Previously this rebuilt the date manually with
/// `Date::new_with_year_month_day`, which always yields midnight local — the
/// hour/minute fields came out as `00:00` regardless of the input timestamp.
/// We now construct the `Date` from the full ms-since-epoch so `getHours` /
/// `getMinutes` reflect the actual moment the benchmark ran.
///
/// Note: `js_sys::Date::get_month()` returns 0-indexed months (0=Jan), hence
/// the `+1` adjustment below.
pub fn format_timestamp(ts: i64) -> String {
    let ms = wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0);
    let date = js_sys::Date::new(&ms);
    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        date.get_full_year(),
        date.get_month() + 1,
        date.get_date(),
        date.get_hours(),
        date.get_minutes(),
    )
}

/// Format a Unix timestamp as a short relative "time ago" string (e.g. "5m
/// ago", "2h ago", "3d ago"). Falls back to the absolute format for anything
/// older than a week.
pub fn format_relative(ts: i64) -> String {
    let now_ms = js_sys::Date::now();
    let then_ms = ts as f64 * 1000.0;
    let delta_ms = (now_ms - then_ms).max(0.0);
    let secs = (delta_ms / 1000.0) as i64;
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 7 * 86_400 {
        format!("{}d ago", secs / 86_400)
    } else {
        format_timestamp(ts)
    }
}
