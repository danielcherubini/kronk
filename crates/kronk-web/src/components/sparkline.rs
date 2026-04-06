use leptos::prelude::*;

#[component]
#[allow(dead_code)]
pub fn SparklineChart(data: Vec<f32>, max_value: f32, color: String, height: f32) -> impl IntoView {
    // Handle empty data case
    if data.is_empty() {
        return view! {
            <svg viewBox="0 0 100 {height}" class="sparkline" preserveAspectRatio="none"/>
        }
        .into_any();
    }

    // Handle single data point - duplicate to create flat line
    let points = if data.len() == 1 {
        vec![data[0], data[0]]
    } else {
        data
    };

    // Build path data strings
    let mut fill_path = String::new();
    let mut line_path = String::new();

    // Start at first point
    let first_x = 0.0;
    let first_y = height - (points[0] / max_value * height).clamp(0.0, height);
    fill_path.push_str(&format!("M {first_x},{first_y}"));
    line_path.push_str(&format!("M {first_x},{first_y}"));

    // Add remaining points
    for (i, &value) in points.iter().enumerate().skip(1) {
        let x = (i as f32 / (points.len() - 1) as f32) * 100.0;
        let y = height - (value / max_value * height).clamp(0.0, height);
        fill_path.push_str(&format!(" L {x},{y}"));
        line_path.push_str(&format!(" L {x},{y}"));
    }

    // Close the fill path to bottom corners
    fill_path.push_str(" L 100,0 L 0,0 Z");

    view! {
        <svg
            viewBox=format!("0 0 100 {height}")
            width="100%"
            class="sparkline"
            preserveAspectRatio="none"
        >
            {/* Fill area - area under the line */}
            <path d=fill_path stroke="none" fill={color.clone()} fill_opacity="0.15"/>
            {/* Line stroke - the actual data line */}
            <path d=line_path fill="none" stroke={color.clone()} stroke_width="1.5"/>
        </svg>
    }
    .into_any()
}
