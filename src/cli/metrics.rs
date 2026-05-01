//! Metrics Export CLI Command
//!
//! Provides command-line tools for exporting server metrics in Prometheus or JSON format.

use crate::error::PdsResult;
use prometheus::{Encoder, TextEncoder};
use serde_json::json;
use std::fs;

/// Export metrics in Prometheus or JSON format
pub fn export_metrics(format: &str, output: Option<&str>) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Metrics Export");
    println!("════════════════════════════════════════════════════════");

    let metric_families = prometheus::gather();
    println!("Collected {} metric families", metric_families.len());

    // Generate output based on format
    let content = match format.to_lowercase().as_str() {
        "prometheus" | "prom" => {
            println!("Format: Prometheus");
            export_prometheus(&metric_families)?
        }
        "json" => {
            println!("Format: JSON");
            export_json(&metric_families)?
        }
        _ => {
            return Err(crate::error::PdsError::Validation(format!(
                "Unknown format: {}. Use 'prometheus' or 'json'",
                format
            )));
        }
    };

    println!("Generated {} bytes of metrics data", content.len());

    // Output to file or stdout
    if let Some(output_path) = output {
        println!("\n📤 Writing to file: {}", output_path);
        fs::write(output_path, &content).map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to write output file: {}", e))
        })?;
        println!("✓ File written successfully");
    } else {
        println!("\n════════════════════════════════════════════════════════");
        println!("  Exported Metrics");
        println!("════════════════════════════════════════════════════════\n");
        println!("{}", content);
    }

    println!("\n════════════════════════════════════════════════════════");
    println!("✅ Metrics export completed successfully");
    println!("════════════════════════════════════════════════════════\n");

    Ok(())
}

/// Export metrics in Prometheus text format
fn export_prometheus(metric_families: &[prometheus::proto::MetricFamily]) -> PdsResult<String> {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    encoder.encode(metric_families, &mut buffer).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to encode metrics: {}", e))
    })?;
    String::from_utf8(buffer).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to convert metrics to string: {}", e))
    })
}

/// Export metrics in JSON format
fn export_json(metric_families: &[prometheus::proto::MetricFamily]) -> PdsResult<String> {
    let mut metrics = Vec::new();

    for mf in metric_families {
        let metric_type = format!("{:?}", mf.get_field_type());

        for m in mf.get_metric() {
            let mut labels = std::collections::HashMap::new();
            for label in m.get_label() {
                labels.insert(label.name().to_string(), label.value().to_string());
            }

            // prometheus 0.14 dropped the `has_counter()` / `has_gauge()` /
            // ... methods on `Metric` — the fields are public `MessageField`s
            // (effectively `Option<Box<T>>`), so we probe `.is_some()` directly.
            let value = if m.counter.is_some() {
                m.get_counter().value()
            } else if m.gauge.is_some() {
                m.get_gauge().value()
            } else if m.histogram.is_some() {
                m.get_histogram().sample_count() as f64
            } else if m.summary.is_some() {
                m.get_summary().sample_count() as f64
            } else {
                0.0
            };

            metrics.push(json!({
                "name": mf.name(),
                "help": mf.help(),
                "type": metric_type,
                "labels": labels,
                "value": value,
            }));
        }
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    serde_json::to_string_pretty(&json!({
        "metrics": metrics,
        "timestamp": timestamp,
        "count": metrics.len(),
    }))
    .map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to serialize metrics to JSON: {}", e))
    })
}
