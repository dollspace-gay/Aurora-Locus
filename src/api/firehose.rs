/// WebSocket firehose for real-time event streaming
///
/// Implements com.atproto.sync.subscribeRepos

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
use tokio::time::{interval, Duration};

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

/// Handle WebSocket subscription
async fn handle_subscription(
    socket: WebSocket,
    params: SubscribeReposParams,
    ctx: AppContext,
) {
    let (mut sender, mut receiver) = socket.split();

    // Send initial info message
    let info = FirehoseFrame::Info(FirehoseInfo {
        name: "OutdatedCursor".to_string(),
        message: Some("Firehose subscription started".to_string()),
    });

    if let Ok(json) = serde_json::to_string(&info) {
        let _ = sender.send(Message::Text(json)).await;
    }

    // Start from cursor or beginning
    let mut cursor = params.cursor.unwrap_or(0);

    // Create a ticker for polling new events
    let mut tick = interval(Duration::from_millis(100));

    loop {
        tokio::select! {
            // Poll for new events
            _ = tick.tick() => {
                // Get next event from sequencer
                match ctx.sequencer.next_event(cursor).await {
                    Ok(Some(event)) => {
                        cursor = event.seq;

                        // Convert to firehose frame
                        let frame = match event.event_type.as_str() {
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
                        };

                        // Send frame if valid
                        if let Some(frame) = frame {
                            if let Ok(json) = serde_json::to_string(&frame) {
                                if sender.send(Message::Text(json)).await.is_err() {
                                    // Client disconnected
                                    break;
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // No new events, continue polling
                    }
                    Err(_) => {
                        // Error reading events, close connection
                        break;
                    }
                }
            }

            // Handle client messages (mostly ping/pong)
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) => {
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) | None => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
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
}
