use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// A hover point on the sparkline, tracking the nearest data index.
#[derive(Clone, Debug)]
struct HoverPoint {
    /// X position in the viewBox coordinate system (0–100).
    x_pct: f32,
    /// Y position in the viewBox coordinate system.
    y_pct: f32,
    /// The data value at the hovered point.
    value: f32,
    /// Index in the data array.
    #[allow(dead_code)]
    index: usize,
    /// Timestamp for this data point (Unix ms), if available.
    ts_unix_ms: Option<i64>,
}

/// Format a relative time string from a Unix ms timestamp.
///
/// Returns a human-readable string like "2m 15s ago" or "45s ago"
/// based on the difference between `ts_unix_ms` and the current
/// browser time. Returns an empty string if the timestamp is 0.
fn format_relative_time(ts_unix_ms: i64) -> String {
    if ts_unix_ms == 0 {
        return String::new();
    }
    let now_ms = js_sys::Date::now() as i64;
    let diff_ms = now_ms - ts_unix_ms;
    if diff_ms < 0 {
        return String::new();
    }
    let secs = diff_ms / 1_000;
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3_600 {
        let mins = secs / 60;
        let remain_secs = secs % 60;
        if remain_secs == 0 {
            format!("{}m ago", mins)
        } else {
            format!("{}m {}s ago", mins, remain_secs)
        }
    } else {
        let hours = secs / 3_600;
        format!("{}h ago", hours)
    }
}

/// Format a duration given in seconds into a short label like "-3m" or "-1h".
fn format_duration_label(secs: i64) -> String {
    if secs < 60 {
        format!("-{}s", secs)
    } else if secs < 3_600 {
        format!("-{}m", secs / 60)
    } else {
        format!("-{}h", secs / 3_600)
    }
}

/// A responsive SVG area chart component for displaying time-series data.
///
/// Renders a filled area chart with a stroke line, interactive hover overlay,
/// Y-axis reference lines, and time axis labels. Suitable for system metrics
/// like CPU usage, memory, GPU utilization, and VRAM.
///
/// ## Parameters
///
/// * `data` — Sample values to plot. Each value is expected to be in
///   the range `[0, max_value]`. Empty vectors render Y-axis refs only.
/// * `max_value` — Maximum expected Y-axis value. Must be > 0; defaults to 1.0.
/// * `color` — CSS color string for the chart (e.g. `"var(--accent-green)"`).
/// * `height` — SVG height in pixels. Recommended range: 30–150.
/// * `timestamps` — Optional Unix ms timestamps for each data point. If provided
///   and matching `data.len()`, enables time axis labels and relative-time tooltips.
/// * `unit_label` — Unit string shown in tooltip (e.g. `"%"`, `"MiB"`).
/// * `y_refs` — Y-axis reference values to draw as subtle dashed lines
///   (e.g. `vec![0.0, 100.0]` for percentages, `vec![max_value]` for memory).
#[component]
pub fn SparklineChart(
    data: Vec<f32>,
    max_value: f32,
    color: String,
    height: f32,
    #[prop(default = Vec::new())] timestamps: Vec<i64>,
    #[prop(default = String::new())] unit_label: String,
    #[prop(default = Vec::new())] y_refs: Vec<f32>,
) -> impl IntoView {
    let hover = RwSignal::new(None::<HoverPoint>);

    // Guard against division by zero
    let safe_max = if max_value > 0.0 { max_value } else { 1.0 };

    // Handle empty data — render only Y-axis refs
    let has_data = !data.is_empty();
    let timestamps_valid = !timestamps.is_empty() && timestamps.len() == data.len();

    // Store unit_label and color in signals so closures can read them reactively
    let unit_label_signal = RwSignal::new(unit_label);
    let color_signal = RwSignal::new(color);

    // Build path data strings (only if data exists)
    let (fill_path, line_path) = if has_data {
        // Duplicate single data point to create a flat line
        let points = if data.len() == 1 {
            vec![data[0], data[0]]
        } else {
            data.clone()
        };

        let mut fill = String::new();
        let mut line = String::new();

        let first_y = height - (points[0] / safe_max * height).clamp(0.0, height);
        fill.push_str(&format!("M 0,{first_y}"));
        line.push_str(&format!("M 0,{first_y}"));

        for (i, &value) in points.iter().enumerate().skip(1) {
            let x = (i as f32 / (points.len() - 1) as f32) * 100.0;
            let y = height - (value / safe_max * height).clamp(0.0, height);
            fill.push_str(&format!(" L {x},{y}"));
            line.push_str(&format!(" L {x},{y}"));
        }

        fill.push_str(&format!(" L 100,{height} L 0,{height} Z"));

        (fill, line)
    } else {
        (String::new(), String::new())
    };

    // Compute time axis labels
    let (left_label, right_label) = if timestamps_valid && !timestamps.is_empty() {
        let oldest_ts = *timestamps.first().unwrap();
        let now_ms = js_sys::Date::now() as i64;
        let diff_secs = (now_ms - oldest_ts) / 1_000;
        let left = format_duration_label(diff_secs.max(0));
        let right = "now".to_string();
        (left, right)
    } else {
        (String::new(), String::new())
    };

    // Helper to compute hover state from mouse event
    let on_mouse_move = move |ev: leptos::ev::MouseEvent| {
        if !has_data || data.is_empty() {
            hover.set(None);
            return;
        }

        // Get the SVG element's bounding rect to calculate relative position
        let target = ev.target().unwrap();
        let svg_el: web_sys::SvgsvgElement = match target.dyn_into() {
            Ok(el) => el,
            Err(_) => {
                hover.set(None);
                return;
            }
        };

        let rect = svg_el.get_bounding_client_rect();
        let svg_width = rect.width();
        let svg_height = rect.height();

        if svg_width <= 0.0 || svg_height <= 0.0 {
            hover.set(None);
            return;
        }

        // Compute mouse position relative to SVG
        let mouse_x = ev.client_x() as f64 - rect.left();
        let x_pct = ((mouse_x / svg_width) * 100.0) as f32;

        // Clamp to chart bounds
        let x_pct = x_pct.clamp(0.0, 100.0);

        // Find nearest data index
        let data_len = if data.len() == 1 { 2 } else { data.len() };
        let raw_index = (x_pct / 100.0 * (data_len as f32 - 1.0)).round() as usize;
        let index = raw_index.clamp(0, data.len() - 1);

        // Get the actual data value
        let value = data[index];

        // Compute Y position
        let y_pct = height - (value / safe_max * height).clamp(0.0, height);

        // Get timestamp for this point if available
        let ts = if timestamps_valid && index < timestamps.len() {
            Some(timestamps[index])
        } else {
            None
        };

        // Recompute x_pct from the index for precise positioning
        let precise_x = if data_len > 1 {
            (index as f32 / (data_len as f32 - 1.0)) * 100.0
        } else {
            50.0
        };

        hover.set(Some(HoverPoint {
            x_pct: precise_x,
            y_pct,
            value,
            index,
            ts_unix_ms: ts,
        }));
    };

    let on_mouse_leave = move |_ev: leptos::ev::MouseEvent| {
        hover.set(None);
    };

    // Render Y-axis reference lines
    let y_ref_lines = y_refs
        .iter()
        .map(|&ref_val| {
            let y = height - (ref_val / safe_max * height).clamp(0.0, height);
            view! {
                <line x1="0" y1=y x2="100" y2=y stroke="rgba(255,255,255,0.1)" stroke-dasharray="2,2" stroke-width="0.5"/>
            }
        })
        .collect::<Vec<_>>();

    // Render hover overlay elements (SVG elements: vertical line + dot)
    let hover_overlay = move || {
        hover.get().map(|hp| {
            let c1 = color_signal.get();
            let c2 = c1.clone();
            view! {
                // Vertical indicator line
                <line
                    x1=hp.x_pct
                    y1="0"
                    x2=hp.x_pct
                    y2=height
                    stroke=c1
                    stroke-opacity="0.4"
                    stroke-dasharray="4,2"
                    stroke-width="0.8"
                />
                // Highlighted dot on the data line
                <circle
                    cx=hp.x_pct
                    cy=hp.y_pct
                    r="2"
                    fill=c2
                    stroke="var(--bg-secondary)"
                    stroke-width="1"
                />
            }
        })
    };

    // Tooltip HTML element rendered outside SVG, positioned absolutely
    let tooltip_html = move || {
        hover.get().map(|hp| {
            let unit = unit_label_signal.get();
            let tooltip_value = format!(
                "{:.1}{}",
                hp.value,
                if unit.is_empty() { "" } else { &unit }
            );
            let tooltip_time = hp.ts_unix_ms.map(format_relative_time).unwrap_or_default();
            let left_style = format!("left: {}%;", hp.x_pct);

            view! {
                <div class="sparkline-tooltip" style=left_style>
                    <span class="sparkline-tooltip-value">{tooltip_value}</span>
                    {if tooltip_time.is_empty() {
                        ().into_any()
                    } else {
                        view! {
                            <span class="sparkline-tooltip-time">{tooltip_time}</span>
                        }.into_any()
                    }}
                </div>
            }
        })
    };

    // Data path fill and stroke colors (read from signal for reactivity)
    let fill_color = color_signal.get();
    let stroke_color = color_signal.get();

    view! {
        <div class="sparkline-container">
            <svg
                viewBox=format!("0 0 100 {height}")
                width="100%"
                class="sparkline"
                preserveAspectRatio="none"
                on:mousemove=on_mouse_move
                on:mouseleave=on_mouse_leave
            >
                // Y-axis reference lines
                {y_ref_lines}

                // Data fill and stroke paths
                {if has_data {
                    view! {
                        <path d=fill_path stroke="none" fill=fill_color fill-opacity="0.15"/>
                        <path d=line_path fill="none" stroke=stroke_color stroke-width="1.5"/>
                    }.into_any()
                } else {
                    ().into_any()
                }}

                // Hover overlay (vertical line + dot inside SVG)
                {hover_overlay}
            </svg>
            // Tooltip HTML element positioned absolutely above the SVG
            {tooltip_html}
            // Time axis labels
            {if !left_label.is_empty() || !right_label.is_empty() {
                view! {
                    <div class="sparkline-time-axis">
                        <span>{left_label}</span>
                        <span>{right_label}</span>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
}
