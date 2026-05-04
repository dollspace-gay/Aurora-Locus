//! Real-time subscription endpoint for moderation events.
//!
//! Phase 3.9 (chainlink #106) per
//! [design doc](../../docs/AURORA_ADMIN_UI_DESIGN.md) §8.5.
//!
//! `tools.aurora.admin.subscribeModEvents` upgrades to a WebSocket
//! and streams JSON-framed messages:
//!
//! ```json
//! { "$type": "hello",     "instanceVersion": "...", "sequence": <last_event_id> }
//! { "$type": "event",     "event": {...},            "sequence": <event_id> }
//! { "$type": "heartbeat", "sequence": <last_event_id> }
//! { "$type": "error",     "code": "...", "message": "..." }
//! ```
//!
//! Polling-driven: the server polls the `moderation_event` table on
//! a 5-second tick and pushes any newly-inserted rows since the
//! last seen sequence. This is simpler than a notify-channel
//! integration and gives sub-10s end-to-end latency for v0.2.
//! Phase 3.9+ optimization can swap in LISTEN/NOTIFY-driven push
//! transparently — the wire protocol stays the same.
//!
//! Heartbeat every 30s when no events flow (per §8.5) so clients
//! can detect dead connections.
//!
//! Auth: AdminModeration scope, Moderator+ role. The WebSocket
//! upgrade carries the bearer token in the standard Authorization
//! header; see attached_role for the role check.

use crate::{
    auth::AdminAuthContext,
    AppContext,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::Response,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::Row as _;
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};

/// Subscription parameters per §8.5.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeModEventsParams {
    /// Resume from sequence position (event id).
    #[serde(default)]
    pub cursor: Option<i64>,
    #[serde(default)]
    pub actor_did: Option<String>,
    #[serde(default)]
    pub subject_did: Option<String>,
    /// Filter by subject record URI (§8.5). Distinct from
    /// `subject_did` because record-level moderation events carry a
    /// URI but no DID-as-subject.
    #[serde(default)]
    pub subject_uri: Option<String>,
    /// Filter by event-type. Multiple values are OR-combined into an
    /// `action IN (...)` clause; an empty Vec or omitted field is
    /// treated as "no action filter."
    ///
    /// Wire-format note: §8.5 specifies a list shape. v0.2 had this
    /// field as `Option<String>` (scalar) — a wire-format breaking
    /// change for any client that was sending the scalar shape.
    #[serde(default)]
    pub action_filter: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "$type")]
enum SubscribeMessage<'a> {
    #[serde(rename = "hello")]
    Hello {
        #[serde(rename = "instanceVersion")]
        instance_version: &'static str,
        sequence: i64,
    },
    #[serde(rename = "event")]
    Event {
        event: serde_json::Value,
        sequence: i64,
    },
    #[serde(rename = "heartbeat")]
    Heartbeat { sequence: i64 },
    #[serde(rename = "error")]
    Error { code: &'a str, message: &'a str },
}

const POLL_INTERVAL_SECS: u64 = 5;
const HEARTBEAT_INTERVAL_SECS: u64 = 30;
/// Cap each poll so a long backlog doesn't flood a single tick.
const MAX_EVENTS_PER_POLL: i64 = 50;

pub async fn subscribe_mod_events(
    ws: WebSocketUpgrade,
    Query(params): Query<SubscribeModEventsParams>,
    auth: AdminAuthContext,
    State(ctx): State<AppContext>,
) -> Response {
    use crate::admin::roles::Role;
    if !auth.role.can_act_as(Role::Moderator) {
        // axum WebSocketUpgrade can't reject post-auth easily without
        // returning a 403 before upgrade. Returning a normal Response
        // achieves that.
        return axum::http::Response::builder()
            .status(axum::http::StatusCode::FORBIDDEN)
            .body(axum::body::Body::from(
                "subscribeModEvents requires Moderator+ role",
            ))
            .expect("static response body");
    }
    ws.on_upgrade(move |socket| handle_subscription(socket, params, ctx))
}

async fn handle_subscription(
    socket: WebSocket,
    params: SubscribeModEventsParams,
    ctx: AppContext,
) {
    let (mut sender, mut receiver) = socket.split();

    // Determine starting cursor: caller-provided or current-tail.
    let mut cursor: i64 = match params.cursor {
        Some(c) => c,
        None => current_event_id(&ctx).await.unwrap_or(0),
    };

    // Hello
    let hello = SubscribeMessage::Hello {
        instance_version: env!("CARGO_PKG_VERSION"),
        sequence: cursor,
    };
    if send_msg(&mut sender, &hello).await.is_err() {
        return;
    }

    let mut poll_tick = interval(Duration::from_secs(POLL_INTERVAL_SECS));
    poll_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut heartbeat_tick = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    heartbeat_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_send = std::time::Instant::now();

    loop {
        tokio::select! {
            _ = poll_tick.tick() => {
                match fetch_new_events(&ctx, cursor, &params).await {
                    Ok(events) => {
                        for (id, event_json) in events {
                            let msg = SubscribeMessage::Event {
                                event: event_json,
                                sequence: id,
                            };
                            if send_msg(&mut sender, &msg).await.is_err() {
                                return;
                            }
                            cursor = id;
                            last_send = std::time::Instant::now();
                        }
                    }
                    Err(e) => {
                        tracing::warn!("subscribeModEvents poll error: {}", e);
                        let err = SubscribeMessage::Error {
                            code: "Internal",
                            message: "event poll failed",
                        };
                        let _ = send_msg(&mut sender, &err).await;
                        return;
                    }
                }
            }
            _ = heartbeat_tick.tick() => {
                if last_send.elapsed() >= Duration::from_secs(HEARTBEAT_INTERVAL_SECS) {
                    let hb = SubscribeMessage::Heartbeat { sequence: cursor };
                    if send_msg(&mut sender, &hb).await.is_err() {
                        return;
                    }
                    last_send = std::time::Instant::now();
                }
            }
            // Watch for client close
            client_msg = receiver.next() => {
                match client_msg {
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Err(_)) => return,
                    Some(Ok(_)) => { /* ignore client-side messages */ }
                }
            }
        }
    }
}

async fn current_event_id(ctx: &AppContext) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT MAX(id) AS max_id FROM moderation_event")
        .fetch_optional(&ctx.account_db)
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<Option<i64>, _>("max_id").ok().flatten())
        .unwrap_or(0))
}

async fn fetch_new_events(
    ctx: &AppContext,
    after_id: i64,
    params: &SubscribeModEventsParams,
) -> Result<Vec<(i64, serde_json::Value)>, sqlx::Error> {
    // Normalize the action filter once: an empty Vec is equivalent to
    // None, since `IN ()` is invalid SQL. Filtering at the boundary
    // means the SQL builder below sees only "no filter" or "filter
    // with N>=1 values."
    let action_filter: Option<&[String]> = params
        .action_filter
        .as_deref()
        .filter(|v| !v.is_empty());

    let mut clauses: Vec<String> = vec!["id > ?".to_string()];
    let mut binds: Vec<String> = vec![after_id.to_string()];
    if let Some(a) = &params.actor_did {
        clauses.push("actor_did = ?".to_string());
        binds.push(a.clone());
    }
    if let Some(s) = &params.subject_did {
        clauses.push("subject_did = ?".to_string());
        binds.push(s.clone());
    }
    if let Some(u) = &params.subject_uri {
        clauses.push("subject_uri = ?".to_string());
        binds.push(u.clone());
    }
    if let Some(actions) = action_filter {
        // Build `event_type IN (?, ?, ?)` with one placeholder per
        // value. The renumbering loop below converts `?` → `$N`
        // uniformly, so we don't need a backend-specific code path.
        let placeholders: Vec<&str> = actions.iter().map(|_| "?").collect();
        clauses.push(format!("event_type IN ({})", placeholders.join(", ")));
        binds.extend(actions.iter().cloned());
    }
    // Renumber `?` to `$N` for cross-backend compat.
    let mut idx = 1usize;
    let clauses_pg: Vec<String> = clauses
        .iter()
        .map(|clause| {
            let mut out = String::with_capacity(clause.len() + 8);
            for c in clause.chars() {
                if c == '?' {
                    out.push_str(&format!("${}", idx));
                    idx += 1;
                } else {
                    out.push(c);
                }
            }
            out
        })
        .collect();
    let limit_idx = binds.len() + 1;
    let sql = format!(
        "SELECT id, event_type, actor_did, subject_did, subject_uri, subject_cid, \
                details, created_at \
         FROM moderation_event WHERE {} ORDER BY id ASC LIMIT ${}",
        clauses_pg.join(" AND "),
        limit_idx
    );
    let mut q = sqlx::query(&sql);
    // First bind: after_id (we wrote it as a string for uniform binding;
    // SQL coerces. Re-parse here as i64 to bind correctly.)
    q = q.bind(after_id);
    // Skip the first bind in the binds Vec since we already bound after_id above.
    for b in binds.iter().skip(1) {
        q = q.bind(b);
    }
    q = q.bind(MAX_EVENTS_PER_POLL);
    let rows = q.fetch_all(&ctx.account_db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let event_type: String = row.try_get("event_type")?;
        let actor_did: String = row.try_get("actor_did")?;
        let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
        let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
        let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
        let details_str: String = row.try_get("details")?;
        let created_at: String = row.try_get("created_at")?;
        let json = serde_json::json!({
            "id": id,
            "eventType": event_type,
            "actorDid": actor_did,
            "subjectDid": subject_did,
            "subjectUri": subject_uri,
            "subjectCid": subject_cid,
            "details": serde_json::from_str::<serde_json::Value>(&details_str)
                .unwrap_or(serde_json::Value::String(details_str)),
            "createdAt": created_at,
        });
        out.push((id, json));
    }
    Ok(out)
}

async fn send_msg<S>(sender: &mut S, msg: &SubscribeMessage<'_>) -> Result<(), ()>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
{
    let payload = match serde_json::to_string(msg) {
        Ok(s) => s,
        Err(_) => return Err(()),
    };
    sender.send(Message::Text(payload)).await.map_err(|_| ())
}
