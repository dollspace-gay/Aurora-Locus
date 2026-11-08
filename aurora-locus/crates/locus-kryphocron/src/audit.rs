//! Kryphocron's audit sinks, as bounded in-memory channels to a writer task.
//!
//! Sinks are synchronous and must not block: `record` enqueues and returns,
//! and a full channel is `BufferRejected`. Events kryphocron emits
//! fire-and-forget that are rejected are counted and logged by the fallback
//! sink, which can't fail.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use kryphocron::audit::{
    ChannelAuditEvent, CompositeOpId, FallbackAuditEvent, ModerationAuditEvent,
    SubstrateAuditEvent, UserAuditEvent,
};
use kryphocron::{
    AuditError, CapabilityKind, ChannelAuditSink, FallbackAuditSink, ModerationAuditSink, SinkKind,
    SubstrateAuditSink, TraceId, UserAuditSink,
};
use rand::RngCore;
use sha2::Sha256;
use tokio::sync::mpsc;

/// How many events may wait for the writer.
pub const BUFFER: usize = 10_000;

/// One event, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    pub at: i64,
    pub sink: &'static str,
    pub event: String,
    pub correlation_id: Option<String>,
    pub detail: String,
}

/// A fresh trace id for one request or job step.
#[must_use]
pub fn new_trace() -> TraceId {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    TraceId::from_bytes(b)
}

/// Every sink kryphocron takes, over one channel.
pub struct AuditSinks {
    tx: mpsc::Sender<AuditEvent>,
    correlation_key: [u8; 32],
    rejected: AtomicU64,
}

impl std::fmt::Debug for AuditSinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditSinks")
            .field("rejected", &self.rejected.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

fn ms(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// An event's name and detail from its `Debug` form (kryphocron's events
/// carry no plaintext).
fn describe(debug: String) -> (String, String) {
    let end = debug
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(debug.len());
    (debug[..end].to_owned(), debug)
}

impl AuditSinks {
    /// Sinks and the receiving end the writer task drains.
    #[must_use]
    pub fn new(correlation_key: [u8; 32]) -> (Self, mpsc::Receiver<AuditEvent>) {
        let (tx, rx) = mpsc::channel(BUFFER);
        (
            Self {
                tx,
                correlation_key,
                rejected: AtomicU64::new(0),
            },
            rx,
        )
    }

    /// Events refused because the channel was full.
    #[must_use]
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Ordering::Relaxed)
    }

    /// The correlation id of a trace: keyed by the install's correlation
    /// key, so ids don't correlate across installs.
    #[must_use]
    pub fn correlation_id(&self, trace: &TraceId) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.correlation_key)
            .expect("HMAC takes any key length");
        mac.update(trace.as_bytes());
        hex::encode(&mac.finalize().into_bytes()[..16])
    }

    fn push(
        &self,
        sink: &'static str,
        at: SystemTime,
        trace: Option<&TraceId>,
        debug: String,
    ) -> Result<(), AuditError> {
        let (event, detail) = describe(debug);
        self.push_row(AuditEvent {
            at: ms(at),
            sink,
            event,
            correlation_id: trace.map(|t| self.correlation_id(t)),
            detail,
        })
    }

    fn push_row(&self, row: AuditEvent) -> Result<(), AuditError> {
        self.tx.try_send(row).map_err(|e| {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            let row = match e {
                mpsc::error::TrySendError::Full(r) | mpsc::error::TrySendError::Closed(r) => r,
            };
            tracing::warn!(
                sink = row.sink,
                event = row.event,
                "kryphocron audit event rejected (buffer full)"
            );
            AuditError::BufferRejected
        })
    }

    /// Records a locus event of its own (rotation passes, trusted fetches).
    pub fn record_locus(&self, event: &str, detail: String) {
        let _ = self.push_row(AuditEvent {
            at: ms(SystemTime::now()),
            sink: "locus",
            event: event.to_owned(),
            correlation_id: None,
            detail,
        });
    }
}

impl UserAuditSink for AuditSinks {
    fn record(&self, event: UserAuditEvent) -> Result<(), AuditError> {
        self.push("user", SystemTime::now(), None, format!("{event:?}"))
    }
}

impl ChannelAuditSink for AuditSinks {
    fn record(&self, event: ChannelAuditEvent) -> Result<(), AuditError> {
        self.push("channel", SystemTime::now(), None, format!("{event:?}"))
    }
}

impl SubstrateAuditSink for AuditSinks {
    fn record(&self, event: SubstrateAuditEvent) -> Result<(), AuditError> {
        self.push("substrate", SystemTime::now(), None, format!("{event:?}"))
    }
}

impl ModerationAuditSink for AuditSinks {
    fn record(&self, event: ModerationAuditEvent) -> Result<(), AuditError> {
        self.push("moderation", SystemTime::now(), None, format!("{event:?}"))
    }
}

/// The fallback sink: logs, and can't fail.
impl FallbackAuditSink for AuditSinks {
    fn record_panic(
        &self,
        sink: SinkKind,
        trace_id: TraceId,
        capability: CapabilityKind,
        _at: SystemTime,
    ) {
        tracing::error!(
            ?sink,
            ?capability,
            correlation = self.correlation_id(&trace_id),
            "kryphocron audit sink panicked"
        );
    }

    fn record_composite_failure(
        &self,
        trace_id: TraceId,
        composite_op_id: CompositeOpId,
        sinks_committed: &[SinkKind],
        sinks_failed: &[SinkKind],
        _at: SystemTime,
    ) {
        tracing::error!(
            ?composite_op_id,
            ?sinks_committed,
            ?sinks_failed,
            correlation = self.correlation_id(&trace_id),
            "kryphocron composite audit failed"
        );
    }

    fn record_event(&self, event: FallbackAuditEvent) {
        tracing::warn!(event = ?event, "kryphocron fallback audit event");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_go_through_the_channel_and_a_full_one_rejects() {
        let (sinks, mut rx) = AuditSinks::new([7; 32]);
        sinks.record_locus("rotation.pass", "accounts=1".into());
        let e = rx.try_recv().unwrap();
        assert_eq!((e.sink, e.event.as_str()), ("locus", "rotation.pass"));
        for _ in 0..BUFFER {
            sinks.record_locus("x", String::new());
        }
        sinks.record_locus("one.too.many", String::new());
        assert_eq!(sinks.rejected(), 1);
        let t = TraceId::from_bytes([1; 16]);
        assert_eq!(sinks.correlation_id(&t), sinks.correlation_id(&t));
        let (other, _rx) = AuditSinks::new([8; 32]);
        assert_ne!(sinks.correlation_id(&t), other.correlation_id(&t));
    }
}
