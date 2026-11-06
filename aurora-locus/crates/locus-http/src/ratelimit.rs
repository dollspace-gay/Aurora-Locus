//! Rate limiting, as the reference: a global per-IP bucket on every XRPC
//! request (`getRepo` has its own), per-endpoint buckets keyed by IP, DID
//! or login identifier, and shared repo-write buckets with point costs.
//! On by default **[deviation]** (the reference's package default is off).
//! Counters are fixed windows, kept in process or in the database (shared
//! by every node). Responses carry `RateLimit-*` headers from the bucket
//! with the fewest points left; a refusal is 429 `RateLimitExceeded` with
//! `Retry-After`.
//!
//! Also: the client's real IP (forwarded headers honored only from trusted
//! proxies) and the firehose's per-IP and total subscriber limits.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::{XrpcError, XrpcResult};
use crate::state::AppState;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// One bucket: a name (the key prefix), a window and its points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bucket {
    pub name: &'static str,
    pub window_ms: i64,
    pub points: i64,
}

/// The reference's per-endpoint buckets.
pub mod buckets {
    use super::{Bucket, DAY, HOUR, MINUTE};

    pub const CREATE_SESSION_DAY: Bucket = Bucket {
        name: "com.atproto.server.createSession-0",
        window_ms: DAY,
        points: 300,
    };
    pub const CREATE_SESSION_5M: Bucket = Bucket {
        name: "com.atproto.server.createSession-1",
        window_ms: 5 * MINUTE,
        points: 30,
    };
    pub const CREATE_ACCOUNT: Bucket = Bucket {
        name: "com.atproto.server.createAccount-0",
        window_ms: 5 * MINUTE,
        points: 100,
    };
    /// Per IP **[deviation]** (the reference doesn't limit it).
    pub const RESERVE_SIGNING_KEY: Bucket = Bucket {
        name: "com.atproto.server.reserveSigningKey-0",
        window_ms: 5 * MINUTE,
        points: 100,
    };
    pub const DELETE_ACCOUNT: Bucket = Bucket {
        name: "com.atproto.server.deleteAccount-0",
        window_ms: 5 * MINUTE,
        points: 50,
    };
    pub const RESET_PASSWORD: Bucket = Bucket {
        name: "com.atproto.server.resetPassword-0",
        window_ms: 5 * MINUTE,
        points: 50,
    };
    pub const REQUEST_PASSWORD_RESET_DAY: Bucket = Bucket {
        name: "com.atproto.server.requestPasswordReset-0",
        window_ms: DAY,
        points: 50,
    };
    pub const REQUEST_PASSWORD_RESET_HOUR: Bucket = Bucket {
        name: "com.atproto.server.requestPasswordReset-1",
        window_ms: HOUR,
        points: 15,
    };
    pub const UPDATE_HANDLE_5M: Bucket = Bucket {
        name: "com.atproto.identity.updateHandle-0",
        window_ms: 5 * MINUTE,
        points: 10,
    };
    pub const UPDATE_HANDLE_DAY: Bucket = Bucket {
        name: "com.atproto.identity.updateHandle-1",
        window_ms: DAY,
        points: 50,
    };
    pub const UPLOAD_BLOB: Bucket = Bucket {
        name: "com.atproto.repo.uploadBlob-0",
        window_ms: DAY,
        points: 1000,
    };
    pub const GET_REPO: Bucket = Bucket {
        name: "com.atproto.sync.getRepo-0",
        window_ms: 5 * MINUTE,
        points: 6000,
    };
    /// Console sign-in attempts, per address.
    pub const CONSOLE_SIGN_IN: Bucket = Bucket {
        name: "locus.console.signIn-ip",
        window_ms: 5 * MINUTE,
        points: 30,
    };
    /// Console sign-in attempts (passwords and codes), per operator name.
    pub const CONSOLE_SIGN_IN_NAME: Bucket = Bucket {
        name: "locus.console.signIn-name",
        window_ms: 5 * MINUTE,
        points: 10,
    };

    /// `requestEmailConfirmation`, `requestEmailUpdate` and
    /// `requestAccountDelete` share this shape (each its own key prefix).
    #[must_use]
    pub const fn email_day(name: &'static str) -> Bucket {
        Bucket {
            name,
            window_ms: DAY,
            points: 15,
        }
    }
    #[must_use]
    pub const fn email_hour(name: &'static str) -> Bucket {
        Bucket {
            name,
            window_ms: HOUR,
            points: 5,
        }
    }
}

/// A bucket's state after a consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub limit: i64,
    pub window_ms: i64,
    pub remaining: i64,
    pub ms_before_next: i64,
    pub exceeded: bool,
}

/// The node's counters (in-process backend) and subscriber counts.
#[derive(Default)]
pub struct Limiter {
    counters: Mutex<HashMap<String, (i64, i64)>>,
    subscribers: Mutex<HashMap<IpAddr, usize>>,
}

impl Limiter {
    fn consume_memory(&self, key: &str, points: i64, window_ms: i64, now: i64) -> (i64, i64) {
        let mut map = self
            .counters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() > 200_000 {
            map.retain(|_, (end, _)| *end > now);
        }
        let entry = map.entry(key.to_owned()).or_insert((now + window_ms, 0));
        if entry.0 <= now {
            *entry = (now + window_ms, 0);
        }
        entry.1 += points;
        (entry.1, entry.0)
    }
}

/// Whether limits apply to this request (on, and not bypassed).
fn bypassed(s: &AppState, headers: &HeaderMap, ip: Option<IpAddr>) -> bool {
    let snap = s.snapshot();
    if !snap.bool("rate_limits.enabled") {
        return true;
    }
    let key = snap.text("rate_limits.bypass_key");
    if !key.is_empty()
        && headers
            .get("x-ratelimit-bypass")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == key)
    {
        return true;
    }
    ip.is_some_and(|ip| {
        snap.list("rate_limits.bypass_ips")
            .iter()
            .any(|c| cidr_contains(c, ip))
    })
}

async fn consume(s: &AppState, b: Bucket, key: &str, points: i64) -> Status {
    let now = locus_core::time::now_ms();
    let full = format!("{}:{key}", b.name);
    let use_db = match s.snapshot().text("rate_limits.backend") {
        "database" => true,
        "memory" => false,
        _ => s.store.is_postgres(),
    };
    let (consumed, window_end) = if use_db {
        match s.store.consume_rate_limit(&full, points, b.window_ms).await {
            Ok(v) => v,
            Err(e) => {
                // Fail open, as the reference.
                tracing::warn!("rate limit store unavailable: {e}");
                (0, now + b.window_ms)
            }
        }
    } else {
        s.limits.consume_memory(&full, points, b.window_ms, now)
    };
    Status {
        limit: b.points,
        window_ms: b.window_ms,
        remaining: (b.points - consumed).max(0),
        ms_before_next: (window_end - now).max(0),
        exceeded: consumed > b.points,
    }
}

// ---------------------------------------------------------------- per request

/// What a request's limits recorded: its client IP and the statuses of
/// every bucket it consumed.
#[derive(Debug, Default)]
struct Slot {
    ip: Option<IpAddr>,
    bypass: bool,
    statuses: Vec<Status>,
}

tokio::task_local! {
    static SLOT: Arc<Mutex<Slot>>;
}

/// The client IP of the request being handled.
#[must_use]
pub fn client_ip() -> Option<IpAddr> {
    SLOT.try_with(|s| s.lock().map(|s| s.ip).ok().flatten())
        .ok()
        .flatten()
}

/// Consumes `points` from each bucket for `key` (a handler's own limits).
/// Refused with 429 `RateLimitExceeded` when any is over.
pub async fn limit(s: &AppState, list: &[(Bucket, &str, i64)]) -> XrpcResult<()> {
    let Ok(slot) = SLOT.try_with(Arc::clone) else {
        return Ok(());
    };
    if slot.lock().map(|s| s.bypass).unwrap_or(true) {
        return Ok(());
    }
    let mut exceeded = false;
    for (b, key, points) in list {
        if *points < 1 {
            continue;
        }
        let st = consume(s, *b, key, *points).await;
        exceeded |= st.exceeded;
        if let Ok(mut sl) = slot.lock() {
            sl.statuses.push(st);
        }
    }
    if exceeded {
        return Err(too_many());
    }
    Ok(())
}

/// Consumes from one bucket keyed by the client IP.
pub async fn limit_ip(s: &AppState, list: &[Bucket]) -> XrpcResult<()> {
    let ip = client_ip().map_or_else(|| "unknown".to_owned(), |i| i.to_string());
    let with_keys: Vec<(Bucket, &str, i64)> = list.iter().map(|b| (*b, ip.as_str(), 1)).collect();
    limit(s, &with_keys).await
}

fn too_many() -> XrpcError {
    XrpcError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "RateLimitExceeded",
        "Rate Limit Exceeded",
    )
}

fn set_headers(res: &mut Response, statuses: &[Status]) {
    // The refusing bucket if any, else the one with the fewest points left.
    let chosen = statuses
        .iter()
        .find(|s| s.exceeded)
        .or_else(|| statuses.iter().min_by_key(|s| s.remaining));
    let Some(st) = chosen else {
        return;
    };
    let now = locus_core::time::now_ms();
    let h = res.headers_mut();
    let mut put = |name: &'static str, v: String| {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(name, v);
        }
    };
    put("ratelimit-limit", st.limit.to_string());
    put(
        "ratelimit-reset",
        ((now + st.ms_before_next) / 1000).to_string(),
    );
    put("ratelimit-remaining", st.remaining.to_string());
    put(
        "ratelimit-policy",
        format!("{};w={}", st.limit, st.window_ms / 1000),
    );
    let mut expose =
        String::from("RateLimit-Limit, RateLimit-Reset, RateLimit-Remaining, RateLimit-Policy");
    if st.exceeded {
        put(
            "retry-after",
            ((st.ms_before_next + 999) / 1000).to_string(),
        );
        expose.push_str(", Retry-After");
    }
    if let Ok(v) = HeaderValue::from_str(&expose) {
        h.append("access-control-expose-headers", v);
    }
}

/// The middleware: resolves the client IP, applies the global bucket to
/// XRPC requests (not `getRepo`, which has its own), and puts the
/// `RateLimit-*` headers of whatever the request consumed on the response.
pub async fn middleware(
    State(s): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip());
    let ip = real_ip(&s, peer, req.headers());
    let bypass = bypassed(&s, req.headers(), ip);
    let slot = Arc::new(Mutex::new(Slot {
        ip,
        bypass,
        statuses: Vec::new(),
    }));
    let path = req.uri().path().to_owned();
    let res = SLOT
        .scope(Arc::clone(&slot), async {
            if !bypass && path.starts_with("/xrpc/") && path != "/xrpc/_health" {
                let global = if path == "/xrpc/com.atproto.sync.getRepo" {
                    buckets::GET_REPO
                } else {
                    Bucket {
                        name: "global-ip",
                        window_ms: 5 * MINUTE,
                        points: s.snapshot().int("rate_limits.global_points"),
                    }
                };
                if let Err(e) = limit_ip(&s, &[global]).await {
                    return e.into_response();
                }
            }
            next.run(req).await
        })
        .await;
    let mut res = res;
    let statuses = slot.lock().map(|s| s.statuses.clone()).unwrap_or_default();
    set_headers(&mut res, &statuses);
    res
}

/// The repo-write buckets (creates 3 points, updates 2, deletes 1).
pub async fn limit_repo_write(s: &AppState, did: &str, points: i64) -> XrpcResult<()> {
    let snap = s.snapshot();
    let hour = Bucket {
        name: "repo-write-hour",
        window_ms: HOUR,
        points: snap.int("rate_limits.repo_write_hour_points"),
    };
    let day = Bucket {
        name: "repo-write-day",
        window_ms: DAY,
        points: snap.int("rate_limits.repo_write_day_points"),
    };
    limit(s, &[(hour, did, points), (day, did, points)]).await
}

// ---------------------------------------------------------------- client IP

/// Whether `ip` is in `cidr` (`a.b.c.d/n`, `x::/n`, or a bare address).
#[must_use]
pub fn cidr_contains(cidr: &str, ip: IpAddr) -> bool {
    let (addr, bits) = match cidr.trim().split_once('/') {
        Some((a, b)) => (a, b.parse::<u32>().ok()),
        None => (cidr.trim(), None),
    };
    let Ok(net) = addr.parse::<IpAddr>() else {
        return false;
    };
    let ip = match (net, ip) {
        (IpAddr::V4(_), IpAddr::V6(v6)) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        _ => ip,
    };
    match (net, ip) {
        (IpAddr::V4(n), IpAddr::V4(i)) => {
            let bits = bits.unwrap_or(32).min(32);
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(n) & mask == u32::from(i) & mask
        }
        (IpAddr::V6(n), IpAddr::V6(i)) => {
            let bits = bits.unwrap_or(128).min(128);
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(n) & mask == u128::from(i) & mask
        }
        _ => false,
    }
}

fn trusted(s: &AppState, ip: IpAddr) -> bool {
    s.snapshot()
        .list("http.trusted_proxies")
        .iter()
        .any(|c| cidr_contains(c, ip))
}

/// The client's IP: the peer, or (when the peer is a trusted proxy) the
/// right-most untrusted address in `X-Forwarded-For` (or `Forwarded`).
#[must_use]
pub fn real_ip(s: &AppState, peer: Option<IpAddr>, headers: &HeaderMap) -> Option<IpAddr> {
    let peer = peer?;
    if !trusted(s, peer) {
        return Some(peer);
    }
    let mut chain: Vec<IpAddr> = Vec::new();
    for v in headers.get_all("x-forwarded-for") {
        if let Ok(v) = v.to_str() {
            chain.extend(v.split(',').filter_map(|p| parse_addr(p.trim())));
        }
    }
    if chain.is_empty() {
        for v in headers.get_all("forwarded") {
            if let Ok(v) = v.to_str() {
                for part in v.split([',', ';']) {
                    let part = part.trim();
                    if let Some(f) = part
                        .strip_prefix("for=")
                        .or_else(|| part.strip_prefix("For="))
                        && let Some(ip) = parse_addr(f.trim_matches('"'))
                    {
                        chain.push(ip);
                    }
                }
            }
        }
    }
    let found = chain
        .iter()
        .rev()
        .find(|ip| !trusted(s, **ip))
        .copied()
        .or_else(|| chain.first().copied());
    // Behind Cloudflare the hop before our proxy is Cloudflare's; only then
    // does its CF-Connecting-IP name the client.
    if let Some(hop) = found
        && s.snapshot().text("deploy.proxy_mode") == "cloudflare"
        && crate::cloudflare::contains(hop)
        && let Some(client) = headers
            .get("cf-connecting-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
    {
        return Some(client);
    }
    found.or(Some(peer))
}

fn parse_addr(s: &str) -> Option<IpAddr> {
    if let Ok(ip) = s.parse() {
        return Some(ip);
    }
    // [v6]:port, v4:port
    if let Some(rest) = s.strip_prefix('[') {
        return rest.split(']').next()?.parse().ok();
    }
    s.rsplit_once(':').and_then(|(a, _)| a.parse().ok())
}

// ---------------------------------------------------------------- subscribers

impl Limiter {
    /// Firehose connections on this node: in all, and from how many
    /// addresses.
    pub fn subscriber_count(&self) -> (usize, usize) {
        let map = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (map.values().sum(), map.len())
    }
}

/// A firehose connection's place under the subscriber limits; released on
/// drop.
pub struct SubscriberGuard {
    s: Arc<AppState>,
    ip: Option<IpAddr>,
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        let mut map = self
            .s
            .limits
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = self.ip.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        if let Some(n) = map.get_mut(&key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&key);
            }
        }
    }
}

/// Takes a firehose place for the current client, or refuses with 429 when
/// this node is at `sync.max_subscribers` or the IP at
/// `sync.max_subscribers_per_ip` (0 = no limit).
pub fn subscribe(s: &Arc<AppState>) -> XrpcResult<SubscriberGuard> {
    let ip = client_ip();
    let snap = s.snapshot();
    let (total_max, per_ip_max) = (
        usize::try_from(snap.int("sync.max_subscribers")).unwrap_or(0),
        usize::try_from(snap.int("sync.max_subscribers_per_ip")).unwrap_or(0),
    );
    let bypass = SLOT
        .try_with(|sl| sl.lock().map(|s| s.bypass).unwrap_or(false))
        .unwrap_or(false);
    let key = ip.unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let mut map = s
        .limits
        .subscribers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let total: usize = map.values().sum();
    let mine = map.get(&key).copied().unwrap_or(0);
    if !bypass && ((total_max > 0 && total >= total_max) || (per_ip_max > 0 && mine >= per_ip_max))
    {
        return Err(XrpcError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RateLimitExceeded",
            "Too many firehose connections",
        ));
    }
    *map.entry(key).or_default() += 1;
    Ok(SubscriberGuard {
        s: Arc::clone(s),
        ip,
    })
}

#[cfg(test)]
mod tests {
    use super::cidr_contains;

    #[test]
    fn cidrs() {
        let ip = |s: &str| s.parse().unwrap();
        assert!(cidr_contains("127.0.0.0/8", ip("127.1.2.3")));
        assert!(!cidr_contains("127.0.0.0/8", ip("128.0.0.1")));
        assert!(cidr_contains("10.0.0.0/8", ip("::ffff:10.1.1.1")));
        assert!(cidr_contains("::1/128", ip("::1")));
        assert!(cidr_contains("2001:db8::/32", ip("2001:db8:1::5")));
        assert!(!cidr_contains("2001:db8::/32", ip("2001:db9::5")));
        assert!(cidr_contains("192.168.1.7", ip("192.168.1.7")));
        assert!(cidr_contains("0.0.0.0/0", ip("8.8.8.8")));
        assert!(!cidr_contains("nonsense", ip("8.8.8.8")));
    }
}
