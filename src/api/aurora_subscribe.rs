//! Real-time subscription endpoint for moderation events.
//!
//! Phase 3.9 (chainlink #106) per
//! [design doc](../../docs/AURORA_ADMIN_UI_DESIGN.md) §8.5.
//!
//! `tools.aurora.admin.subscribeModEvents` upgrades to a WebSocket
//! and streams JSON-framed messages:
//!
//! ```json
//! { "$type": "hello",          "instanceVersion": "...", "sequence": <last_seq> }
//! { "$type": "event",          "event": {...},           "sequence": <seq> }
//! { "$type": "auditEntry",     "entry": {...AuditEntry...}, "sequence": <chain_seq> }
//! { "$type": "heartbeat",      "sequence": <last_seq> }
//! { "$type": "outdatedCursor", "oldestAvailableSeq": <seq>, "message": "..." }
//! { "$type": "error",          "code": "...", "message": "..." }
//! ```
//!
//! The `entry` payload of the `auditEntry` frame is the same
//! `audit_chain::AuditEntry` shape returned in `getAuditTrail`'s
//! `items` array (§8.4). Sharing the type means consumers parse one
//! schema regardless of whether the row arrived via polling or via
//! the live tail. The envelope-level `sequence` is the chain
//! sequence (matching `entry.sequence`) and is what subscribers
//! cursor on for resume.
//!
//! Polling-driven: the server polls the retention-bounded
//! `mod_event_seq` table on a 5-second tick and pushes any newly-
//! inserted rows since the last seen `seq`. The unbounded
//! `moderation_event` table is the historical aggregate (queried via
//! `tools.aurora.moderator.queryEvents`); the streaming surface uses
//! `mod_event_seq` so storage stays bounded by operator-configured
//! retention (chainlink #115 / §3.5).
//!
//! When `include_audit_chain` is requested AND the caller's role
//! permits chain visibility, the same tick also polls
//! `audit_chain_entry` and merges new rows into the stream by
//! timestamp order (sequence as tiebreaker). Phase 3.9+ optimization
//! can swap in LISTEN/NOTIFY-driven push transparently — the wire
//! protocol stays the same.
//!
//! `OutdatedCursor` (§8.5): if the caller's `cursor` is older than
//! the oldest row in `mod_event_seq` (i.e., events have been pruned
//! since the last subscription), the handler emits one
//! `OutdatedCursor` frame on connect and closes cleanly. The client
//! re-bootstraps via `queryEvents` and resubscribes with a fresh
//! cursor.
//!
//! Heartbeat every 30s when no events flow (per §8.5) so clients
//! can detect dead connections.
//!
//! Auth: AdminModeration scope, Moderator+ role. The WebSocket
//! upgrade carries the bearer token in the standard Authorization
//! header; see attached_role for the role check.
//!
//! Audit chain visibility role gate mirrors `getAuditTrail` (§8.4):
//! Moderator+ permits chain row visibility. If the caller's role
//! doesn't qualify, AuditEntry messages are SILENTLY omitted from
//! the stream even when `include_audit_chain: true` is requested
//! — per §3.6 stealth/non-enumeration framing, an error here would
//! tell the operator something about the role-tier hierarchy they
//! may not need to know.

use crate::{
    admin::{audit_chain::AuditEntry, defs::Subject},
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
use chrono::{DateTime, Utc};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::Row as _;
use std::time::Duration;
use tokio::time::{interval, MissedTickBehavior};

/// Subscription parameters per §8.5.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeModEventsParams {
    /// Resume from sequence position (event id) for the
    /// moderation_event stream. Distinct from `audit_chain_cursor`
    /// because event ids and chain sequences are independent
    /// monotonic counters.
    #[serde(default)]
    pub cursor: Option<i64>,
    /// Resume from chain sequence for the audit_chain_entry stream.
    /// Only meaningful when `include_audit_chain` is true; otherwise
    /// ignored. Default `None` means "start from current chain head"
    /// (matching how `cursor` defaults for events).
    ///
    /// Two separate cursors rather than one combined cursor keeps
    /// the wire format backward-compatible: existing v0.2 clients
    /// that send a single `cursor` keep working untouched. Clients
    /// opting into audit chain pass the second field; on reconnect
    /// they remember both independently.
    #[serde(default)]
    pub audit_chain_cursor: Option<i64>,
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
    #[serde(default)]
    pub action_filter: Option<Vec<String>>,
    /// Opt into audit-chain-entry messages alongside moderation
    /// events. Per §8.5, default is false. When true, the server
    /// also polls `audit_chain_entry` each tick and emits
    /// `AuditEntry` messages interleaved with `Event` messages by
    /// timestamp order. The visibility role gate (Moderator+,
    /// mirroring getAuditTrail per §8.4) applies; insufficient role
    /// silently drops chain entries without erroring (§3.6
    /// non-enumeration).
    #[serde(default)]
    pub include_audit_chain: bool,
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
    /// Audit chain row. Per §8.5, the wire shape is
    /// `{ entry: AuditEntry, sequence: u64 }` — `entry` is the same
    /// `audit_chain::AuditEntry` returned by `getAuditTrail`'s
    /// `items` so consumers parse one schema regardless of source.
    /// The envelope-level `sequence` mirrors `entry.sequence` and is
    /// what subscribers cursor on for resume; the duplication is
    /// intentional per spec so consumers can cursor without
    /// inspecting the payload.
    ///
    /// `verified` (inside `entry`) is the per-row hash recompute —
    /// same primitive getAuditTrail uses. Chain-level verification
    /// stays an explicit getAuditTrail request because it walks the
    /// whole chain.
    #[serde(rename = "auditEntry", rename_all = "camelCase")]
    AuditEntry { entry: Box<AuditEntry>, sequence: i64 },
    #[serde(rename = "heartbeat")]
    Heartbeat { sequence: i64 },
    /// Per §8.5: emitted when the caller's `cursor` is older than the
    /// oldest available row in `mod_event_seq`. The client missed
    /// events that have been pruned by the retention cleanup job
    /// (chainlink #115). After this frame the server closes the
    /// WebSocket cleanly; the client re-bootstraps via
    /// `tools.aurora.moderator.queryEvents` for the missed window
    /// and resubscribes with a fresh cursor (or omits cursor to
    /// start from the current tail).
    #[serde(rename = "outdatedCursor", rename_all = "camelCase")]
    OutdatedCursor {
        /// The lowest `seq` currently retained in `mod_event_seq`.
        /// Resuming from this value (or any value ≥ it) avoids the
        /// outdated-cursor failure on the next connect.
        oldest_available_seq: i64,
        message: &'a str,
    },
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
    ws.on_upgrade(move |socket| handle_subscription(socket, params, ctx, auth))
}

/// Whether `role` is permitted to see audit-chain rows. Mirrors the
/// `getAuditTrail` (§8.4) gate exactly so the streaming and polled
/// surfaces stay in lockstep — if a future tightening raises the
/// chain-visibility floor, both paths move together by updating
/// this one helper.
fn can_see_audit_chain(role: crate::admin::roles::Role) -> bool {
    use crate::admin::roles::Role;
    role.can_act_as(Role::Moderator)
}

async fn handle_subscription(
    socket: WebSocket,
    params: SubscribeModEventsParams,
    ctx: AppContext,
    auth: AdminAuthContext,
) {
    let (mut sender, mut receiver) = socket.split();

    // OutdatedCursor detection (chainlink #115 / §8.5). When the
    // caller sends an explicit cursor older than the oldest retained
    // mod_event_seq.seq, they've missed events that the cleanup job
    // pruned. Emit one OutdatedCursor frame and close cleanly so the
    // client knows to re-bootstrap via queryEvents.
    //
    // Only checked when a cursor was supplied. Callers who omit
    // cursor get the current tail (current_event_id below); there's
    // no gap to signal in that case.
    if let Some(client_cursor) = params.cursor {
        match oldest_available_event_seq(&ctx).await {
            Ok(Some(oldest)) if client_cursor < oldest - 1 => {
                let msg = SubscribeMessage::OutdatedCursor {
                    oldest_available_seq: oldest,
                    message: "cursor is older than the retention window; \
                              re-bootstrap via queryEvents and resubscribe \
                              with a fresh cursor",
                };
                let _ = send_msg(&mut sender, &msg).await;
                // Clean WebSocket close (1000) — frame has communicated
                // the issue, the close is graceful.
                let _ = sender
                    .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                        code: 1000,
                        reason: "outdated cursor".into(),
                    })))
                    .await;
                return;
            }
            // Empty table → any cursor is fine (nothing to deliver
            // until new events land). Or the cursor is within range.
            // Either way, fall through to normal flow.
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(
                    "subscribeModEvents: oldest_available_event_seq query failed: {}",
                    e
                );
                // Don't fail the subscription on the precheck — the
                // poll loop will surface the SQL error if it persists.
            }
        }
    }

    // Determine starting event cursor: caller-provided or current-tail.
    let mut event_cursor: i64 = match params.cursor {
        Some(c) => c,
        None => current_event_id(&ctx).await.unwrap_or(0),
    };

    // Audit-chain side. Only relevant when the caller opted in AND
    // their role permits chain visibility (silent gate per §3.6 —
    // we don't emit AuditEntry messages and we don't error).
    let chain_enabled = params.include_audit_chain && can_see_audit_chain(auth.role);
    let mut chain_cursor: i64 = if chain_enabled {
        match params.audit_chain_cursor {
            Some(c) => c,
            None => current_chain_sequence(&ctx).await.unwrap_or(0),
        }
    } else {
        0
    };

    // Hello uses the event cursor — that's the v0.2 wire shape and
    // existing clients depend on it. Chain cursor is internal state
    // resumed via the audit_chain_cursor query param.
    let hello = SubscribeMessage::Hello {
        instance_version: env!("CARGO_PKG_VERSION"),
        sequence: event_cursor,
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
                let messages = match collect_tick_messages(
                    &ctx,
                    &params,
                    event_cursor,
                    chain_cursor,
                    chain_enabled,
                ).await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("subscribeModEvents poll error: {}", e);
                        let err = SubscribeMessage::Error {
                            code: "Internal",
                            message: "event poll failed",
                        };
                        let _ = send_msg(&mut sender, &err).await;
                        return;
                    }
                };
                for msg in messages {
                    // Advance the appropriate cursor BEFORE attempting
                    // the send so a mid-stream disconnect doesn't
                    // re-deliver the same row on reconnect (the
                    // alternative — advance after send succeeds —
                    // would re-deliver one row per disconnect).
                    match &msg {
                        SubscribeMessage::Event { sequence, .. } => {
                            event_cursor = *sequence;
                        }
                        SubscribeMessage::AuditEntry { sequence, .. } => {
                            chain_cursor = *sequence;
                        }
                        _ => {}
                    }
                    if send_msg(&mut sender, &msg).await.is_err() {
                        return;
                    }
                    last_send = std::time::Instant::now();
                }
            }
            _ = heartbeat_tick.tick() => {
                if last_send.elapsed() >= Duration::from_secs(HEARTBEAT_INTERVAL_SECS) {
                    let hb = SubscribeMessage::Heartbeat { sequence: event_cursor };
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

/// Collect messages for a single poll tick: events plus optionally
/// audit-chain entries, merged in timestamp order with sequence as
/// tiebreaker. Pulled out as its own function so the SQL paths and
/// the merge logic stay separately testable from the WebSocket.
async fn collect_tick_messages(
    ctx: &AppContext,
    params: &SubscribeModEventsParams,
    after_event: i64,
    after_chain: i64,
    chain_enabled: bool,
) -> Result<Vec<SubscribeMessage<'static>>, sqlx::Error> {
    let events = fetch_new_events(ctx, after_event, params).await?;
    let chain_rows = if chain_enabled {
        fetch_new_chain_entries(ctx, after_chain).await?
    } else {
        Vec::new()
    };
    Ok(merge_event_and_chain_streams(events, chain_rows))
}

/// Merge two locally-ordered streams (events by id ASC, chain by
/// sequence ASC) into a single output in timestamp ascending order.
/// On equal timestamps, the event side wins — moderation_event rows
/// are typically written before their corresponding audit_chain_entry
/// rows in the same transaction, so this matches causal order.
fn merge_event_and_chain_streams(
    events: Vec<EventStreamRow>,
    chain_rows: Vec<AuditEntry>,
) -> Vec<SubscribeMessage<'static>> {
    let mut out: Vec<SubscribeMessage<'static>> =
        Vec::with_capacity(events.len() + chain_rows.len());
    let mut ei = 0;
    let mut ci = 0;
    while ei < events.len() || ci < chain_rows.len() {
        let take_event = match (events.get(ei), chain_rows.get(ci)) {
            (Some(e), Some(c)) => {
                // Compare DateTime<Utc> values directly to avoid
                // RFC3339 format mismatches between sources (Z vs
                // +00:00, etc.). Equal timestamps tip toward events
                // as documented above.
                e.created_at <= c.timestamp
            }
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_event {
            let e = events[ei].clone();
            out.push(SubscribeMessage::Event {
                event: e.payload,
                sequence: e.id,
            });
            ei += 1;
        } else {
            let entry = chain_rows[ci].clone();
            let sequence = entry.sequence;
            out.push(SubscribeMessage::AuditEntry {
                entry: Box::new(entry),
                sequence,
            });
            ci += 1;
        }
    }
    out
}

/// Owned moderation-event row used by the merge step. Timestamp
/// is parsed to `DateTime<Utc>` here rather than left as a string
/// so the merge step can compare against `ChainStreamRow.timestamp`
/// without RFC3339 format mismatches (`Z` vs `+00:00` etc.).
#[derive(Debug, Clone)]
struct EventStreamRow {
    id: i64,
    created_at: DateTime<Utc>,
    payload: serde_json::Value,
}

/// Current tail of the live subscription channel. Returns
/// `MAX(seq)` from `mod_event_seq` (the retention-bounded mirror,
/// chainlink #115), or 0 when the table is empty. Clients that
/// connect without a cursor resume from this value so they don't
/// re-receive any historical events.
async fn current_event_id(ctx: &AppContext) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT MAX(seq) AS max_seq FROM mod_event_seq")
        .fetch_optional(&ctx.account_db)
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<Option<i64>, _>("max_seq").ok().flatten())
        .unwrap_or(0))
}

/// Lowest `seq` currently retained in `mod_event_seq`. Returns
/// `None` when the table is empty (no events to compare against —
/// any cursor is fine because there's nothing to deliver yet).
/// Used by the OutdatedCursor detection path on connect: if the
/// caller's cursor is below the floor, they've missed pruned events.
async fn oldest_available_event_seq(ctx: &AppContext) -> Result<Option<i64>, sqlx::Error> {
    let row = sqlx::query("SELECT MIN(seq) AS min_seq FROM mod_event_seq")
        .fetch_optional(&ctx.account_db)
        .await?;
    Ok(row.and_then(|r| r.try_get::<Option<i64>, _>("min_seq").ok().flatten()))
}

async fn current_chain_sequence(ctx: &AppContext) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT MAX(sequence) AS max_seq FROM audit_chain_entry")
        .fetch_optional(&ctx.account_db)
        .await?;
    Ok(row
        .and_then(|r| r.try_get::<Option<i64>, _>("max_seq").ok().flatten())
        .unwrap_or(0))
}

/// Fetch chain entries with sequence > `after_seq`, in sequence-ascending
/// order, capped by `MAX_EVENTS_PER_POLL`. Returns `audit_chain::AuditEntry`
/// values directly so the subscribe wire shape matches getAuditTrail's
/// `items` exactly. Per-row hash recompute populates `verified`
/// (matches getAuditTrail's per-row primitive).
async fn fetch_new_chain_entries(
    ctx: &AppContext,
    after_seq: i64,
) -> Result<Vec<AuditEntry>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, sequence, created_at, actor_did, action, subject_did, subject_uri, \
                subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                cascade_subjects, cascade_snapshot_ids \
         FROM audit_chain_entry WHERE sequence > $1 ORDER BY sequence ASC LIMIT $2",
    )
    .bind(after_seq)
    .bind(MAX_EVENTS_PER_POLL)
    .fetch_all(&ctx.account_db)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let sequence: i64 = row.try_get("sequence")?;
        let created_at_str: String = row.try_get("created_at")?;
        let timestamp = match DateTime::parse_from_rfc3339(&created_at_str) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => continue, // skip unparseable rows defensively
        };
        let actor_did: String = row.try_get("actor_did")?;
        let action: String = row.try_get("action")?;
        let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
        let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
        let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
        let rationale: String = row.try_get("rationale")?;
        let snapshot_id: Option<i64> = row.try_get("snapshot_id").ok().flatten();
        let event_id: Option<i64> = row.try_get("event_id").ok().flatten();
        let current_hash: String = row.try_get("current_hash")?;
        let previous_hash: Option<String> = row.try_get("previous_hash").ok().flatten();
        let cascade_str: Option<String> = row.try_get("cascade_subjects").ok().flatten();
        let cascade_snapshot_ids_str: Option<String> =
            row.try_get("cascade_snapshot_ids").ok().flatten();
        // Parse on-disk numeric JSON for the wire field. The
        // verify_entry call below still receives the raw JSON
        // string because the canonical hash sees that form.
        // Mirrors the handler at aurora_admin.rs::get_audit_trail —
        // both consumer sites must populate cascade_snapshot_ids
        // identically or the subscribe-vs-handler wire shapes
        // diverge.
        let cascade_snapshot_ids_i64: Vec<Option<i64>> = cascade_snapshot_ids_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let cascade_snapshot_ids: Vec<Option<String>> = cascade_snapshot_ids_i64
            .iter()
            .map(|opt| opt.map(|v| v.to_string()))
            .collect();

        let verified = crate::admin::audit_chain::verify_entry(
            sequence,
            &created_at_str,
            &actor_did,
            &action,
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
            &rationale,
            snapshot_id,
            event_id,
            previous_hash.as_deref(),
            cascade_str.as_deref(),
            cascade_snapshot_ids_str.as_deref(),
            &current_hash,
        );

        let subject_ref = Subject::from_columns(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
        );
        let cascade_subjects: Vec<Subject> = cascade_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        out.push(AuditEntry {
            id: id.to_string(),
            sequence,
            timestamp,
            actor_did,
            action,
            subject_ref,
            rationale,
            snapshot_id: snapshot_id.map(|i| i.to_string()),
            event_id: event_id.map(|i| i.to_string()),
            current_hash,
            previous_hash,
            verified,
            cascade_subjects,
            cascade_snapshot_ids,
        });
    }
    Ok(out)
}

async fn fetch_new_events(
    ctx: &AppContext,
    after_seq: i64,
    params: &SubscribeModEventsParams,
) -> Result<Vec<EventStreamRow>, sqlx::Error> {
    // Source: mod_event_seq (the retention-bounded mirror, chainlink
    // #115). Read columns map 1:1 to the wire payload — `meta` is
    // not mirrored on this table because the wire format doesn't
    // carry it. Cursor is `seq`, distinct from `moderation_event.id`.
    //
    // Normalize the action filter once: an empty Vec is equivalent
    // to None, since `IN ()` is invalid SQL. Filtering at the
    // boundary means the SQL builder below sees only "no filter"
    // or "filter with N>=1 values."
    let action_filter: Option<&[String]> = params
        .action_filter
        .as_deref()
        .filter(|v| !v.is_empty());

    let mut clauses: Vec<String> = vec!["seq > ?".to_string()];
    let mut binds: Vec<String> = vec![after_seq.to_string()];
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
        // Build `action IN (?, ?, ?)` with one placeholder per
        // value. The renumbering loop below converts `?` → `$N`
        // uniformly, so we don't need a backend-specific code path.
        let placeholders: Vec<&str> = actions.iter().map(|_| "?").collect();
        clauses.push(format!("action IN ({})", placeholders.join(", ")));
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
        "SELECT seq, moderation_event_id, actor_did, action, subject_did, \
                subject_uri, subject_cid, detail, created_at \
         FROM mod_event_seq WHERE {} ORDER BY seq ASC LIMIT ${}",
        clauses_pg.join(" AND "),
        limit_idx
    );
    let mut q = sqlx::query(&sql);
    // First bind: after_seq (we wrote it as a string for uniform
    // binding; SQL coerces. Re-parse here as i64 to bind correctly.)
    q = q.bind(after_seq);
    // Skip the first bind in the binds Vec since we already bound after_seq above.
    for b in binds.iter().skip(1) {
        q = q.bind(b);
    }
    q = q.bind(MAX_EVENTS_PER_POLL);
    let rows = q.fetch_all(&ctx.account_db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let seq: i64 = row.try_get("seq")?;
        let moderation_event_id: i64 = row.try_get("moderation_event_id")?;
        let action: String = row.try_get("action")?;
        let actor_did: String = row.try_get("actor_did")?;
        let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
        let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
        let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
        let detail_str: Option<String> = row.try_get("detail").ok().flatten();
        let created_at_str: String = row.try_get("created_at")?;
        let created_at = match DateTime::parse_from_rfc3339(&created_at_str) {
            Ok(dt) => dt.with_timezone(&Utc),
            Err(_) => continue, // skip unparseable rows defensively
        };
        // Wire payload preserves the v0.2 shape callers depend on.
        // `id` is now the moderation_event row id (consistent with
        // what queryEvents returns); `seq` lives at the message
        // envelope level (`SubscribeMessage::Event.sequence`).
        let payload = serde_json::json!({
            "id": moderation_event_id,
            "eventType": action,
            "actorDid": actor_did,
            "subjectDid": subject_did,
            "subjectUri": subject_uri,
            "subjectCid": subject_cid,
            "details": detail_str
                .as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .unwrap_or(serde_json::Value::Null),
            "createdAt": created_at_str,
        });
        out.push(EventStreamRow {
            id: seq,
            created_at,
            payload,
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::audit_chain::{append_entry, AppendEntryParams};
    use crate::admin::defs::Subject;
    use crate::admin::roles::Role;

    // ---- Pure merge / role-gate tests (no DB) ----

    fn chain_row(seq: i64, ts: &str, action: &str) -> AuditEntry {
        AuditEntry {
            id: seq.to_string(),
            sequence: seq,
            timestamp: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            actor_did: "did:plc:m1".to_string(),
            action: action.to_string(),
            subject_ref: Some(Subject::Repo {
                did: "did:plc:s".to_string(),
            }),
            rationale: "test".to_string(),
            snapshot_id: None,
            event_id: None,
            current_hash: "h".to_string(),
            previous_hash: None,
            verified: true,
            cascade_subjects: Vec::new(),
            cascade_snapshot_ids: Vec::new(),
        }
    }

    fn event_row(id: i64, ts: &str) -> EventStreamRow {
        EventStreamRow {
            id,
            created_at: DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&Utc),
            payload: serde_json::json!({ "id": id, "createdAt": ts }),
        }
    }

    fn message_kind(msg: &SubscribeMessage<'static>) -> &'static str {
        match msg {
            SubscribeMessage::Event { .. } => "event",
            SubscribeMessage::AuditEntry { .. } => "auditEntry",
            SubscribeMessage::Hello { .. } => "hello",
            SubscribeMessage::Heartbeat { .. } => "heartbeat",
            SubscribeMessage::OutdatedCursor { .. } => "outdatedCursor",
            SubscribeMessage::Error { .. } => "error",
        }
    }

    #[test]
    fn can_see_audit_chain_gate_matches_get_audit_trail() {
        // §8.4 getAuditTrail requires Moderator+. The streaming gate
        // mirrors it exactly. If §8.4 ever tightens, this test will
        // notice as soon as the helper is updated.
        assert!(can_see_audit_chain(Role::Moderator));
        assert!(can_see_audit_chain(Role::Admin));
        assert!(can_see_audit_chain(Role::SuperAdmin));
    }

    #[test]
    fn merge_orders_by_timestamp_with_event_tiebreaker() {
        // Events @ T+0 and T+2; chain entries @ T+1 and T+2.
        // Expected order:
        //   event(id=1, t+0)
        //   chain(seq=10, t+1)
        //   event(id=2, t+2)        ← tiebreak prefers event
        //   chain(seq=11, t+2)
        let events = vec![
            event_row(1, "2026-05-04T00:00:00Z"),
            event_row(2, "2026-05-04T00:00:02Z"),
        ];
        let chain_rows = vec![
            chain_row(10, "2026-05-04T00:00:01Z", "TakedownAccount"),
            chain_row(11, "2026-05-04T00:00:02Z", "RestoreAccount"),
        ];
        let merged = merge_event_and_chain_streams(events, chain_rows);
        let kinds: Vec<&'static str> = merged.iter().map(message_kind).collect();
        assert_eq!(kinds, ["event", "auditEntry", "event", "auditEntry"]);
    }

    #[test]
    fn merge_handles_chain_only_stream() {
        let merged = merge_event_and_chain_streams(
            vec![],
            vec![
                chain_row(1, "2026-05-04T00:00:00Z", "A"),
                chain_row(2, "2026-05-04T00:00:01Z", "B"),
            ],
        );
        let kinds: Vec<_> = merged.iter().map(message_kind).collect();
        assert_eq!(kinds, ["auditEntry", "auditEntry"]);
    }

    #[test]
    fn merge_handles_event_only_stream() {
        let merged = merge_event_and_chain_streams(
            vec![event_row(1, "2026-05-04T00:00:00Z")],
            vec![],
        );
        let kinds: Vec<_> = merged.iter().map(message_kind).collect();
        assert_eq!(kinds, ["event"]);
    }

    #[test]
    fn audit_entry_serializes_with_wrapped_entry_field_per_spec() {
        // §8.5: AuditEntry { entry: AuditEntry, sequence: u64 } —
        // entry payload is the same shape as getAuditTrail's items,
        // including id / subjectRef / snapshotId / eventId /
        // cascadeSubjects. Pinning the wrapped shape so future
        // refactors don't silently flatten or drop fields.
        let msg = SubscribeMessage::AuditEntry {
            entry: Box::new(AuditEntry {
                id: "100".to_string(),
                sequence: 42,
                timestamp: DateTime::parse_from_rfc3339("2026-05-04T00:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                actor_did: "did:plc:m1".to_string(),
                action: "TakedownAccount".to_string(),
                subject_ref: Some(Subject::Repo {
                    did: "did:plc:s".to_string(),
                }),
                rationale: "spam".to_string(),
                snapshot_id: Some("7".to_string()),
                event_id: Some("13".to_string()),
                current_hash: "h".to_string(),
                previous_hash: Some("p".to_string()),
                verified: true,
                cascade_subjects: Vec::new(),
                cascade_snapshot_ids: Vec::new(),
            }),
            sequence: 42,
        };
        let json = serde_json::to_string(&msg).unwrap();
        // Envelope.
        assert!(json.contains("\"$type\":\"auditEntry\""));
        assert!(json.contains("\"sequence\":42"));
        // Wrapped entry (camelCase per audit_chain::AuditEntry).
        assert!(json.contains("\"entry\":{"));
        assert!(json.contains("\"id\":\"100\""));
        assert!(json.contains("\"actorDid\":\"did:plc:m1\""));
        assert!(json.contains("\"currentHash\":\"h\""));
        assert!(json.contains("\"previousHash\":\"p\""));
        assert!(json.contains("\"snapshotId\":\"7\""));
        assert!(json.contains("\"eventId\":\"13\""));
        assert!(json.contains("\"verified\":true"));
        assert!(json.contains("\"cascadeSubjects\":[]"));
        // subject_ref present and wrapped per Subject's $type
        // discriminator.
        assert!(json.contains("\"subjectRef\":{"));
        assert!(json.contains("\"$type\":\"com.atproto.admin.defs#repoRef\""));
        assert!(json.contains("\"did\":\"did:plc:s\""));
    }

    #[test]
    fn audit_entry_wire_shape_matches_get_audit_trail_items() {
        // Cross-surface parity (§8.5 commit): the JSON shape for
        // `entry` here must equal the JSON shape getAuditTrail emits
        // per-item. Pinning by serializing one AuditEntry and
        // comparing against the entry-extracted JSON from a wrapped
        // SubscribeMessage::AuditEntry.
        let entry = AuditEntry {
            id: "100".to_string(),
            sequence: 42,
            timestamp: DateTime::parse_from_rfc3339("2026-05-04T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            actor_did: "did:plc:m1".to_string(),
            action: "TakedownAccount".to_string(),
            subject_ref: Some(Subject::Repo {
                did: "did:plc:s".to_string(),
            }),
            rationale: "spam".to_string(),
            snapshot_id: None,
            event_id: None,
            current_hash: "h".to_string(),
            previous_hash: None,
            verified: true,
            cascade_subjects: Vec::new(),
            cascade_snapshot_ids: Vec::new(),
        };
        let standalone = serde_json::to_value(&entry).unwrap();
        let wrapped = serde_json::to_value(&SubscribeMessage::AuditEntry {
            entry: Box::new(entry),
            sequence: 42,
        })
        .unwrap();
        assert_eq!(
            wrapped["entry"], standalone,
            "subscribe entry payload must equal getAuditTrail's per-item shape"
        );
    }

    /// Arc 9 Step 2 / Item 8: the manual `Debug` impl on
    /// `AppContext` must redact every secret-bearing field
    /// (jwt_secret, repo signing key, PLC rotation key, SMTP
    /// creds, S3 secret_access_key). `create_test_context`
    /// constructs a context with well-known sentinel values; the
    /// Debug output must not contain any of them. Future changes
    /// to the impl that drop a redaction will fail this test.
    #[tokio::test]
    async fn app_context_debug_redacts_sensitive_fields() {
        let ctx = create_test_context().await;
        let rendered = format!("{:?}", ctx);
        // jwt_secret literal from create_test_context.
        assert!(
            !rendered.contains("test-secret-key-aurora-subscribe-32xx"),
            "AppContext Debug leaked jwt_secret: {}",
            rendered
        );
        // repo_signing_key / plc_rotation_key — 64-char sentinels.
        assert!(
            !rendered.contains(&"a".repeat(64)),
            "AppContext Debug leaked repo_signing_key"
        );
        assert!(
            !rendered.contains(&"b".repeat(64)),
            "AppContext Debug leaked plc_rotation_key"
        );
        // Shape sanity: the redacted placeholder text appears.
        assert!(
            rendered.contains("<redacted: ServerConfig>"),
            "AppContext Debug missing redacted-config placeholder: {}",
            rendered
        );
    }

    // ---- DB-backed tests for fetch_new_chain_entries + collect_tick ----
    //
    // Mirror create_test_context's pattern from aurora_admin's tests
    // module; using AppContext directly keeps the SQL paths exercised
    // end-to-end against a real (in-memory) sqlite instance.

    async fn create_test_context() -> AppContext {
        use crate::config::*;
        use std::path::PathBuf;
        use tempfile::tempdir;
        let dir = tempdir().unwrap().keep();
        let db_path = dir.join("test.db");
        let config = ServerConfig {
            service: ServiceConfig {
                hostname: "localhost".to_string(),
                port: 2583,
                service_did: "did:web:localhost".to_string(),
                version: "0.1.0-test".to_string(),
                blob_upload_limit: 5_242_880,
                public_url: None,
                max_blob_fetch_size: 50_000_000,
                blob_fetch_timeout_seconds: 30,
                blob_fetch_max_retries: 3,
                accepting_imports: true,
                max_import_size: None,
            },
            storage: StorageConfig {
                data_directory: dir.clone(),
                account_db: db_path.clone(),
                sequencer_db: dir.join("sequencer.db"),
                did_cache_db: dir.join("did_cache.db"),
                actor_store_directory: dir.join("actors"),
                blobstore: BlobstoreConfig::Disk {
                    location: dir.join("blobs"),
                    tmp_location: dir.join("temp"),
                },
            },
            database: Default::default(),
            authentication: AuthConfig {
                jwt_secret: "test-secret-key-aurora-subscribe-32xx".to_string(),
                repo_signing_key: "a".repeat(64),
                plc_rotation_key: "b".repeat(64),
                oauth: OAuthConfig {
                    client_id: "http://localhost:3000/client-metadata.json".to_string(),
                    redirect_uri: "http://localhost:3000/oauth/callback".to_string(),
                    pds_url: "https://bsky.social".to_string(),
                },
                jwt_sunset_date: "Sat, 31 Dec 2024 23:59:59 GMT".to_string(),
                oauth_migration_guide_url:
                    "https://docs.atproto.com/guides/oauth-migration".to_string(),
                oauth_features: Default::default(),
            },
            identity: IdentityConfig {
                did_plc_url: "https://plc.directory".to_string(),
                service_handle_domains: vec![".localhost".to_string()],
                did_cache_stale_ttl: 3600,
                did_cache_max_ttl: 86400,
                recovery_did_key: None,
            },
            email: None,
            invites: InviteConfig {
                required: false,
                interval: 604800,
                epoch: "2024-01-01T00:00:00Z".to_string(),
            },
            rate_limit: RateLimitConfig {
                enabled: false,
                global_requests_per_minute: 3000,
                exempt_admin_assets: true,
            },
            logging: LoggingConfig {
                level: "info".to_string(),
            },
            federation: FederationConfig {
                enabled: false,
                relay_urls: vec![],
                appview_url: None,
                firehose_enabled: false,
                crawl_enabled: false,
                public_url: Some("http://localhost:2583".to_string()),
                auto_stream_events: false,
                peer_pds: vec![],
            },
            validation_mode: PathBuf::from("required")
                .into_os_string()
                .to_string_lossy()
                .parse()
                .unwrap_or(crate::validation::ValidationMode::Required),
            distributed_state_mode: Default::default(),
            maintenance_pool: Default::default(),
            gc_sweep: Default::default(),
            blob_metadata: Default::default(),
            entryway: None,
        };
        AppContext::new(
            config,
            std::sync::Arc::new(crate::api::registry::RouteRegistry::default()),
        )
        .await
        .unwrap()
    }

    async fn write_test_chain_entry(ctx: &AppContext, action: &'static str) -> i64 {
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        append_entry(
            &ctx.account_db,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action,
                subject: Some(&subject),
                rationale: "test rationale",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn fetch_new_chain_entries_returns_rows_after_cursor() {
        let ctx = create_test_context().await;
        write_test_chain_entry(&ctx, "TakedownAccount").await;
        write_test_chain_entry(&ctx, "RestoreAccount").await;
        let rows = fetch_new_chain_entries(&ctx, 0).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].action, "TakedownAccount");
        assert_eq!(rows[1].action, "RestoreAccount");
        assert!(rows[0].verified, "fresh chain entry must verify");
    }

    #[tokio::test]
    async fn fetch_new_chain_entries_resumes_from_cursor() {
        let ctx = create_test_context().await;
        write_test_chain_entry(&ctx, "TakedownAccount").await;
        write_test_chain_entry(&ctx, "RestoreAccount").await;
        // Resume past the first row → should only see the second.
        let rows = fetch_new_chain_entries(&ctx, 1).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sequence, 2);
        assert_eq!(rows[0].action, "RestoreAccount");
    }

    #[tokio::test]
    async fn collect_tick_emits_audit_entries_when_chain_enabled() {
        let ctx = create_test_context().await;
        write_test_chain_entry(&ctx, "TakedownAccount").await;
        let params = SubscribeModEventsParams {
            include_audit_chain: true,
            ..Default::default()
        };
        let messages = collect_tick_messages(&ctx, &params, 0, 0, true)
            .await
            .unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(message_kind(&messages[0]), "auditEntry");
    }

    #[tokio::test]
    async fn collect_tick_omits_audit_entries_when_chain_disabled() {
        let ctx = create_test_context().await;
        write_test_chain_entry(&ctx, "TakedownAccount").await;
        // include_audit_chain: false → chain_enabled gets set false
        // upstream → collect_tick must skip the chain fetch entirely.
        let params = SubscribeModEventsParams {
            include_audit_chain: false,
            ..Default::default()
        };
        let messages = collect_tick_messages(&ctx, &params, 0, 0, false)
            .await
            .unwrap();
        assert!(
            messages.is_empty(),
            "no events written, chain disabled → empty tick"
        );
    }

    #[tokio::test]
    async fn collect_tick_silently_omits_audit_entries_for_insufficient_role() {
        // Per §3.6, an operator who passes include_audit_chain=true
        // but whose role doesn't satisfy can_see_audit_chain MUST
        // get a stream with no AuditEntry messages and no error.
        // The handler enforces this by setting chain_enabled = false
        // when can_see_audit_chain returns false. That path is what
        // we exercise here: chain_enabled=false even though the
        // params asked for chain.
        let ctx = create_test_context().await;
        write_test_chain_entry(&ctx, "TakedownAccount").await;
        let params = SubscribeModEventsParams {
            include_audit_chain: true,
            ..Default::default()
        };
        let messages = collect_tick_messages(&ctx, &params, 0, 0, false)
            .await
            .unwrap();
        // Chain entries written; chain_enabled passed false (mimicking
        // the role-gate having fired) → still no AuditEntry messages.
        assert!(messages.iter().all(|m| message_kind(m) != "auditEntry"));
    }

    #[tokio::test]
    async fn collect_tick_resume_cursor_does_not_redeliver_chain_entries() {
        let ctx = create_test_context().await;
        write_test_chain_entry(&ctx, "TakedownAccount").await;
        write_test_chain_entry(&ctx, "RestoreAccount").await;
        let params = SubscribeModEventsParams {
            include_audit_chain: true,
            ..Default::default()
        };
        // First tick from cursor=0 → both rows.
        let first_tick = collect_tick_messages(&ctx, &params, 0, 0, true).await.unwrap();
        assert_eq!(first_tick.len(), 2);
        // Resume from chain_cursor=1 (after first row) → only second row.
        let resume = collect_tick_messages(&ctx, &params, 0, 1, true).await.unwrap();
        assert_eq!(resume.len(), 1);
        match &resume[0] {
            SubscribeMessage::AuditEntry { entry, sequence } => {
                assert_eq!(*sequence, 2);
                assert_eq!(entry.sequence, 2);
                assert_eq!(entry.action, "RestoreAccount");
            }
            other => panic!("expected AuditEntry, got {:?}", message_kind(other)),
        }
    }

    // ---- chainlink #115 commit 3: read-source migration tests ----

    /// Insert a `mod_event_seq` row directly. Bypasses
    /// `insert_moderation_event_in_tx` deliberately — the read-source
    /// tests need controlled fixture rows independent of the dual-
    /// write helper. Returns the inserted seq.
    async fn write_seq_row_only(
        ctx: &AppContext,
        action: &str,
        actor_did: &str,
        subject_did: Option<&str>,
        created_at: chrono::DateTime<Utc>,
    ) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO mod_event_seq \
             (moderation_event_id, actor_did, action, subject_did, detail, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING seq",
        )
        .bind(1_i64)
        .bind(actor_did)
        .bind(action)
        .bind(subject_did)
        .bind("{}")
        .bind(created_at.to_rfc3339())
        .fetch_one(&ctx.account_db)
        .await
        .unwrap()
    }

    /// Insert directly into `moderation_event` (bypassing the dual-
    /// write helper) — used by the migration sanity test to confirm
    /// the subscription handler reads from `mod_event_seq` and NOT
    /// from `moderation_event`.
    async fn write_moderation_event_only(
        ctx: &AppContext,
        event_type: &str,
        actor_did: &str,
    ) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO moderation_event \
             (event_type, actor_did, details, created_at) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(event_type)
        .bind(actor_did)
        .bind("{}")
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&ctx.account_db)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn fetch_new_events_reads_from_mod_event_seq_not_moderation_event() {
        // Migration sanity (chainlink #115 commit 3): if the handler
        // reads from mod_event_seq, then a row that lands in
        // moderation_event ONLY (bypassing the dual-write) must NOT
        // be delivered to subscribers. This is what pins the
        // read-source migration — pre-fix the test would fail because
        // moderation_event is the source.
        let ctx = create_test_context().await;
        // moderation_event has 1 row; mod_event_seq has 0.
        write_moderation_event_only(&ctx, "TakedownAccount", "did:plc:m1").await;
        let params = SubscribeModEventsParams::default();
        let events = fetch_new_events(&ctx, 0, &params).await.unwrap();
        assert_eq!(
            events.len(),
            0,
            "subscription must not deliver moderation_event-only rows"
        );

        // Now write through the canonical path → both tables.
        let logger = crate::admin::events::ModerationEventLogger::new(ctx.account_db.clone());
        logger
            .log_event(crate::admin::events::LogEventParams {
                event_type: crate::admin::events::ModerationEventType::AccountSuspend,
                actor_did: "did:plc:m1",
                subject_did: Some("did:plc:s1"),
                subject_uri: None,
                subject_cid: None,
                details: serde_json::json!({"reason": "test"}),
                meta: None,
            })
            .await
            .unwrap();
        // After dual-write, fetch sees the row.
        let events = fetch_new_events(&ctx, 0, &params).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].payload["eventType"], "account_suspend");
    }

    #[tokio::test]
    async fn outdated_cursor_detected_when_below_oldest_retained_seq() {
        // Populate mod_event_seq with seq=1..3 (an older window),
        // then simulate cleanup having removed seq=1..2 by deleting
        // those rows. Subscribe with cursor=0 (older than the oldest
        // remaining seq). The detection helper must see the gap.
        let ctx = create_test_context().await;
        let now = Utc::now();
        for i in 0..3 {
            write_seq_row_only(
                &ctx,
                "TakedownAccount",
                "did:plc:m1",
                Some("did:plc:s"),
                now - chrono::Duration::seconds(10 - i),
            )
            .await;
        }
        // Simulate retention cleanup pruning the first two rows.
        sqlx::query("DELETE FROM mod_event_seq WHERE seq <= 2")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let oldest = oldest_available_event_seq(&ctx).await.unwrap();
        assert_eq!(oldest, Some(3), "after pruning seq<=2, MIN(seq) is 3");

        // Cursor 0 < oldest(3) - 1 → outdated.
        let client_cursor = 0i64;
        assert!(
            client_cursor < oldest.unwrap() - 1,
            "cursor 0 < 3-1=2 → outdated"
        );

        // Cursor 2 == oldest(3) - 1 → still safe (the next row to
        // deliver is seq=3 which exists). The OutdatedCursor frame
        // detection condition is `client_cursor < oldest - 1` exactly.
        let client_cursor = 2i64;
        assert!(
            !(client_cursor < oldest.unwrap() - 1),
            "cursor 2 == oldest-1 is the boundary; not outdated"
        );
    }

    #[tokio::test]
    async fn outdated_cursor_skipped_when_mod_event_seq_empty() {
        // Edge case (chainlink #115 commit 3): empty table → any
        // cursor is fine because there's nothing to deliver until
        // new events land. The detection helper returns None; the
        // handler falls through to normal subscription flow.
        let ctx = create_test_context().await;
        let oldest = oldest_available_event_seq(&ctx).await.unwrap();
        assert_eq!(
            oldest, None,
            "empty mod_event_seq must yield None oldest_available_seq"
        );
        // Confirm fetch returns zero rows so the subscription is a
        // pure heartbeat-only stream.
        let events = fetch_new_events(&ctx, 12345, &SubscribeModEventsParams::default())
            .await
            .unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn outdated_cursor_serializes_with_camel_case_fields() {
        // Wire format pin (chainlink #115 commit 3): the
        // OutdatedCursor frame uses the §8.5 shape exactly so admin
        // UI clients can parse it without case-folding fallbacks.
        let msg = SubscribeMessage::OutdatedCursor {
            oldest_available_seq: 42,
            message: "cursor older than retention window",
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"$type\":\"outdatedCursor\""));
        assert!(json.contains("\"oldestAvailableSeq\":42"));
        assert!(json.contains("\"message\":\"cursor older than retention window\""));
    }
}
