//! WebSocket firehose for real-time event streaming.
//!
//! Implements `com.atproto.sync.subscribeRepos` per the atproto
//! subscription protocol: DAG-CBOR-encoded binary WebSocket frames,
//! header + body packed into a single `Message::Binary` payload.
//!
//! # Wire format (Arc 14 §7.3.1)
//!
//! Each frame is two consecutive CBOR objects:
//!
//! - **Header**: `{t: "<frame-type>", op: 1}` for data frames, or
//!   `{op: -1}` for error frames. Map keys are emitted in canonical
//!   byte-length-then-lex order by `proto_blue::lex_cbor::encode`
//!   (RFC 8949 §4.2.1), so `"t"` (len 1) precedes `"op"` (len 2).
//! - **Body**: a `LexValue::Map` carrying the frame payload.
//!
//! `#commit` body `blocks` field is emitted as raw CBOR major-type-2
//! bytes (NOT base64). Field-absence on optional fields is enforced
//! by omit-if-none discipline in the builders (`firehose_encoder`).
//!
//! # Cursor management
//!
//! Pre-stream validation (Step 3 — currently delegating to the
//! existing event-count outdated heuristic; Step 3 will switch to a
//! time-window based check with explicit `FutureCursor`/`OutdatedCursor`
//! error frames + `repo_backfill_limit` config).
//!
//! # Protocol errors + close codes (Arc 14 §7.3.4)
//!
//! Named lexicon errors emitted via error frames (header `op: -1`):
//!
//! - `FutureCursor` — client cursor beyond current head. WS close 1008.
//! - `ConsumerTooSlow` — client slow-reader buffer overflowed.
//!   WS close 1008.
//!
//! WebSocket close codes:
//!
//! - **1000** — normal close (client disconnect, server graceful shutdown).
//! - **1008** — policy violation (`FutureCursor`, `ConsumerTooSlow`).
//! - **1011** — internal error. No named lexicon error frame is emitted
//!   (per Sub-step 0.G: the atproto subscription lexicon doesn't
//!   define an "internal error" frame name; close-code alone is the
//!   signal).
//!
//! `#info` frames carry only the lexicon's `knownValues` for `name`:
//! `"OutdatedCursor"` is the only value Aurora-Locus emits. Pre-Arc-14
//! spurious values `"Connected"` and `"Error"` are removed.
//!
//! # Backpressure
//!
//! Buffered mpsc channel (100 events) + 5s send timeout. Slow
//! consumers trigger a `ConsumerTooSlow` error frame + WebSocket
//! close code 1008.
//!
//! # Connection health
//!
//! Ping/pong every 30s. Clean close on client disconnect.

use crate::{
    context::AppContext,
    sequencer::events::{AccountEvent, CommitEvent, IdentityEvent, SyncEvent},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::Response,
    routing::get,
    Router,
};
use futures::{sink::SinkExt, stream::StreamExt};
use proto_blue::lex_data::LexValue;
use serde::Deserialize;
use tokio::{
    sync::mpsc,
    time::{interval, timeout, Duration, Instant},
};

use crate::api::firehose_encoder::{
    account_body_to_lex_value, commit_body_to_lex_value, commit_op_to_lex_value,
    firehose_error_frame_to_cbor, firehose_frame_to_cbor, identity_body_to_lex_value,
    info_body_to_lex_value, sync_body_to_lex_value,
};

/// Firehose configuration constants.
const BUFFER_SIZE: usize = 100;
const POLL_INTERVAL_MS: u64 = 100;
const SEND_TIMEOUT_MS: u64 = 5000;
const PING_INTERVAL_SECS: u64 = 30;

/// Request parameters for subscribeRepos.
#[derive(Debug, Deserialize)]
pub struct SubscribeReposParams {
    pub cursor: Option<i64>,
}

/// In-memory frame representation passed across the producer/consumer
/// channel. Body is the typed `LexValue::Map`; the wire-encoding step
/// happens at the consumer (so encoding errors close the connection
/// rather than killing the producer task).
#[derive(Debug, Clone)]
pub enum FirehoseFrame {
    Data { frame_type: String, body: LexValue },
    Error { name: String, message: Option<String> },
}

/// Convenience constructor for an info frame.
fn info_frame(name: &str, message: Option<&str>) -> FirehoseFrame {
    FirehoseFrame::Data {
        frame_type: "#info".to_string(),
        body: info_body_to_lex_value(name, message),
    }
}

/// WebSocket handler for `subscribeRepos`.
pub async fn subscribe_repos(
    ws: WebSocketUpgrade,
    Query(params): Query<SubscribeReposParams>,
    State(ctx): State<AppContext>,
) -> Response {
    ws.on_upgrade(move |socket| handle_subscription(socket, params, ctx))
}

/// Why a cursor was diagnosed as outdated. Distinguishes the two
/// cases that share an OutdatedCursor `#info` emission per §7.3.3
/// but log at different `at_branch` tags (chainlink #76 fix).
#[derive(Debug, Clone, PartialEq, Eq)]
enum OutdatedReason {
    /// Cursor < earliest seq within the configured backfill window.
    /// At least one event sits inside the window — we advance to it.
    CursorBelowWindow,
    /// Every event in `repo_seq` is outside the configured window
    /// (window too narrow OR retention pruned every backfill-eligible
    /// row). Distinct from the genuinely-empty-table case
    /// (`current_seq == 0`); this case is reachable ONLY when
    /// `current_seq > 0`. Advance to `current_seq`, skipping the
    /// backfill entirely.
    WindowExcludedAllEvents,
}

/// Pure decision over the cursor-validation inputs (Arc 14 §7.3.3 +
/// chainlink #76 closure). Caller computes `earliest_in_window` via
/// `Sequencer::earliest_after_time` before invoking; no I/O happens
/// inside. Returned variant drives the handler's frame emission +
/// cursor placement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CursorDecision {
    /// No cursor sent (`requested_cursor == 0`): start streaming
    /// from current head. cursor = current_seq.
    LiveTailNoCursor,
    /// `repo_seq` is empty (`current_seq == 0`) with a non-zero
    /// requested cursor — round-1 F8 closure "empty-window
    /// fall-through" semantics, distinct from #76's
    /// `WindowExcludedAllEvents`. cursor = 0 so any first event is
    /// visible.
    EmptyRepoSeq,
    /// `requested_cursor > current_seq > 0`: client state is invalid.
    /// Emit `FutureCursor` error frame + WS close 1008.
    FutureCursor,
    /// Cursor sits within or past the configured backfill window.
    /// Stream from `starting_cursor` forward (no info frame).
    Backfill { starting_cursor: i64 },
    /// Cursor is outdated for one of the `OutdatedReason` cases.
    /// Emit `#info OutdatedCursor` + advance to `advanced_to`.
    OutdatedCursor {
        reason: OutdatedReason,
        advanced_to: i64,
    },
}

fn decide_cursor(
    requested_cursor: i64,
    current_seq: i64,
    earliest_in_window: Option<i64>,
) -> CursorDecision {
    if requested_cursor == 0 {
        return CursorDecision::LiveTailNoCursor;
    }
    if current_seq == 0 {
        // Round-1 F8 closure: requested_cursor > 0 with an empty
        // table → silent live-tail (no OutdatedCursor; nothing to
        // advance past).
        return CursorDecision::EmptyRepoSeq;
    }
    if requested_cursor > current_seq {
        return CursorDecision::FutureCursor;
    }
    match earliest_in_window {
        Some(earliest) if requested_cursor < earliest => {
            // Subtract 1 because `next_event` is exclusive on `cursor`.
            CursorDecision::OutdatedCursor {
                reason: OutdatedReason::CursorBelowWindow,
                advanced_to: earliest - 1,
            }
        }
        Some(_) => {
            // requested_cursor is within or past the window's earliest;
            // normal backfill from the requested cursor.
            CursorDecision::Backfill {
                starting_cursor: requested_cursor,
            }
        }
        None => {
            // chainlink #76: we know `current_seq > 0` (gate above),
            // so the table is NOT empty. `None` from
            // `earliest_after_time` therefore means every event is
            // outside the window — cursor is provably outdated.
            // Emit OutdatedCursor + advance to current_seq (skip
            // backfill, go to live-tail).
            CursorDecision::OutdatedCursor {
                reason: OutdatedReason::WindowExcludedAllEvents,
                advanced_to: current_seq,
            }
        }
    }
}

/// Handle WebSocket subscription with backpressure and error recovery.
///
/// Pre-stream cursor validation per Arc 14 §7.3.3 + chainlink #76:
/// delegates the branch selection to [`decide_cursor`] (pure,
/// unit-tested) and acts on the returned `CursorDecision`. Each
/// branch logs at `tracing::info!` with an `at_branch` field for
/// operator visibility (same per-? logging discipline as the
/// Arc 13 #71 fix).
async fn handle_subscription(socket: WebSocket, params: SubscribeReposParams, ctx: AppContext) {
    let (mut sender, mut receiver) = socket.split();

    let current_seq = match ctx.sequencer.current_seq().await {
        Ok(Some(seq)) => seq,
        Ok(None) => 0,
        Err(_) => {
            let _ = send_internal_error_close(&mut sender).await;
            return;
        }
    };

    let requested_cursor = params.cursor.unwrap_or(0);
    let backfill_secs = ctx.sequencer.backfill_limit_secs();

    // `earliest_in_window` is only consulted by `decide_cursor` when
    // the requested cursor + current head warrant it. We compute it
    // eagerly here only when both gates would let it matter — saves
    // a DB round-trip on no-cursor + empty-table paths.
    let earliest_in_window = if requested_cursor > 0 && current_seq > 0 {
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(backfill_secs);
        match ctx.sequencer.earliest_after_time(cutoff).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    at_branch = "outdated_cursor_lookup",
                    error = %e,
                    "firehose: earliest_after_time DB error — closing connection",
                );
                let _ = send_internal_error_close(&mut sender).await;
                return;
            }
        }
    } else {
        None
    };

    let decision = decide_cursor(requested_cursor, current_seq, earliest_in_window);

    let cursor: i64 = match decision {
        CursorDecision::LiveTailNoCursor => {
            tracing::info!(
                at_branch = "live_tail_no_cursor",
                current_seq,
                "firehose: live-tail-from-now, no cursor provided",
            );
            current_seq
        }
        CursorDecision::EmptyRepoSeq => {
            tracing::info!(
                at_branch = "empty_repo_seq",
                requested_cursor,
                "firehose: empty repo_seq, falling through to live-tail",
            );
            0
        }
        CursorDecision::FutureCursor => {
            tracing::info!(
                at_branch = "future_cursor",
                requested_cursor,
                current_seq,
                "firehose: FutureCursor emitted, cursor beyond current head",
            );
            let frame = FirehoseFrame::Error {
                name: "FutureCursor".to_string(),
                message: Some(format!(
                    "Cursor {} is beyond current head {}",
                    requested_cursor, current_seq
                )),
            };
            let _ = send_frame(&mut sender, &frame).await;
            let _ = sender
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: 1008,
                    reason: "FutureCursor".into(),
                })))
                .await;
            return;
        }
        CursorDecision::Backfill { starting_cursor } => {
            tracing::info!(
                at_branch = "backfill",
                requested_cursor = starting_cursor,
                earliest_in_window = earliest_in_window.unwrap_or(0),
                "firehose: backfill from cursor inside configured window",
            );
            starting_cursor
        }
        CursorDecision::OutdatedCursor {
            reason,
            advanced_to,
        } => {
            let (msg, reason_tag) = match reason {
                OutdatedReason::CursorBelowWindow => (
                    format!(
                        "Requested cursor {} is older than the configured \
                         backfill window ({}s). Advancing to {}.",
                        requested_cursor,
                        backfill_secs,
                        advanced_to + 1
                    ),
                    "cursor_below_window",
                ),
                OutdatedReason::WindowExcludedAllEvents => (
                    format!(
                        "Requested cursor {} is older than every event in the \
                         configured backfill window ({}s). Advancing to \
                         current_seq={}.",
                        requested_cursor, backfill_secs, advanced_to
                    ),
                    "window_excluded_all_events",
                ),
            };
            tracing::info!(
                at_branch = "outdated_cursor",
                reason = reason_tag,
                requested_cursor,
                current_seq,
                earliest_in_window = earliest_in_window.unwrap_or(0),
                advanced_to,
                "firehose: OutdatedCursor emitted",
            );
            let frame = info_frame("OutdatedCursor", Some(&msg));
            if send_frame(&mut sender, &frame).await.is_err() {
                return;
            }
            advanced_to
        }
    };

    // Buffered channel for backpressure.
    let (event_tx, mut event_rx) = mpsc::channel::<FirehoseFrame>(BUFFER_SIZE);

    let producer_ctx = ctx.clone();
    let producer =
        tokio::spawn(async move { produce_events(producer_ctx, cursor, event_tx).await });

    let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECS));
    let mut last_activity = Instant::now();

    loop {
        tokio::select! {
            Some(frame) = event_rx.recv() => {
                match send_frame_with_timeout(&mut sender, &frame).await {
                    Ok(_) => {
                        last_activity = Instant::now();
                    }
                    Err(SendError::Timeout) => {
                        tracing::warn!("Send timeout, client may be slow");
                        let _ = send_consumer_too_slow_close(&mut sender).await;
                        break;
                    }
                    Err(SendError::Disconnected) => {
                        tracing::debug!("Client disconnected during send");
                        break;
                    }
                    Err(SendError::EncodeFailed(e)) => {
                        tracing::error!("Frame encoding failed: {}", e);
                        let _ = send_internal_error_close(&mut sender).await;
                        break;
                    }
                }
            }

            _ = ping_interval.tick() => {
                if last_activity.elapsed() > Duration::from_secs(PING_INTERVAL_SECS)
                    && sender.send(Message::Ping(vec![])).await.is_err() {
                        break;
                    }
            }

            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) => {
                        tracing::debug!("Client closed connection");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let send_result = sender.send(Message::Pong(data)).await;
                        if send_result.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_activity = Instant::now();
                    }
                    Some(Err(e)) => {
                        tracing::error!("WebSocket error: {}", e);
                        break;
                    }
                    None => {
                        tracing::debug!("Client disconnected");
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    producer.abort();
}

/// Produce events from the sequencer and forward as `FirehoseFrame`
/// values over the buffered channel.
async fn produce_events(ctx: AppContext, mut cursor: i64, tx: mpsc::Sender<FirehoseFrame>) {
    let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
    let mut error_count = 0;
    const MAX_ERRORS: u32 = 5;
    const BATCH_THRESHOLD: i64 = 100;

    loop {
        tick.tick().await;

        let current_seq = match ctx.sequencer.current_seq().await {
            Ok(Some(seq)) => seq,
            Ok(None) => 0,
            Err(_) => cursor + 1,
        };

        let events_behind = current_seq - cursor;

        if events_behind > BATCH_THRESHOLD {
            tracing::debug!(
                "Catch-up mode: {} events behind, using batch fetch",
                events_behind
            );

            match ctx.sequencer.next_events(cursor, Some(500)).await {
                Ok(events) if !events.is_empty() => {
                    error_count = 0;
                    for event in events {
                        cursor = event.seq;
                        if let Some(frame) = event_to_frame(event) {
                            if tx.send(frame).await.is_err() {
                                return;
                            }
                        }
                    }
                }
                Ok(_) => {
                    error_count = 0;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!("Error reading events (batch): {}", e);
                    if error_count >= MAX_ERRORS {
                        tracing::error!("Too many errors, closing producer");
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100 * 2_u64.pow(error_count))).await;
                }
            }
        } else {
            match ctx.sequencer.next_event(cursor).await {
                Ok(Some(event)) => {
                    error_count = 0;
                    cursor = event.seq;
                    if let Some(frame) = event_to_frame(event) {
                        if tx.send(frame).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(None) => {
                    error_count = 0;
                }
                Err(e) => {
                    error_count += 1;
                    tracing::error!("Error reading event: {}", e);
                    if error_count >= MAX_ERRORS {
                        tracing::error!("Too many errors, closing producer");
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100 * 2_u64.pow(error_count))).await;
                }
            }
        }
    }
}

/// Convert a stored `SeqRow` to a typed `FirehoseFrame` (header + body).
///
/// Returns `None` if the stored event bytes fail to deserialize or
/// the body builder rejects a value (e.g. malformed CID). Returning
/// `None` drops the event silently from this consumer's view; the
/// next consumer may successfully consume if the issue was
/// per-consumer transient (unlikely — most rejection paths reflect
/// stored-data corruption).
fn event_to_frame(event: crate::sequencer::SeqRow) -> Option<FirehoseFrame> {
    let time = event.sequenced_at.to_rfc3339();
    match event.event_type.as_str() {
        "commit" => {
            let commit = serde_cbor::from_slice::<CommitEvent>(&event.event).ok()?;
            let ops_lex: Result<Vec<LexValue>, _> = commit
                .ops
                .iter()
                .map(|op| {
                    commit_op_to_lex_value(
                        match op.action {
                            crate::sequencer::events::OpAction::Create => "create",
                            crate::sequencer::events::OpAction::Update => "update",
                            crate::sequencer::events::OpAction::Delete => "delete",
                        },
                        &op.path,
                        op.cid.as_deref(),
                        // Arc 14 §7.3.2: prior record version CID;
                        // absent for create ops.
                        op.prev.as_deref(),
                    )
                })
                .collect();
            let ops = ops_lex.ok()?;
            let body = commit_body_to_lex_value(
                event.seq,
                commit.rebase,
                commit.too_big,
                &commit.repo,
                &commit.commit,
                &commit.rev,
                commit.since.as_deref(),
                // Arc 14 §7.3.2: prior commit's MST root CID; absent
                // for genesis commits.
                commit.prev_data.as_deref(),
                commit.blocks,
                ops,
                &commit.blobs,
                &time,
            )
            .ok()?;
            Some(FirehoseFrame::Data {
                frame_type: "#commit".to_string(),
                body,
            })
        }
        "sync" => {
            let sync = serde_cbor::from_slice::<SyncEvent>(&event.event).ok()?;
            let body = sync_body_to_lex_value(
                event.seq,
                &sync.did,
                &sync.rev,
                sync.blocks,
                &time,
            )
            .ok()?;
            Some(FirehoseFrame::Data {
                frame_type: "#sync".to_string(),
                body,
            })
        }
        "identity" => {
            let identity = serde_cbor::from_slice::<IdentityEvent>(&event.event).ok()?;
            let body = identity_body_to_lex_value(
                event.seq,
                &identity.did,
                &time,
                identity.handle.as_deref(),
            );
            Some(FirehoseFrame::Data {
                frame_type: "#identity".to_string(),
                body,
            })
        }
        "account" => {
            let account = serde_cbor::from_slice::<AccountEvent>(&event.event).ok()?;
            let status_str = account.status.map(|s| match s {
                crate::sequencer::events::AccountStatus::Takendown => "takendown",
                crate::sequencer::events::AccountStatus::Suspended => "suspended",
                crate::sequencer::events::AccountStatus::Deleted => "deleted",
                crate::sequencer::events::AccountStatus::Deactivated => "deactivated",
                crate::sequencer::events::AccountStatus::Desynchronized => "desynchronized",
                crate::sequencer::events::AccountStatus::Throttled => "throttled",
            });
            let body = account_body_to_lex_value(
                event.seq,
                &account.did,
                &time,
                account.active,
                status_str,
            );
            Some(FirehoseFrame::Data {
                frame_type: "#account".to_string(),
                body,
            })
        }
        _ => None,
    }
}

/// Local error type for frame-send failures.
#[derive(Debug)]
enum SendError {
    Timeout,
    Disconnected,
    EncodeFailed(crate::error::PdsError),
}

/// Encode a `FirehoseFrame` to a binary CBOR payload.
fn encode_frame(frame: &FirehoseFrame) -> Result<Vec<u8>, crate::error::PdsError> {
    match frame {
        FirehoseFrame::Data { frame_type, body } => {
            firehose_frame_to_cbor(frame_type, body.clone())
        }
        FirehoseFrame::Error { name, message } => {
            firehose_error_frame_to_cbor(name, message.as_deref())
        }
    }
}

/// Send a frame as a binary WebSocket message with a send timeout.
async fn send_frame_with_timeout(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    frame: &FirehoseFrame,
) -> Result<(), SendError> {
    let bytes = encode_frame(frame).map_err(SendError::EncodeFailed)?;

    match timeout(
        Duration::from_millis(SEND_TIMEOUT_MS),
        sender.send(Message::Binary(bytes)),
    )
    .await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(SendError::Disconnected),
        Err(_) => Err(SendError::Timeout),
    }
}

/// Send a frame as a binary WebSocket message without timeout.
async fn send_frame(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    frame: &FirehoseFrame,
) -> Result<(), ()> {
    let bytes = encode_frame(frame).map_err(|_| ())?;
    sender.send(Message::Binary(bytes)).await.map_err(|_| ())
}

/// Emit a `ConsumerTooSlow` error frame and close with WebSocket
/// code 1008 (Policy Violation). Step 4 finalizes the close-code
/// emission discipline.
async fn send_consumer_too_slow_close(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), ()> {
    let frame = FirehoseFrame::Error {
        name: "ConsumerTooSlow".to_string(),
        message: Some("Client processing too slow".to_string()),
    };
    let _ = send_frame(sender, &frame).await;
    sender
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: 1008,
            reason: "ConsumerTooSlow".into(),
        })))
        .await
        .map_err(|_| ())
}

/// Close the connection with WebSocket code 1011 (Internal Error)
/// without emitting a named lexicon error frame. Per Step 0 Sub-step
/// 0.G: the atproto lexicon does NOT define a named subscription
/// error for internal server failures; close-code 1011 alone is the
/// signal.
async fn send_internal_error_close(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), ()> {
    sender
        .send(Message::Close(Some(axum::extract::ws::CloseFrame {
            code: 1011,
            reason: "InternalError".into(),
        })))
        .await
        .map_err(|_| ())
}

/// Build firehose routes.
pub fn routes() -> Router<AppContext> {
    Router::new().route(
        "/xrpc/com.atproto.sync.subscribeRepos",
        get(subscribe_repos),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::{events::CommitOp, events::OpAction, CommitEvent, SeqRow};
    use chrono::Utc;

    #[test]
    fn test_subscribe_repos_params_deserialize() {
        let json = r#"{"cursor":123}"#;
        let params: SubscribeReposParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.cursor, Some(123));

        let json_no_cursor = r#"{}"#;
        let params_no_cursor: SubscribeReposParams = serde_json::from_str(json_no_cursor).unwrap();
        assert_eq!(params_no_cursor.cursor, None);
    }

    // ============================================================
    // decide_cursor — Arc 14 §7.3.3 + chainlink #76 closure.
    // Each test names the precondition it exercises so a future
    // reader can map test → §7.3.3 branch.
    // ============================================================

    /// Branch 1 (live-tail, no cursor sent). cursor=0 → start from head.
    #[test]
    fn decide_cursor_live_tail_when_no_cursor() {
        assert_eq!(
            decide_cursor(0, 6, Some(3)),
            CursorDecision::LiveTailNoCursor
        );
        // Also when both are 0 (empty table, no cursor).
        assert_eq!(decide_cursor(0, 0, None), CursorDecision::LiveTailNoCursor);
    }

    /// Round-1 F8 closure: cursor > 0 with an empty `repo_seq`
    /// (`current_seq == 0`) → silent live-tail (no OutdatedCursor).
    /// Distinct from chainlink #76's WindowExcludedAllEvents case.
    #[test]
    fn decide_cursor_empty_repo_seq_no_outdated() {
        assert_eq!(decide_cursor(5, 0, None), CursorDecision::EmptyRepoSeq);
    }

    /// Branch 2 (FutureCursor): cursor > current_seq > 0.
    #[test]
    fn decide_cursor_future_cursor_when_beyond_head() {
        assert_eq!(decide_cursor(10, 5, Some(3)), CursorDecision::FutureCursor);
        assert_eq!(decide_cursor(2, 1, Some(1)), CursorDecision::FutureCursor);
    }

    /// Branch 3 normal-backfill: cursor sits within the window.
    /// `earliest_in_window <= cursor <= current_seq`.
    #[test]
    fn decide_cursor_normal_backfill_when_cursor_within_window() {
        assert_eq!(
            decide_cursor(5, 6, Some(3)),
            CursorDecision::Backfill { starting_cursor: 5 }
        );
        // Boundary: cursor == earliest_in_window.
        assert_eq!(
            decide_cursor(3, 6, Some(3)),
            CursorDecision::Backfill { starting_cursor: 3 }
        );
    }

    /// Branch 3 OutdatedCursor — CursorBelowWindow: cursor < earliest
    /// available event in the window. Advance to `earliest - 1` so
    /// the exclusive `next_event(cursor)` returns the window's
    /// earliest event next.
    #[test]
    fn decide_cursor_outdated_below_window_advances_to_earliest_minus_one() {
        assert_eq!(
            decide_cursor(1, 6, Some(3)),
            CursorDecision::OutdatedCursor {
                reason: OutdatedReason::CursorBelowWindow,
                advanced_to: 2,
            }
        );
    }

    /// chainlink #76 fix — WindowExcludedAllEvents: every event in
    /// `repo_seq` is outside the configured window
    /// (`earliest_after_time = None`) but the table is NOT empty
    /// (`current_seq > 0`). Cursor is provably outdated; advance to
    /// `current_seq` (live-tail forward) AND emit OutdatedCursor.
    /// Reproduces Phase B Scenario 3c with
    /// PDS_REPO_BACKFILL_LIMIT_MS=1000 + stale repo_seq.
    #[test]
    fn decide_cursor_outdated_window_excluded_all_events_chainlink_76() {
        assert_eq!(
            decide_cursor(1, 6, None),
            CursorDecision::OutdatedCursor {
                reason: OutdatedReason::WindowExcludedAllEvents,
                advanced_to: 6,
            }
        );
    }

    /// chainlink #76 regression boundary: the WindowExcludedAllEvents
    /// path must NOT fire when `current_seq == 0` (that's the
    /// EmptyRepoSeq case — round-1 F8 closure preserves silent
    /// live-tail). Confirms the two cases are correctly disambiguated.
    #[test]
    fn decide_cursor_does_not_emit_outdated_when_repo_seq_genuinely_empty() {
        let d = decide_cursor(5, 0, None);
        assert!(
            matches!(d, CursorDecision::EmptyRepoSeq),
            "expected EmptyRepoSeq, got {:?}",
            d
        );
    }

    /// Round-trip: a stored commit event becomes a `FirehoseFrame::Data`
    /// with `frame_type = "#commit"`; encoding produces a non-empty
    /// CBOR payload starting with the canonical header `0xa2 0x61 0x74`.
    #[test]
    fn test_event_to_frame_commit_binary_encoded() {
        let commit_event = CommitEvent {
            rebase: false,
            too_big: false,
            repo: "did:plc:test".to_string(),
            commit: "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string(),
            rev: "3l4example".to_string(),
            since: None,
            blocks: vec![1, 2, 3],
            ops: vec![CommitOp {
                action: OpAction::Create,
                path: "app.bsky.feed.post/123".to_string(),
                cid: Some(
                    "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string(),
                ),
                prev: None,
            }],
            blobs: vec![],
            prev_data: None,
        };

        let event_bytes = serde_cbor::to_vec(&commit_event).unwrap();
        let seq_row = SeqRow {
            seq: 1,
            did: "did:plc:test".to_string(),
            event_type: "commit".to_string(),
            event: event_bytes,
            invalidated: false,
            sequenced_at: Utc::now(),
        };

        let frame = event_to_frame(seq_row).expect("frame builds");
        match &frame {
            FirehoseFrame::Data { frame_type, .. } => {
                assert_eq!(frame_type, "#commit");
            }
            FirehoseFrame::Error { .. } => panic!("expected Data, got Error"),
        }

        // Wire format: canonical header `{t: "#commit", op: 1}` then body.
        let bytes = encode_frame(&frame).expect("encode");
        assert_eq!(&bytes[0..3], &[0xa2, 0x61, 0x74]);
    }

    /// Info-frame helper produces a Data variant with `frame_type = "#info"`.
    #[test]
    fn test_info_frame_helper() {
        let frame = info_frame("OutdatedCursor", Some("test"));
        match frame {
            FirehoseFrame::Data { frame_type, .. } => {
                assert_eq!(frame_type, "#info");
            }
            _ => panic!("expected Data"),
        }
    }

    /// Error-frame variant encodes to a header with `op: -1`.
    #[test]
    fn test_error_frame_encode_op_neg1() {
        let frame = FirehoseFrame::Error {
            name: "FutureCursor".to_string(),
            message: Some("test".to_string()),
        };
        let bytes = encode_frame(&frame).expect("encode");
        // Header is `{op: -1}` → 0xa1 0x62 0x6f 0x70 0x20.
        assert_eq!(&bytes[0..5], &[0xa1, 0x62, 0x6f, 0x70, 0x20]);
    }

    /// Arc 14 §7.3.4 / §7.6.4: `ConsumerTooSlow` is a named lexicon
    /// error (header `op: -1`), NOT an `#info` frame.
    #[test]
    fn test_consumer_too_slow_is_error_frame() {
        let frame = FirehoseFrame::Error {
            name: "ConsumerTooSlow".to_string(),
            message: Some("slow".to_string()),
        };
        let bytes = encode_frame(&frame).expect("encode");
        // op: -1 header.
        assert_eq!(&bytes[0..5], &[0xa1, 0x62, 0x6f, 0x70, 0x20]);
        // Body contains the literal "ConsumerTooSlow" name string.
        let needle = b"ConsumerTooSlow";
        assert!(
            bytes.windows(needle.len()).any(|w| w == needle),
            "error name 'ConsumerTooSlow' must appear in encoded body"
        );
    }

    /// Arc 14 §7.3.4 / §7.6.4: the only `#info` name value
    /// Aurora-Locus emits is `"OutdatedCursor"`. Pre-Arc-14 spurious
    /// `"Connected"` and `"Error"` names are removed; this test will
    /// regression-catch their reintroduction by grepping the encoded
    /// output of the info-frame helper.
    #[test]
    fn test_info_frame_only_outdated_cursor_name_used() {
        let frame = info_frame("OutdatedCursor", Some("test"));
        let bytes = encode_frame(&frame).expect("encode");
        assert!(
            bytes.windows(b"OutdatedCursor".len()).any(|w| w == b"OutdatedCursor"),
            "expected OutdatedCursor name in encoded info frame"
        );
        assert!(
            !bytes.windows(b"Connected".len()).any(|w| w == b"Connected"),
            "Connected name MUST NOT appear in info frame"
        );
        // Note: substring "Error" appears innocuously inside other words
        // (e.g. "InternalError"), but the literal name string
        // `name: "Error"` would be encoded as text-string-5 prefix
        // 0x65 followed by ASCII bytes — check that exact byte
        // sequence is absent.
        let error_name_encoded: &[u8] = b"\x65Error";
        assert!(
            !bytes.windows(error_name_encoded.len()).any(|w| w == error_name_encoded),
            "Error name MUST NOT appear in info frame"
        );
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_constants() {
        assert!(BUFFER_SIZE > 0);
        assert!(BUFFER_SIZE <= 1000);
        assert!(POLL_INTERVAL_MS >= 10);
        assert!(SEND_TIMEOUT_MS >= 1000);
        assert!(PING_INTERVAL_SECS >= 10);
    }
}
