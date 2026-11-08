//! The node's metrics, rendered in the Prometheus text format at
//! `/metrics`. Every crate records into one process-wide registry; each
//! metric is declared once in [`DEFS`] with its kind and help text.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Counter,
    Gauge,
    Histogram,
}

pub struct Def {
    pub name: &'static str,
    pub kind: Kind,
    pub help: &'static str,
}

/// Every metric the node records.
pub const DEFS: &[Def] = &[
    Def {
        name: "locus_http_requests_total",
        kind: Kind::Counter,
        help: "XRPC requests, by method NSID and status.",
    },
    Def {
        name: "locus_http_request_seconds",
        kind: Kind::Histogram,
        help: "XRPC request latency, by method NSID.",
    },
    Def {
        name: "locus_repo_commit_seconds",
        kind: Kind::Histogram,
        help: "Time to build, sign and store a repository commit.",
    },
    Def {
        name: "locus_repo_verify_seconds",
        kind: Kind::Histogram,
        help: "Time to verify an imported repository's tree.",
    },
    Def {
        name: "locus_sequencer_lag_seconds",
        kind: Kind::Gauge,
        help: "Age of the oldest event waiting to be sequenced.",
    },
    Def {
        name: "locus_sequencer_events_total",
        kind: Kind::Counter,
        help: "Events sequenced into the firehose log.",
    },
    Def {
        name: "locus_firehose_subscribers",
        kind: Kind::Gauge,
        help: "Open firehose subscriptions on this node.",
    },
    Def {
        name: "locus_firehose_dropped_total",
        kind: Kind::Counter,
        help: "Firehose subscribers dropped for falling behind.",
    },
    Def {
        name: "locus_job_runs_total",
        kind: Kind::Counter,
        help: "Background job runs, by job and outcome.",
    },
    Def {
        name: "locus_rate_limit_hits_total",
        kind: Kind::Counter,
        help: "Requests refused by a rate limit, by bucket.",
    },
    Def {
        name: "locus_blob_bytes_total",
        kind: Kind::Counter,
        help: "Blob bytes uploaded.",
    },
    Def {
        name: "locus_db_pool_connections",
        kind: Kind::Gauge,
        help: "Database pool connections, by state.",
    },
    Def {
        name: "locus_kryphocron_encodes_total",
        kind: Kind::Counter,
        help: "Records encoded for an audience.",
    },
    Def {
        name: "locus_scanner_calls_total",
        kind: Kind::Counter,
        help: "Media scanner calls, by result.",
    },
    Def {
        name: "locus_gated_fetches_total",
        kind: Kind::Counter,
        help: "Gated media fetches, by outcome.",
    },
];

/// Histogram bucket bounds, in seconds.
const BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

/// Series per metric beyond which new label sets are folded into one
/// `other` series (labels can come from requests).
const MAX_SERIES: usize = 2000;

enum Value {
    Num(f64),
    Hist {
        counts: Vec<u64>,
        sum: f64,
        count: u64,
    },
}

type Series = BTreeMap<String, Value>;

fn registry() -> &'static Mutex<BTreeMap<&'static str, Series>> {
    static R: OnceLock<Mutex<BTreeMap<&'static str, Series>>> = OnceLock::new();
    R.get_or_init(Mutex::default)
}

fn label_string(labels: &[(&str, &str)]) -> String {
    let mut s = String::new();
    for (i, (k, v)) in labels.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let v = v
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n");
        let _ = write!(s, "{k}=\"{v}\"");
    }
    s
}

fn with_series(name: &'static str, labels: &[(&str, &str)], f: impl FnOnce(&mut Series, String)) {
    let mut reg = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let series = reg.entry(name).or_default();
    let mut key = label_string(labels);
    if !series.contains_key(&key) && series.len() >= MAX_SERIES {
        key = label_string(
            &labels
                .iter()
                .map(|(k, _)| (*k, "other"))
                .collect::<Vec<_>>(),
        );
    }
    f(series, key);
}

/// Adds `by` to a counter.
pub fn inc(name: &'static str, labels: &[(&str, &str)], by: u64) {
    #[allow(clippy::cast_precision_loss)]
    let by = by as f64;
    with_series(name, labels, |series, key| {
        if let Value::Num(n) = series.entry(key).or_insert(Value::Num(0.0)) {
            *n += by;
        }
    });
}

/// Sets a gauge.
pub fn set(name: &'static str, labels: &[(&str, &str)], v: f64) {
    with_series(name, labels, |series, key| {
        series.insert(key, Value::Num(v));
    });
}

/// Adds `by` (possibly negative) to a gauge.
pub fn add(name: &'static str, labels: &[(&str, &str)], by: f64) {
    with_series(name, labels, |series, key| {
        if let Value::Num(n) = series.entry(key).or_insert(Value::Num(0.0)) {
            *n += by;
        }
    });
}

/// Records one observation (seconds) in a histogram.
pub fn observe(name: &'static str, labels: &[(&str, &str)], secs: f64) {
    with_series(name, labels, |series, key| {
        let v = series.entry(key).or_insert_with(|| Value::Hist {
            counts: vec![0; BUCKETS.len()],
            sum: 0.0,
            count: 0,
        });
        if let Value::Hist { counts, sum, count } = v {
            for (i, b) in BUCKETS.iter().enumerate() {
                if secs <= *b {
                    counts[i] += 1;
                }
            }
            *sum += secs;
            *count += 1;
        }
    });
}

/// Times a block into a histogram.
pub struct Timer {
    name: &'static str,
    start: std::time::Instant,
}

impl Timer {
    #[must_use]
    pub fn start(name: &'static str) -> Self {
        Self {
            name,
            start: std::time::Instant::now(),
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        observe(self.name, &[], self.start.elapsed().as_secs_f64());
    }
}

fn series_name(name: &str, suffix: &str, labels: &str, extra: Option<(&str, &str)>) -> String {
    let mut l = labels.to_owned();
    if let Some((k, v)) = extra {
        if !l.is_empty() {
            l.push(',');
        }
        let _ = write!(l, "{k}=\"{v}\"");
    }
    if l.is_empty() {
        format!("{name}{suffix}")
    } else {
        format!("{name}{suffix}{{{l}}}")
    }
}

/// The registry in the Prometheus text format.
#[must_use]
pub fn render() -> String {
    let reg = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut out = String::new();
    for def in DEFS {
        let kind = match def.kind {
            Kind::Counter => "counter",
            Kind::Gauge => "gauge",
            Kind::Histogram => "histogram",
        };
        let _ = writeln!(out, "# HELP {} {}", def.name, def.help);
        let _ = writeln!(out, "# TYPE {} {kind}", def.name);
        let Some(series) = reg.get(def.name) else {
            continue;
        };
        for (labels, v) in series {
            match v {
                Value::Num(n) => {
                    let _ = writeln!(out, "{} {n}", series_name(def.name, "", labels, None));
                }
                Value::Hist { counts, sum, count } => {
                    for (b, c) in BUCKETS.iter().zip(counts) {
                        let _ = writeln!(
                            out,
                            "{} {c}",
                            series_name(def.name, "_bucket", labels, Some(("le", &b.to_string())))
                        );
                    }
                    let _ = writeln!(
                        out,
                        "{} {count}",
                        series_name(def.name, "_bucket", labels, Some(("le", "+Inf")))
                    );
                    let _ = writeln!(out, "{} {sum}", series_name(def.name, "_sum", labels, None));
                    let _ = writeln!(
                        out,
                        "{} {count}",
                        series_name(def.name, "_count", labels, None)
                    );
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_counters_gauges_and_histograms() {
        inc("locus_rate_limit_hits_total", &[("bucket", "t\"x")], 2);
        set("locus_firehose_subscribers", &[], 3.0);
        observe("locus_repo_commit_seconds", &[], 0.02);
        observe("locus_repo_commit_seconds", &[], 7.0);
        let r = render();
        assert!(
            r.contains("locus_rate_limit_hits_total{bucket=\"t\\\"x\"} 2"),
            "{r}"
        );
        assert!(r.contains("locus_firehose_subscribers 3"));
        assert!(r.contains("locus_repo_commit_seconds_bucket{le=\"0.025\"} 1"));
        assert!(r.contains("locus_repo_commit_seconds_bucket{le=\"10\"} 2"));
        assert!(r.contains("locus_repo_commit_seconds_count 2"));
        assert!(r.contains("# TYPE locus_http_request_seconds histogram"));
    }

    #[test]
    fn label_sets_are_capped() {
        for i in 0..MAX_SERIES + 10 {
            inc(
                "locus_scanner_calls_total",
                &[("result", &i.to_string())],
                1,
            );
        }
        let r = render();
        assert!(
            r.contains("locus_scanner_calls_total{result=\"other\"} 10"),
            "capped"
        );
    }
}
