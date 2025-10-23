/// WebSocket firehose for real-time event streaming
///
/// Implements com.atproto.sync.subscribeRepos with comprehensive production features:
///
/// # Features
///
/// ## Backpressure Handling
/// - Buffered channel (100 events) prevents overwhelming slow clients
/// - Timeout on sends (5s) detects and disconnects slow consumers
/// - Producer-consumer pattern separates event fetching from transmission
///
/// ## Cursor Management
/// - Clients can resume from any sequence number
/// - Outdated cursor detection (max 1000 events behind)
/// - Automatic adjustment when cursor is too old
///
/// ## Error Recovery
/// - Exponential backoff on database errors (max 5 attempts)
/// - Graceful shutdown on producer failures
/// - Detailed error messages sent to clients before disconnect
///
/// ## Connection Health
/// - Ping/pong every 30 seconds to detect dead connections
/// - Activity tracking to optimize keepalive messages
/// - Clean shutdown on client disconnect
///
/// ## Performance
/// - Non-blocking producer polls every 100ms
/// - Efficient CBOR event deserialization
/// - Base64-encoded CAR blocks in JSON frames
///
/// # Protocol
///
/// Clients connect via WebSocket and receive JSON frames:
/// - `#commit`: Repository commit with operations
/// - `#identity`: Handle changes
/// - `#account`: Account status changes
/// - `#info`: Control messages (connection status, errors)
///
/// Each frame includes a monotonically increasing `seq` number for cursor tracking.

use crate::{
    context::AppContext,
    error::{PdsError, PdsResult},
    sequencer::events::{AccountEvent, CommitEvent, IdentityEvent},
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
use base64::{Engine as _, engine::general_purpose};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc,
    time::{interval, timeout, Duration, Instant},
};

/// Firehose configuration constants
const BUFFER_SIZE: usize = 100; // Size of the event buffer for backpressure
const POLL_INTERVAL_MS: u64 = 100; // How often to poll for new events
const SEND_TIMEOUT_MS: u64 = 5000; // Timeout for sending a message
const PING_INTERVAL_SECS: u64 = 30; // Send ping every 30 seconds
const MAX_CATCHUP_EVENTS: i64 = 1000; // Max events to send in catch-up mode

/// Request parameters for subscribeRepos
#[derive(Debug, Deserialize)]
pub struct SubscribeReposParams {
    /// Optional cursor to start from (sequence number)
    pub cursor: Option<i64>,
}

/// Firehose event frame
#[derive(Debug, Serialize)]
#[serde(tag = "$type")]
pub enum FirehoseFrame {
    #[serde(rename = "#commit")]
    Commit(FirehoseCommit),
    #[serde(rename = "#identity")]
    Identity(FirehoseIdentity),
    #[serde(rename = "#account")]
    Account(FirehoseAccount),
    #[serde(rename = "#info")]
    Info(FirehoseInfo),
}

/// Commit event for firehose
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirehoseCommit {
    pub seq: i64,
    pub rebase: bool,
    pub too_big: bool,
    pub repo: String,
    pub commit: String,
    pub rev: String,
    pub since: Option<String>,
    pub blocks: String, // Base64-encoded CAR bytes
    pub ops: Vec<FirehoseOp>,
    pub blobs: Vec<String>,
    pub time: chrono::DateTime<chrono::Utc>,
}

/// Operation in a commit
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirehoseOp {
    pub action: String, // "create", "update", "delete"
    pub path: String,
    pub cid: Option<String>,
}

/// Identity event for firehose
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirehoseIdentity {
    pub seq: i64,
    pub did: String,
    pub time: chrono::DateTime<chrono::Utc>,
    pub handle: Option<String>,
}

/// Account event for firehose
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirehoseAccount {
    pub seq: i64,
    pub did: String,
    pub time: chrono::DateTime<chrono::Utc>,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Info message for firehose
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirehoseInfo {
    pub name: String,
    pub message: Option<String>,
}

/// WebSocket handler for subscribeRepos
pub async fn subscribe_repos(
    ws: WebSocketUpgrade,
    Query(params): Query<SubscribeReposParams>,
    State(ctx): State<AppContext>,
) -> Response {
    ws.on_upgrade(move |socket| handle_subscription(socket, params, ctx))
}

/// Handle WebSocket subscription with backpressure and error recovery
async fn handle_subscription(
    socket: WebSocket,
    params: SubscribeReposParams,
    ctx: AppContext,
) {
    let (mut sender, mut receiver) = socket.split();

    // Validate cursor and get current sequence
    let current_seq = match ctx.sequencer.current_seq().await {
        Ok(Some(seq)) => seq,
        Ok(None) => 0,
        Err(_) => {
            // Send error and close
            let _ = send_error(&mut sender, "Failed to initialize firehose").await;
            return;
        }
    };

    // Start from cursor or beginning
    let requested_cursor = params.cursor.unwrap_or(0);
    let mut cursor = requested_cursor;

    // Check if cursor is too old (backfill limit)
    if requested_cursor > 0 && current_seq - requested_cursor > MAX_CATCHUP_EVENTS {
        // Cursor too old, send info message
        let info = FirehoseFrame::Info(FirehoseInfo {
            name: "OutdatedCursor".to_string(),
            message: Some(format!(
                "Requested cursor {} is too old. Current: {}. Starting from {}",
                requested_cursor,
                current_seq,
                current_seq - MAX_CATCHUP_EVENTS
            )),
        });
        if send_frame(&mut sender, &info).await.is_err() {
            return;
        }
        cursor = current_seq - MAX_CATCHUP_EVENTS;
    }

    // Send initial info message
    let info = FirehoseFrame::Info(FirehoseInfo {
        name: "Connected".to_string(),
        message: Some(format!("Firehose subscription started at seq {}", cursor)),
    });
    if send_frame(&mut sender, &info).await.is_err() {
        return;
    }

    // Create buffered channel for backpressure handling
    let (event_tx, mut event_rx) = mpsc::channel::<FirehoseFrame>(BUFFER_SIZE);

    // Spawn event producer task
    let producer_ctx = ctx.clone();
    let producer = tokio::spawn(async move {
        produce_events(producer_ctx, cursor, event_tx).await
    });

    // Create ping interval
    let mut ping_interval = interval(Duration::from_secs(PING_INTERVAL_SECS));
    let mut last_activity = Instant::now();

    // Main event loop
    loop {
        tokio::select! {
            // Send events from buffer
            Some(frame) = event_rx.recv() => {
                match send_frame_with_timeout(&mut sender, &frame).await {
                    Ok(_) => {
                        last_activity = Instant::now();
                    }
                    Err(SendError::Timeout) => {
                        tracing::warn!("Send timeout, client may be slow");
                        // Send error message and close
                        let _ = send_error(&mut sender, "Client processing too slow").await;
                        break;
                    }
                    Err(SendError::Disconnected) => {
                        tracing::debug!("Client disconnected during send");
                        break;
                    }
                }
            }

            // Send periodic pings
            _ = ping_interval.tick() => {
                if last_activity.elapsed() > Duration::from_secs(PING_INTERVAL_SECS) {
                    if sender.send(Message::Ping(vec![])).await.is_err() {
                        break;
                    }
                }
            }

            // Handle client messages
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) => {
                        tracing::debug!("Client closed connection");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // Client acknowledged our ping
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

    // Cancel producer task
    producer.abort();
}

/// Produce events from sequencer and send to channel
async fn produce_events(
    ctx: AppContext,
    mut cursor: i64,
    tx: mpsc::Sender<FirehoseFrame>,
) {
    let mut tick = interval(Duration::from_millis(POLL_INTERVAL_MS));
    let mut error_count = 0;
    const MAX_ERRORS: u32 = 5;

    loop {
        tick.tick().await;

        // Get next event from sequencer
        match ctx.sequencer.next_event(cursor).await {
            Ok(Some(event)) => {
                error_count = 0; // Reset error count on success
                cursor = event.seq;

                // Convert to firehose frame
                if let Some(frame) = event_to_frame(event) {
                    // Try to send to channel (with backpressure)
                    if tx.send(frame).await.is_err() {
                        // Channel closed, consumer disconnected
                        break;
                    }
                }
            }
            Ok(None) => {
                // No new events, continue polling
                error_count = 0;
            }
            Err(e) => {
                // Error reading events
                error_count += 1;
                tracing::error!("Error reading event: {}", e);

                if error_count >= MAX_ERRORS {
                    tracing::error!("Too many errors, closing producer");
                    break;
                }

                // Exponential backoff
                tokio::time::sleep(Duration::from_millis(100 * 2_u64.pow(error_count))).await;
            }
        }
    }
}

/// Convert SeqRow to FirehoseFrame
fn event_to_frame(event: crate::sequencer::SeqRow) -> Option<FirehoseFrame> {
    match event.event_type.as_str() {
        "commit" => {
            // Deserialize commit event
            if let Ok(commit) = serde_cbor::from_slice::<CommitEvent>(&event.event) {
                Some(FirehoseFrame::Commit(FirehoseCommit {
                    seq: event.seq,
                    rebase: commit.rebase,
                    too_big: commit.too_big,
                    repo: commit.repo,
                    commit: commit.commit,
                    rev: commit.rev,
                    since: commit.since,
                    blocks: general_purpose::STANDARD.encode(&commit.blocks),
                    ops: commit.ops.iter().map(|op| FirehoseOp {
                        action: match op.action {
                            crate::sequencer::events::OpAction::Create => "create".to_string(),
                            crate::sequencer::events::OpAction::Update => "update".to_string(),
                            crate::sequencer::events::OpAction::Delete => "delete".to_string(),
                        },
                        path: op.path.clone(),
                        cid: op.cid.clone(),
                    }).collect(),
                    blobs: commit.blobs,
                    time: event.sequenced_at,
                }))
            } else {
                None
            }
        }
        "identity" => {
            if let Ok(identity) = serde_cbor::from_slice::<IdentityEvent>(&event.event) {
                Some(FirehoseFrame::Identity(FirehoseIdentity {
                    seq: event.seq,
                    did: identity.did,
                    time: event.sequenced_at,
                    handle: identity.handle,
                }))
            } else {
                None
            }
        }
        "account" => {
            if let Ok(account) = serde_cbor::from_slice::<AccountEvent>(&event.event) {
                Some(FirehoseFrame::Account(FirehoseAccount {
                    seq: event.seq,
                    did: account.did,
                    time: event.sequenced_at,
                    active: account.active,
                    status: account.status.map(|s| match s {
                        crate::sequencer::events::AccountStatus::Takendown => "takendown".to_string(),
                        crate::sequencer::events::AccountStatus::Suspended => "suspended".to_string(),
                        crate::sequencer::events::AccountStatus::Deleted => "deleted".to_string(),
                        crate::sequencer::events::AccountStatus::Deactivated => "deactivated".to_string(),
                    }),
                }))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Error type for sending frames
#[derive(Debug)]
enum SendError {
    Timeout,
    Disconnected,
}

/// Send a frame with timeout
async fn send_frame_with_timeout(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    frame: &FirehoseFrame,
) -> Result<(), SendError> {
    let json = serde_json::to_string(frame)
        .map_err(|_| SendError::Disconnected)?;

    match timeout(
        Duration::from_millis(SEND_TIMEOUT_MS),
        sender.send(Message::Text(json))
    ).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(_)) => Err(SendError::Disconnected),
        Err(_) => Err(SendError::Timeout),
    }
}

/// Send a frame without timeout
async fn send_frame(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    frame: &FirehoseFrame,
) -> Result<(), ()> {
    let json = serde_json::to_string(frame)
        .map_err(|_| ())?;
    sender.send(Message::Text(json)).await.map_err(|_| ())
}

/// Send error message and close connection
async fn send_error(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    message: &str,
) -> Result<(), ()> {
    let error_frame = FirehoseFrame::Info(FirehoseInfo {
        name: "Error".to_string(),
        message: Some(message.to_string()),
    });
    send_frame(sender, &error_frame).await?;
    sender.send(Message::Close(None)).await.map_err(|_| ())
}

/// Build firehose routes
pub fn routes() -> Router<AppContext> {
    Router::new().route(
        "/xrpc/com.atproto.sync.subscribeRepos",
        get(subscribe_repos),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequencer::{CommitEvent, SeqRow, events::CommitOp, events::OpAction};
    use chrono::Utc;

    #[test]
    fn test_firehose_op_serialize() {
        let op = FirehoseOp {
            action: "create".to_string(),
            path: "app.bsky.feed.post/123".to_string(),
            cid: Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string()),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert!(json.contains("create"));
        assert!(json.contains("app.bsky.feed.post/123"));
    }

    #[test]
    fn test_firehose_commit_serialize() {
        let commit = FirehoseCommit {
            seq: 123,
            rebase: false,
            too_big: false,
            repo: "did:plc:test123".to_string(),
            commit: "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string(),
            rev: "3l4example".to_string(),
            since: None,
            blocks: "".to_string(),
            ops: vec![],
            blobs: vec![],
            time: Utc::now(),
        };
        let frame = FirehoseFrame::Commit(commit);
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"$type\":\"#commit\""));
        assert!(json.contains("did:plc:test123"));
    }

    #[test]
    fn test_event_to_frame_commit() {
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
                cid: Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string()),
            }],
            blobs: vec![],
            prev: None,
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

        let frame = event_to_frame(seq_row);
        assert!(frame.is_some());

        if let Some(FirehoseFrame::Commit(commit)) = frame {
            assert_eq!(commit.seq, 1);
            assert_eq!(commit.repo, "did:plc:test");
            assert_eq!(commit.ops.len(), 1);
            assert_eq!(commit.ops[0].action, "create");
        } else {
            panic!("Expected Commit frame");
        }
    }

    #[test]
    fn test_firehose_info_serialize() {
        let info = FirehoseInfo {
            name: "Connected".to_string(),
            message: Some("Test message".to_string()),
        };
        let frame = FirehoseFrame::Info(info);
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"$type\":\"#info\""));
        assert!(json.contains("Connected"));
        assert!(json.contains("Test message"));
    }

    #[test]
    fn test_subscribe_repos_params_deserialize() {
        let json = r#"{"cursor":123}"#;
        let params: SubscribeReposParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.cursor, Some(123));

        let json_no_cursor = r#"{}"#;
        let params_no_cursor: SubscribeReposParams = serde_json::from_str(json_no_cursor).unwrap();
        assert_eq!(params_no_cursor.cursor, None);
    }

    #[test]
    fn test_firehose_frame_variants() {
        // Test all frame type serialization
        let commit_frame = FirehoseFrame::Commit(FirehoseCommit {
            seq: 1,
            rebase: false,
            too_big: false,
            repo: "did:plc:test".to_string(),
            commit: "cid123".to_string(),
            rev: "rev123".to_string(),
            since: None,
            blocks: "".to_string(),
            ops: vec![],
            blobs: vec![],
            time: Utc::now(),
        });
        assert!(serde_json::to_string(&commit_frame).is_ok());

        let identity_frame = FirehoseFrame::Identity(FirehoseIdentity {
            seq: 2,
            did: "did:plc:test".to_string(),
            time: Utc::now(),
            handle: Some("test.bsky.social".to_string()),
        });
        assert!(serde_json::to_string(&identity_frame).is_ok());

        let account_frame = FirehoseFrame::Account(FirehoseAccount {
            seq: 3,
            did: "did:plc:test".to_string(),
            time: Utc::now(),
            active: true,
            status: Some("active".to_string()),
        });
        assert!(serde_json::to_string(&account_frame).is_ok());

        let info_frame = FirehoseFrame::Info(FirehoseInfo {
            name: "Test".to_string(),
            message: Some("Test message".to_string()),
        });
        assert!(serde_json::to_string(&info_frame).is_ok());
    }

    #[test]
    fn test_constants() {
        // Verify configuration constants are reasonable
        assert!(BUFFER_SIZE > 0);
        assert!(BUFFER_SIZE <= 1000); // Not too large
        assert!(POLL_INTERVAL_MS >= 10); // Not polling too fast
        assert!(SEND_TIMEOUT_MS >= 1000); // At least 1 second
        assert!(PING_INTERVAL_SECS >= 10); // At least 10 seconds
        assert!(MAX_CATCHUP_EVENTS > 100); // Reasonable catchup window
    }
}
