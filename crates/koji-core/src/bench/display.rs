use crate::bench::BenchReport;

/// Format a statistic as "mean ± stddev" or just "mean" if stddev is 0.0
pub fn format_stat(mean: f64, stddev: f64) -> String {
    if stddev == 0.0 {
        format!("{:.1}", mean)
    } else {
        format!("{:.1} ± {:.1}", mean, stddev)
    }
}

/// Print a formatted bench report to stdout
pub fn print_bench_report(report: &BenchReport) {
    // Header
    let quant_str = report.model_info.quant.as_deref().unwrap_or("");
    let model_id_str = report.model_info.model_id.as_deref().unwrap_or("");
    let name = &report.model_info.name;

    println!(
        "koji bench — {}{}{} via {}",
        name,
        if !model_id_str.is_empty() {
            format!(" ({})", model_id_str)
        } else {
            String::new()
        },
        if !quant_str.is_empty() {
            format!(" ({})", quant_str)
        } else {
            String::new()
        },
        report.model_info.backend
    );
    println!(
        "GPU: {} | Context: {} | Runs: {} | Warmup: {}",
        report.model_info.gpu_type,
        report
            .model_info
            .context_length
            .map(|c| c.to_string())
            .unwrap_or_else(|| "N/A".to_string()),
        report.config.runs,
        report.config.warmup
    );
    println!("───────────────────────────────────────────────────────────────────");

    // Results table
    println!(" Test         │ PP (t/s)        │ TG (t/s)        │ TTFT (ms)  │ Total (ms)");
    println!(" ─────────────┼─────────────────┼─────────────────┼────────────┼────────────");

    for summary in &report.summaries {
        let test_name = &summary.test_name;
        let pp_str = format_stat(summary.pp_mean, summary.pp_stddev);
        let tg_str = format_stat(summary.tg_mean, summary.tg_stddev);
        let ttft_str = format_stat(summary.ttft_mean, summary.ttft_stddev);
        let total_str = format_stat(summary.total_mean, summary.total_stddev);

        println!(
            " {:13} │ {:17} │ {:17} │ {:12} │ {:12}",
            test_name, pp_str, tg_str, ttft_str, total_str
        );
    }

    println!(" ────────────────────────────────────────────────────────────────────");
    println!(" Model load time: {:.0} ms", report.load_time_ms);

    if let Some(vram) = &report.vram {
        println!(" VRAM: {} / {} MiB", vram.used_mib, vram.total_mib);
    }
}
