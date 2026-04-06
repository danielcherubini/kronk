use leptos::prelude::*;

/// A responsive SVG area chart component for displaying time-series data.
///
/// Renders a filled area chart with a stroke line, suitable for system metrics
/// like CPU usage, memory, GPU utilization, and VRAM. The chart automatically
/// scales to fill its container width and maintains a fixed aspect ratio.
///
/// ## Parameters
///
/// * `data`: Slice of sample values to plot. Each value is expected to be in
///   the range `[0, max_value]`. Empty vectors are handled gracefully (renders
///   an empty SVG). NaN values will produce invalid SVG paths.
/// * `max_value`: The maximum expected Y-axis value. Used to normalize data.
///   Must be greater than 0.0; if 0.0, defaults to 1.0 internally.
/// * `color`: CSS color string for the chart (e.g., `"var(--accent-green)"`,
///   `"#34d399"`, `"green"`). Used for both fill and stroke.
/// * `height`: SVG height in pixels. Recommended range: 30–150.
///
/// ## Returns
///
/// An `IntoView` that renders an SVG element containing:
/// - A filled area path (15% opacity) representing the area under the data line
/// - A stroke path (1.5px width) tracing the data points
///
/// ## Example
///
/// ```rust,ignore
/// <SparklineChart
///     data=vec![10.0, 25.0, 30.0, 20.0, 45.0]
///     max_value=100.0
///     color="var(--accent-green)"
///     height=60.0
/// />
/// ```
#[component]
pub fn SparklineChart(data: Vec<f32>, max_value: f32, color: String, height: f32) -> impl IntoView {
    // Handle empty data case
    if data.is_empty() {
        return view! {
            <svg viewBox=format!("0 0 100 {height}") class="sparkline" preserveAspectRatio="none"/>
        }
        .into_any();
    }

    // Handle single data point - duplicate to create flat line
    let points = if data.len() == 1 {
        vec![data[0], data[0]]
    } else {
        data
    };

    // Guard against division by zero if max_value is 0
    let safe_max = if max_value > 0.0 { max_value } else { 1.0 };

    // Build path data strings
    let mut fill_path = String::new();
    let mut line_path = String::new();

    // Start at first point
    let first_x = 0.0;
    let first_y = height - (points[0] / safe_max * height).clamp(0.0, height);
    fill_path.push_str(&format!("M {first_x},{first_y}"));
    line_path.push_str(&format!("M {first_x},{first_y}"));

    // Add remaining points
    for (i, &value) in points.iter().enumerate().skip(1) {
        let x = (i as f32 / (points.len() - 1) as f32) * 100.0;
        let y = height - (value / safe_max * height).clamp(0.0, height);
        fill_path.push_str(&format!(" L {x},{y}"));
        line_path.push_str(&format!(" L {x},{y}"));
    }

    // Close the fill path to bottom corners
    fill_path.push_str(&format!(" L 100,{height} L 0,{height} Z"));

    view! {
        <svg
            viewBox=format!("0 0 100 {height}")
            width="100%"
            class="sparkline"
            preserveAspectRatio="none"
        >
            {/* Fill area - area under the line */}
            <path d=fill_path stroke="none" fill={color.clone()} fill-opacity="0.15"/>
            {/* Line stroke - the actual data line */}
            <path d=line_path fill="none" stroke={color.clone()} stroke-width="1.5"/>
        </svg>
    }
    .into_any()
}
