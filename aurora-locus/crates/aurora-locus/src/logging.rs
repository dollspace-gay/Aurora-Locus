//! Logging setup. The format is fixed at boot; the level follows the live
//! `log.level` setting unless `RUST_LOG` overrides it. Recent lines are
//! also kept in memory for the console's Logs page.

use std::io::Write;
use std::sync::Arc;

use locus_core::bootstrap::LogFormat;
use locus_core::logs::LogRing;
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
}

impl LogControl {
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

/// Installs the global subscriber. `initial` is the level before settings
/// load (from `LOG_LEVEL`, else `info`).
pub fn init(format: LogFormat) -> LogControl {
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
    let registry = tracing_subscriber::registry().with(filter).with(to_ring);
    match format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .init(),
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
    LogControl {
        handle: if pinned.is_some() { None } else { Some(handle) },
        ring,
    }
}
