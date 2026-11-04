//! Logging setup. The format is fixed at boot; the level follows the live
//! `log.level` setting unless `RUST_LOG` overrides it.

use locus_core::bootstrap::LogFormat;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::reload;
use tracing_subscriber::util::SubscriberInitExt;

/// Handle to change the log level while running.
#[derive(Clone)]
pub struct LogControl {
    handle: Option<reload::Handle<EnvFilter, tracing_subscriber::Registry>>,
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
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json())
            .init(),
        LogFormat::Pretty => registry.with(tracing_subscriber::fmt::layer()).init(),
    }
    LogControl {
        handle: if pinned.is_some() { None } else { Some(handle) },
    }
}
