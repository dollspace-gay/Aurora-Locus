//! `com.atproto.sync.subscribeRepos`: the firehose.
//!
//! A subscriber with a cursor is backfilled from the log in pages, then
//! switched to the live stream (deduplicated by seq). A cursor ahead of the
//! log gets `FutureCursor`; one behind retention gets `#info
//! OutdatedCursor` and continues from the oldest retained event. A
//! subscriber that falls too far behind live gets `ConsumerTooSlow`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::state::AppState;

type St = State<Arc<AppState>>;

const PAGE: i64 = 500;

#[derive(Deserialize)]
pub struct SubscribeParams {
    cursor: Option<i64>,
}

pub async fn subscribe_repos(
    State(s): St,
    Query(p): Query<SubscribeParams>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| serve(s, socket, p.cursor))
}

async fn serve(s: Arc<AppState>, socket: WebSocket, cursor: Option<i64>) {
    let (mut tx, mut rx) = socket.split();
    let send = |bytes: Vec<u8>| Message::Binary(bytes.into());
    // Subscribe before reading the log, so nothing falls between.
    let mut live = s.firehose.subscribe();
    let store = s.firehose.store();
    let Ok(state) = store.seq_state().await else {
        return;
    };
    let mut pos = match cursor {
        None => state.last_seq,
        Some(c) if c > state.last_seq => {
            let _ = tx
                .send(send(locus_sync::error_frame(
                    "FutureCursor",
                    "Cursor in the future.",
                )))
                .await;
            let _ = tx.close().await;
            return;
        }
        Some(c) => c,
    };
    // Backfill, checking retention before each page.
    if cursor.is_some() {
        loop {
            let Ok(state) = store.seq_state().await else {
                return;
            };
            if pos < state.pruned_through {
                let msg = "Requested cursor exceeded limit. Possibly missing events";
                if tx
                    .send(send(locus_sync::info_frame("OutdatedCursor", msg)))
                    .await
                    .is_err()
                {
                    return;
                }
                pos = state.pruned_through;
            }
            let Ok(page) = store.seq_events_after(pos, PAGE).await else {
                return;
            };
            let full = page.len() == PAGE as usize;
            for ev in &page {
                let Ok(f) = locus_sync::frame(ev) else {
                    continue;
                };
                if tx.send(send(f.bytes)).await.is_err() {
                    return;
                }
                pos = ev.seq;
            }
            if !full {
                break;
            }
        }
    }
    let ping_every = Duration::from_secs(
        u64::try_from(s.snapshot().int("sync.ws_ping_interval_s")).unwrap_or(30),
    );
    let mut ping = tokio::time::interval(ping_every);
    ping.tick().await;
    loop {
        tokio::select! {
            ev = live.recv() => match ev {
                Ok(ev) => {
                    if ev.seq <= pos {
                        continue;
                    }
                    if tx.send(send(ev.bytes.clone())).await.is_err() {
                        return;
                    }
                    pos = ev.seq;
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = tx.send(send(locus_sync::error_frame("ConsumerTooSlow", "Stream consumer too slow"))).await;
                    let _ = tx.close().await;
                    return;
                }
                Err(RecvError::Closed) => return,
            },
            _ = ping.tick() => {
                if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return;
                }
            }
            msg = rx.next() => match msg {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            },
        }
    }
}
