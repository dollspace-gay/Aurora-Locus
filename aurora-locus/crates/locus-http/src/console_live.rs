//! Live refresh for the console's status pages: `GET /admin/live?page=`
//! is a server-sent event stream that says `change` when the page would
//! render differently, and the page's one script then fetches it again
//! and swaps in its live parts. The stream is signed in like the page
//! (the console cookie), re-checks the sign-in on every tick and ends
//! with it, sends a keepalive comment while nothing changes, ends after
//! a while (the browser reconnects), and is bounded per sign-in and per
//! node. Without the script the pages are the same, refreshed by reloading.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use locus_web::console::{Need, PageId};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::state::AppState;

#[cfg(not(test))]
const TICK: Duration = Duration::from_secs(5);
#[cfg(test)]
const TICK: Duration = Duration::from_millis(100);
#[cfg(not(test))]
const KEEPALIVE: Duration = Duration::from_secs(20);
#[cfg(test)]
const KEEPALIVE: Duration = Duration::from_millis(400);
#[cfg(not(test))]
const LIFETIME: Duration = Duration::from_secs(15 * 60);
#[cfg(test)]
const LIFETIME: Duration = Duration::from_secs(4);
/// Streams one sign-in may hold open (a few tabs).
pub const PER_SESSION: usize = 4;
/// Streams one node holds open.
pub const PER_NODE: usize = 64;
/// How long the browser waits before reconnecting, in ms.
const RETRY_MS: u64 = 5000;

/// Open streams: by sign-in, and in all.
static OPEN: LazyLock<Mutex<(HashMap<String, usize>, usize)>> =
    LazyLock::new(|| Mutex::new((HashMap::new(), 0)));

/// A held stream slot, given back when the stream ends.
struct Slot(String);

impl Slot {
    fn take(key: &str) -> Option<Self> {
        let mut open = OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mine = open.0.get(key).copied().unwrap_or(0);
        if mine >= PER_SESSION || open.1 >= PER_NODE {
            return None;
        }
        open.0.insert(key.to_owned(), mine + 1);
        open.1 += 1;
        Some(Self(key.to_owned()))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut open = OPEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(n) = open.0.get_mut(&self.0) {
            *n -= 1;
            if *n == 0 {
                open.0.remove(&self.0);
            }
        }
        open.1 = open.1.saturating_sub(1);
    }
}

#[derive(Deserialize)]
pub struct LiveQuery {
    #[serde(default)]
    page: String,
}

/// A page as the operator would get it now: its `<main>`'s hash, or None
/// when they no longer may (signed out, or it isn't a page any more).
async fn digest(s: &Arc<AppState>, headers: &HeaderMap, page: PageId) -> Option<String> {
    let st = State(Arc::clone(s));
    let h = headers.clone();
    let res = match page {
        PageId::Home => crate::console::home(st, h).await,
        PageId::Jobs => crate::console_sections::jobs(st, h, Query(Default::default())).await,
        PageId::Storage => crate::console_sections::storage(st, h).await,
        PageId::Services => {
            crate::console_sections::services(st, h, Query(Default::default())).await
        }
        PageId::Hostname => {
            crate::console_hostname::page(st, h, Query(Default::default())).await
        }
        _ => return None,
    };
    if res.status() != StatusCode::OK {
        return None;
    }
    let body = axum::body::to_bytes(res.into_body(), 8 << 20).await.ok()?;
    let html = String::from_utf8_lossy(&body);
    let main = html
        .find("<main")
        .and_then(|a| html[a..].find("</main>").map(|b| &html[a..a + b]))
        .unwrap_or(&html);
    Some(hex::encode(&Sha256::digest(main.as_bytes())[..12]))
}

/// What the stream carries between ticks.
struct Stream {
    s: Arc<AppState>,
    headers: HeaderMap,
    page: PageId,
    last: String,
    started: Instant,
    sent: Instant,
    first: bool,
    done: bool,
    _slot: Slot,
}

async fn next(mut st: Stream) -> Option<(Result<Bytes, Infallible>, Stream)> {
    if st.done {
        return None;
    }
    if st.first {
        st.first = false;
        st.sent = Instant::now();
        let hello = format!("retry: {RETRY_MS}\n: live\n\n");
        return Some((Ok(Bytes::from(hello)), st));
    }
    loop {
        tokio::time::sleep(TICK).await;
        if st.started.elapsed() >= LIFETIME {
            // The browser reconnects, signed in again from scratch.
            st.done = true;
            return Some((Ok(Bytes::from_static(b": bye\n\n")), st));
        }
        let Some(d) = digest(&st.s, &st.headers, st.page).await else {
            st.done = true;
            return Some((Ok(Bytes::from_static(b"event: end\ndata: signed-out\n\n")), st));
        };
        if d != st.last {
            st.last.clone_from(&d);
            st.sent = Instant::now();
            return Some((Ok(Bytes::from(format!("event: change\ndata: {d}\n\n"))), st));
        }
        if st.sent.elapsed() >= KEEPALIVE {
            st.sent = Instant::now();
            return Some((Ok(Bytes::from_static(b": keepalive\n\n")), st));
        }
    }
}

/// `GET /admin/live?page=/admin/jobs`.
pub async fn live(
    State(s): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<LiveQuery>,
) -> Response {
    let Some(op) = crate::console::signed_in(&s, &headers).await else {
        return (StatusCode::UNAUTHORIZED, "Sign in to the console first").into_response();
    };
    let Some(page) = PageId::at(&q.page).filter(|p| p.live()) else {
        return (StatusCode::NOT_FOUND, "Not a live page").into_response();
    };
    if page.page().need > Need::from_role(&op.row.role) {
        return (StatusCode::FORBIDDEN, "That page needs a higher role").into_response();
    }
    let Some(slot) = Slot::take(&crate::console::session_hash(&op.id)) else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many live pages are open; close some",
        )
            .into_response();
    };
    let Some(last) = digest(&s, &headers, page).await else {
        return (StatusCode::UNAUTHORIZED, "Sign in to the console first").into_response();
    };
    let stream = Stream {
        s,
        headers,
        page,
        last,
        started: Instant::now(),
        sent: Instant::now(),
        first: true,
        done: false,
        _slot: slot,
    };
    let mut res = Body::from_stream(futures::stream::unfold(stream, next)).into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    res
}
