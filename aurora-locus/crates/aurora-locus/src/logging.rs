//! Logging setup. The format is fixed at boot; the level follows the live
//! `log.level` setting unless `RUST_LOG` overrides it. Recent lines are
//! also kept in memory for the console's Logs page. A serving node also
//! exports trace spans over OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` (or
//! `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`) is set.

use std::io::Write;
use std::sync::Arc;

use locus_core::bootstrap::LogFormat;
use locus_core::logs::LogRing;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

/// Handle to change the log level while running, and the recent lines.
#[derive(Clone)]
pub struct LogControl {
    handle: Option<reload::Handle<EnvFilter, tracing_subscriber::Registry>>,
    ring: Arc<LogRing>,
    /// The OTLP span exporter, when on.
    tracer: Option<SdkTracerProvider>,
}

impl LogControl {
    /// Sends the spans not exported yet (on the way out).
    pub fn shutdown(&self) {
        if let Some(t) = &self.tracer
            && let Err(e) = t.shutdown()
        {
            eprintln!("OTLP export didn't finish: {e}");
        }
    }

    /// Applies a level from settings. A `RUST_LOG` override pins the filter.
    pub fn apply(&self, level: &str) {
        let Some(handle) = &self.handle else { return };
        match EnvFilter::try_new(level) {
            Ok(filter) => {
                if let Err(e) = handle.reload(filter) {
                    tracing::warn!("couldn't change log level: {e}");
                }
            }
            Err(e) => tracing::warn!("bad log level {level:?}: {e}"),
        }
    }

    pub fn ring(&self) -> Arc<LogRing> {
        Arc::clone(&self.ring)
    }
}

/// Writes each formatted event into the ring.
#[derive(Clone)]
struct RingMaker(Arc<LogRing>);

struct RingLine {
    buf: Vec<u8>,
    ring: Arc<LogRing>,
}

impl Write for RingLine {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for RingLine {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.buf);
        for line in text.lines().filter(|l| !l.is_empty()) {
            self.ring.push(line.to_owned());
        }
    }
}

impl<'a> MakeWriter<'a> for RingMaker {
    type Writer = RingLine;

    fn make_writer(&'a self) -> RingLine {
        RingLine {
            buf: Vec::new(),
            ring: Arc::clone(&self.0),
        }
    }
}

/// The OTLP exporter, when an endpoint is set in env (the standard
/// `OTEL_*` variables configure it: endpoint, headers, timeout).
fn otlp_provider() -> Option<SdkTracerProvider> {
    let set = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    set("OTEL_EXPORTER_OTLP_ENDPOINT").or_else(|| set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"))?;
    let exporter = match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("OTLP export is off: {e}");
            return None;
        }
    };
    let name = set("OTEL_SERVICE_NAME").unwrap_or_else(|| "aurora-locus".to_owned());
    Some(
        SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(
                opentelemetry_sdk::Resource::builder()
                    .with_service_name(name)
                    .build(),
            )
            .build(),
    )
}

/// Installs the global subscriber for a one-off command (no span export).
pub fn init(format: LogFormat) -> LogControl {
    init_with(format, false)
}

/// Installs the global subscriber for a serving node: spans go out over
/// OTLP when an endpoint is set.
pub fn init_serve(format: LogFormat) -> LogControl {
    init_with(format, true)
}

/// Installs the global subscriber. The level before settings load is
/// `LOG_LEVEL`, else `info`.
fn init_with(format: LogFormat, otlp: bool) -> LogControl {
    let pinned = std::env::var("RUST_LOG").ok().filter(|v| !v.is_empty());
    let initial = pinned
        .clone()
        .or_else(|| std::env::var("LOG_LEVEL").ok())
        .unwrap_or_else(|| "info".to_owned());
    let filter = EnvFilter::try_new(&initial).unwrap_or_else(|_| EnvFilter::new("info"));
    let (filter, handle) = reload::Layer::new(filter);
    let ring = Arc::new(LogRing::default());
    let to_ring = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(RingMaker(Arc::clone(&ring)));
    let tracer = if otlp { otlp_provider() } else { None };
    let otel = tracer
        .as_ref()
        .map(|t| tracing_opentelemetry::layer().with_tracer(t.tracer("aurora-locus")));
    locus_core::logs::set_request_spans(tracer.is_some());
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(to_ring)
        .with(otel);
    match format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .init(),
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
    if tracer.is_some() {
        tracing::info!("exporting trace spans over OTLP");
    }
    LogControl {
        handle: if pinned.is_some() { None } else { Some(handle) },
        ring,
        tracer,
    }
}
