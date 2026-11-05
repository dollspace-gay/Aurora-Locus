//! Sync: the sequencer (outbox → numbered event log), the firehose's
//! per-node tail and fan-out, and crawler notification.
//!
//! Events are written to the outbox in the same transaction as the change
//! they describe. One sequencer at a time (leased) moves them to
//! `seq_event` with gapless seqs; each node tails the log and fans new
//! events out to its subscribers.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use locus_core::lease::LeaseToken;
use locus_core::{CoreError, CoreResult};
use locus_store::Store;
use locus_store::seq::SeqEvent;
use proto_blue_lex_data::LexValue;
use proto_blue_ws::frame::{ErrorFrame, Frame, MessageFrame};
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

/// One sequencer pass: moves up to `batch` outbox rows into the log, in
/// outbox-id order, fenced on the sequencer's lease. A deleted account's
/// `#account` event schedules the purge of its earlier events
/// `preservation_ms` later. Returns how many rows were sequenced.
pub async fn sequence_once(
    store: &Store,
    lease: &LeaseToken,
    batch: i64,
    preservation_ms: i64,
) -> CoreResult<usize> {
    let mut tx = store.begin().await?;
    tx.fence(lease).await?;
    let state = tx.lock_seq_state().await?;
    let rows = tx.outbox_batch(batch).await?;
    if rows.is_empty() {
        tx.rollback().await?;
        return Ok(0);
    }
    let mut seq = state.last_seq;
    for row in &rows {
        seq += 1;
        tx.sequence_row(row, seq).await?;
        if row.kind == "account" && is_deleted(&row.payload) {
            tx.queue_seq_purge(&row.did, seq, preservation_ms).await?;
        }
    }
    tx.set_last_seq(seq).await?;
    tx.commit().await?;
    Ok(rows.len())
}

fn is_deleted(payload: &[u8]) -> bool {
    proto_blue_lex_cbor::decode(payload)
        .ok()
        .and_then(|v| {
            v.as_map()
                .and_then(|m| m.get("status"))
                .and_then(LexValue::as_str)
                .map(|s| s == "deleted")
        })
        .unwrap_or(false)
}

/// A sequenced event, framed for the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameEvent {
    pub seq: i64,
    pub bytes: Vec<u8>,
}

/// Frames an event: header `{op: 1, t: "#<kind>"}`, body the stored event
/// with `seq` and `time` added.
pub fn frame(ev: &SeqEvent) -> CoreResult<FrameEvent> {
    let body = proto_blue_lex_cbor::decode(&ev.payload)
        .map_err(|e| CoreError::Storage(format!("event {}: {e}", ev.seq)))?;
    let LexValue::Map(mut m) = body else {
        return Err(CoreError::Storage(format!("event {} isn't a map", ev.seq)));
    };
    m.insert("seq".into(), LexValue::Integer(ev.seq));
    m.insert(
        "time".into(),
        LexValue::String(locus_core::time::iso8601(ev.sequenced_at)),
    );
    let frame = Frame::Message(MessageFrame {
        r#type: Some(format!("#{}", ev.kind)),
        body: LexValue::Map(m),
    });
    Ok(FrameEvent {
        seq: ev.seq,
        bytes: frame
            .encode()
            .map_err(|e| CoreError::Storage(e.to_string()))?,
    })
}

/// An `#info` frame (e.g. `OutdatedCursor`).
#[must_use]
pub fn info_frame(name: &str, message: &str) -> Vec<u8> {
    let mut m = BTreeMap::new();
    m.insert("name".to_owned(), LexValue::String(name.to_owned()));
    m.insert("message".to_owned(), LexValue::String(message.to_owned()));
    Frame::Message(MessageFrame {
        r#type: Some("#info".into()),
        body: LexValue::Map(m),
    })
    .encode()
    .unwrap_or_default()
}

/// An error frame (`FutureCursor`, `ConsumerTooSlow`); the stream ends after it.
#[must_use]
pub fn error_frame(error: &str, message: &str) -> Vec<u8> {
    Frame::Error(ErrorFrame {
        error: error.to_owned(),
        message: Some(message.to_owned()),
    })
    .encode()
    .unwrap_or_default()
}

/// This node's view of the log: a tail task reads new events and
/// broadcasts them to subscribers.
pub struct Firehose {
    store: Store,
    live: broadcast::Sender<Arc<FrameEvent>>,
    wake: Notify,
    tailed: AtomicI64,
}

impl Firehose {
    /// `buffer` is how far a subscriber may fall behind live before it's
    /// dropped as too slow.
    #[must_use]
    pub fn new(store: Store, buffer: usize) -> Arc<Self> {
        let (live, _) = broadcast::channel(buffer.max(1));
        Arc::new(Self {
            store,
            live,
            wake: Notify::new(),
            tailed: AtomicI64::new(-1),
        })
    }

    /// Tells the tail new events may be in the log.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<FrameEvent>> {
        self.live.subscribe()
    }

    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Follows the log until `cancel`: woken locally, by Postgres `NOTIFY`
    /// from another node's sequencer, or every `poll`.
    pub async fn run_tail(self: Arc<Self>, cancel: CancellationToken, poll: Duration) {
        let mut pos = match self.store.seq_state().await {
            Ok(s) => s.last_seq,
            Err(e) => {
                tracing::error!("firehose can't read the log: {e}");
                0
            }
        };
        self.tailed.store(pos, Ordering::SeqCst);
        let mut listener = self.listen().await;
        loop {
            let notified = async {
                match listener.as_mut() {
                    Some(l) => l.recv().await.map(|_| ()).map_err(|e| e.to_string()),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = self.wake.notified() => {}
                () = tokio::time::sleep(poll) => {}
                res = notified => if res.is_err() { listener = None; }
            }
            loop {
                match self.store.seq_events_after(pos, 500).await {
                    Ok(page) => {
                        let full = page.len() == 500;
                        for ev in &page {
                            match frame(ev) {
                                Ok(f) => {
                                    let _ = self.live.send(Arc::new(f));
                                }
                                Err(e) => tracing::error!("unframeable event: {e}"),
                            }
                            pos = ev.seq;
                        }
                        self.tailed.store(pos, Ordering::SeqCst);
                        if !full {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::warn!("firehose tail read failed: {e}");
                        break;
                    }
                }
            }
            if listener.is_none() {
                listener = self.listen().await;
            }
        }
    }

    async fn listen(&self) -> Option<sqlx::postgres::PgListener> {
        let Store::Postgres(pool) = &self.store else {
            return None;
        };
        let mut l = sqlx::postgres::PgListener::connect_with(pool).await.ok()?;
        l.listen(locus_store::seq::SEQ_CHANNEL).await.ok()?;
        Some(l)
    }
}

/// When to call each crawler: at most once per `min_interval`, and a call
/// that falls inside the window is scheduled for its end instead of dropped.
pub struct CrawlSchedule {
    min_interval: Duration,
    last: HashMap<String, Instant>,
    pending: HashMap<String, Instant>,
}

impl CrawlSchedule {
    #[must_use]
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    /// New events arrived: each crawler is due now, or scheduled.
    pub fn on_events(&mut self, crawlers: &[String], now: Instant) {
        for c in crawlers {
            let due = self
                .last
                .get(c)
                .map_or(now, |&t| (t + self.min_interval).max(now));
            self.pending.entry(c.clone()).or_insert(due);
        }
    }

    /// The crawlers to call now (marked as called).
    pub fn due(&mut self, now: Instant) -> Vec<String> {
        let ready: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, t)| **t <= now)
            .map(|(c, _)| c.clone())
            .collect();
        for c in &ready {
            self.pending.remove(c);
            self.last.insert(c.clone(), now);
        }
        ready
    }

    /// When the next scheduled call is due, if any.
    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.values().min().copied()
    }
}

/// Asks a crawler to crawl this PDS (`com.atproto.sync.requestCrawl`).
pub async fn request_crawl(
    http: &reqwest::Client,
    crawler: &str,
    hostname: &str,
) -> Result<(), String> {
    let url = format!(
        "{}/xrpc/com.atproto.sync.requestCrawl",
        crawler.trim_end_matches('/')
    );
    let res = http
        .post(url)
        .json(&serde_json::json!({ "hostname": hostname }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status().is_success() {
        Ok(())
    } else {
        Err(format!("{} answered {}", crawler, res.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crawls_are_spaced_and_scheduled_not_dropped() {
        let t0 = Instant::now();
        let window = Duration::from_secs(1200);
        let crawlers = vec![
            "https://relay.one".to_owned(),
            "https://relay.two".to_owned(),
        ];
        let mut s = CrawlSchedule::new(window);
        s.on_events(&crawlers, t0);
        let mut first = s.due(t0);
        first.sort();
        assert_eq!(first, crawlers);
        // Events inside the window: scheduled for the window's end.
        s.on_events(&crawlers, t0 + Duration::from_secs(60));
        assert!(s.due(t0 + Duration::from_secs(61)).is_empty());
        assert_eq!(s.next_due(), Some(t0 + window));
        assert_eq!(s.due(t0 + window).len(), 2);
        assert!(s.next_due().is_none(), "nothing more until new events");
    }

    #[test]
    fn frames_carry_seq_time_and_type() {
        let mut body = BTreeMap::new();
        body.insert("did".to_owned(), LexValue::String("did:plc:x".into()));
        let payload = proto_blue_lex_cbor::encode(&LexValue::Map(body)).unwrap();
        let ev = SeqEvent {
            seq: 7,
            did: "did:plc:x".into(),
            kind: "identity".into(),
            payload,
            sequenced_at: 0,
        };
        let f = frame(&ev).unwrap();
        let Frame::Message(m) = Frame::decode(&f.bytes).unwrap() else {
            panic!("not a message")
        };
        assert_eq!(m.r#type.as_deref(), Some("#identity"));
        let b = m.body.as_map().unwrap();
        assert_eq!(b["seq"], LexValue::Integer(7));
        assert_eq!(
            b["time"],
            LexValue::String("1970-01-01T00:00:00.000Z".into())
        );
        assert!(matches!(
            Frame::decode(&error_frame("FutureCursor", "x")).unwrap(),
            Frame::Error(_)
        ));
    }
}
