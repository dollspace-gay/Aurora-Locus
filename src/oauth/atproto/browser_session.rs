//! Browser-session substrate for the atproto-OAuth provider (Arc 2 Phase β.2,
//! chainlink #420 / LOCKED design §3.4 / R1 F-3.3).
//!
//! Aurora's two existing holder-auth extractors are both `Authorization:
//! Bearer`-shaped (XRPC). The atproto-OAuth provider's browser endpoints
//! (`/oauth/atproto/authorize`, `/oauth/atproto/consent/*`) arrive with
//! cookies, so they need a server-side session keyed by an opaque
//! `HttpOnly`/`Secure`/`SameSite=Lax` cookie. The session is minted by the
//! AS-login endpoint (login-α; see [`super::login`]) once the holder proves
//! control of their `#atproto` key, and it resolves the holder DID for the
//! consent flow.
//!
//! A browser session is **auth-only**: it authenticates the holder to the
//! consent flow and never authorizes the substrate to sign (pre-decision 3).
//! Operator revoke/expire of a session is censorship-class (forced re-auth),
//! never a forging window (§7).

use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, HeaderValue};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use sqlx::{AnyPool, FromRow};

use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

/// Cookie name carrying the opaque browser-session id.
pub const SESSION_COOKIE: &str = "aurora_oauth_session";

/// Cookie path — scopes the session to the OAuth routes only (blast-radius
/// limit per the §3.4 threat model). Covers both `/oauth/atproto/login` and
/// the `/oauth/atproto/{authorize,consent/*}` consumers.
pub const COOKIE_PATH: &str = "/oauth";

/// Absolute session lifetime: a session cannot outlive this regardless of
/// activity.
pub const SESSION_ABSOLUTE_TTL_SECS: i64 = 24 * 60 * 60;

/// Idle lifetime: a session expires this long after its last validated
/// request even if the absolute ceiling has not been reached.
pub const SESSION_IDLE_TTL_SECS: i64 = 60 * 60;

/// A server-side browser session row. Auth-only; carries no signing authority.
#[derive(Debug, Clone, FromRow)]
pub struct BrowserSession {
    /// Opaque CSPRNG session id (the cookie value).
    pub id: String,
    /// The authenticated holder DID.
    pub did: String,
    /// RFC3339 creation time.
    pub created_at: String,
    /// RFC3339; refreshed on each validated request (idle-expiry input).
    pub last_seen_at: String,
    /// RFC3339 absolute lifetime ceiling.
    pub expires_at: String,
    /// Per-session anti-CSRF token for the consent POSTs (the authorization
    /// `request_id` is NOT a trust token — F-3.2).
    pub csrf_token: String,
    /// Diagnostic only.
    pub user_agent: Option<String>,
    /// SHA-256 of the client IP (never the raw IP); optional diagnostic.
    pub ip_hash: Option<String>,
}

/// Generate a 256-bit opaque, URL-safe token (session id / CSRF token).
fn opaque_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Mint a fresh browser session for `did` and persist it. Returns the row
/// (the caller sets the cookie via [`set_session_cookie`]).
pub async fn create_session(
    db: &AnyPool,
    did: &str,
    user_agent: Option<String>,
    ip_hash: Option<String>,
) -> PdsResult<BrowserSession> {
    let now = Utc::now();
    let session = BrowserSession {
        id: opaque_token(),
        did: did.to_string(),
        created_at: now.to_rfc3339(),
        last_seen_at: now.to_rfc3339(),
        expires_at: (now + Duration::seconds(SESSION_ABSOLUTE_TTL_SECS)).to_rfc3339(),
        csrf_token: opaque_token(),
        user_agent,
        ip_hash,
    };
    sqlx::query(
        r#"
        INSERT INTO browser_session (
            id, did, created_at, last_seen_at, expires_at,
            csrf_token, user_agent, ip_hash
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(&session.id)
    .bind(&session.did)
    .bind(&session.created_at)
    .bind(&session.last_seen_at)
    .bind(&session.expires_at)
    .bind(&session.csrf_token)
    .bind(&session.user_agent)
    .bind(&session.ip_hash)
    .execute(db)
    .await
    .map_err(PdsError::Database)?;
    Ok(session)
}

/// Look up a session by id, enforcing both the absolute and idle lifetimes.
/// A valid session has its `last_seen_at` refreshed (sliding idle window).
/// An expired/idle session is deleted and `None` is returned.
pub async fn get_valid_session(
    db: &AnyPool,
    id: &str,
) -> PdsResult<Option<BrowserSession>> {
    let session = sqlx::query_as::<_, BrowserSession>(
        "SELECT id, did, created_at, last_seen_at, expires_at, csrf_token, \
         user_agent, ip_hash FROM browser_session WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(PdsError::Database)?;

    let Some(session) = session else {
        return Ok(None);
    };

    let now = Utc::now();
    let expires_at = parse_rfc3339(&session.expires_at)?;
    let last_seen = parse_rfc3339(&session.last_seen_at)?;
    let idle_deadline = last_seen + Duration::seconds(SESSION_IDLE_TTL_SECS);

    if now >= expires_at || now >= idle_deadline {
        // Past the absolute ceiling or idle-expired — evict and reject.
        delete_session(db, id).await?;
        return Ok(None);
    }

    // Slide the idle window forward.
    let touched = now.to_rfc3339();
    sqlx::query("UPDATE browser_session SET last_seen_at = $1 WHERE id = $2")
        .bind(&touched)
        .bind(id)
        .execute(db)
        .await
        .map_err(PdsError::Database)?;

    Ok(Some(BrowserSession {
        last_seen_at: touched,
        ..session
    }))
}

/// Delete a session row (logout / fixation-rotation / eviction).
pub async fn delete_session(db: &AnyPool, id: &str) -> PdsResult<()> {
    sqlx::query("DELETE FROM browser_session WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(PdsError::Database)?;
    Ok(())
}

fn parse_rfc3339(s: &str) -> PdsResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| PdsError::Internal(format!("malformed session timestamp '{}': {}", s, e)))
}

/// Build the `Set-Cookie` value that installs `session_id`.
///
/// `HttpOnly` (no JS access), `Secure` (HTTPS-only), `SameSite=Lax` (ships on
/// the top-level navigation a third-party client makes to `/oauth/atproto/
/// authorize`, but not on cross-site subrequests), path-scoped to the OAuth
/// routes, and `Max-Age` bounded to the absolute session lifetime.
pub fn set_session_cookie(session_id: &str) -> HeaderValue {
    let v = format!(
        "{SESSION_COOKIE}={session_id}; HttpOnly; Secure; SameSite=Lax; Path={COOKIE_PATH}; Max-Age={SESSION_ABSOLUTE_TTL_SECS}"
    );
    HeaderValue::from_str(&v).expect("cookie value is ascii by construction")
}

/// Build the `Set-Cookie` value that clears the session cookie (logout).
pub fn clear_session_cookie() -> HeaderValue {
    let v = format!(
        "{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Lax; Path={COOKIE_PATH}; Max-Age=0"
    );
    HeaderValue::from_str(&v).expect("cookie value is ascii by construction")
}

/// Extract the session id from the request `Cookie` header, if present.
pub fn read_session_cookie(headers: &HeaderMap) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    // `Cookie` is `name=value; name2=value2; ...` — find ours.
    cookie_header.split(';').find_map(|pair| {
        let pair = pair.trim();
        let (name, value) = pair.split_once('=')?;
        if name == SESSION_COOKIE {
            Some(value.to_string())
        } else {
            None
        }
    })
}

/// Axum extractor: resolves an authenticated browser session from the request
/// cookie. Rejects (401) when no cookie is present or the session is
/// missing/expired. Consumed by the AS `whoami` endpoint (β.2) and, in β.3,
/// by the authorize + consent handlers (which additionally match the session
/// DID to the stored `authorization_request.did` — FC-2 / FC-N+1).
pub struct BrowserSessionContext {
    pub session: BrowserSession,
}

#[async_trait]
impl FromRequestParts<AppContext> for BrowserSessionContext {
    type Rejection = PdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        ctx: &AppContext,
    ) -> Result<Self, Self::Rejection> {
        let id = read_session_cookie(&parts.headers).ok_or_else(|| {
            PdsError::Authentication("no browser session cookie".to_string())
        })?;
        let session = get_valid_session(&ctx.account_db, &id)
            .await?
            .ok_or_else(|| {
                PdsError::Authentication("browser session invalid or expired".to_string())
            })?;
        Ok(BrowserSessionContext { session })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::AppContext;

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    /// Insert a session row directly with explicit timestamps (for expiry
    /// tests), bypassing `create_session`'s now-anchored values.
    async fn seed_session(db: &AnyPool, id: &str, last_seen: &str, expires: &str) {
        sqlx::query(
            "INSERT INTO browser_session (id, did, created_at, last_seen_at, expires_at, \
             csrf_token, user_agent, ip_hash) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind("did:web:x.example.com")
        .bind("2026-01-01T00:00:00Z")
        .bind(last_seen)
        .bind(expires)
        .bind("csrf-tok")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .execute(db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_get_delete_round_trip() {
        let ctx = ctx().await;
        let s = create_session(&ctx.account_db, "did:web:a.example.com", None, None)
            .await
            .unwrap();
        assert!(!s.id.is_empty() && !s.csrf_token.is_empty());

        let got = get_valid_session(&ctx.account_db, &s.id)
            .await
            .unwrap()
            .expect("freshly created session is valid");
        assert_eq!(got.did, "did:web:a.example.com");

        delete_session(&ctx.account_db, &s.id).await.unwrap();
        assert!(get_valid_session(&ctx.account_db, &s.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn absolute_expired_session_is_evicted() {
        let ctx = ctx().await;
        let now = Utc::now();
        let past = (now - Duration::seconds(10)).to_rfc3339();
        seed_session(&ctx.account_db, "expired-abs", &now.to_rfc3339(), &past).await;

        assert!(get_valid_session(&ctx.account_db, "expired-abs")
            .await
            .unwrap()
            .is_none());
        // Eviction deleted the row.
        let still = sqlx::query("SELECT id FROM browser_session WHERE id = $1")
            .bind("expired-abs")
            .fetch_optional(&ctx.account_db)
            .await
            .unwrap();
        assert!(still.is_none());
    }

    #[tokio::test]
    async fn idle_expired_session_is_evicted() {
        let ctx = ctx().await;
        let now = Utc::now();
        // Absolute lifetime is still in the future, but last_seen is past the
        // idle window.
        let stale_last_seen = (now - Duration::seconds(SESSION_IDLE_TTL_SECS + 60)).to_rfc3339();
        let future = (now + Duration::seconds(SESSION_ABSOLUTE_TTL_SECS)).to_rfc3339();
        seed_session(&ctx.account_db, "idle", &stale_last_seen, &future).await;

        assert!(get_valid_session(&ctx.account_db, "idle")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn valid_session_slides_idle_window() {
        let ctx = ctx().await;
        let now = Utc::now();
        let old_last_seen = (now - Duration::seconds(120)).to_rfc3339();
        let future = (now + Duration::seconds(SESSION_ABSOLUTE_TTL_SECS)).to_rfc3339();
        seed_session(&ctx.account_db, "sliding", &old_last_seen, &future).await;

        let got = get_valid_session(&ctx.account_db, "sliding")
            .await
            .unwrap()
            .expect("within idle window");
        // last_seen_at was refreshed to ~now (newer than the seeded value).
        assert!(got.last_seen_at > old_last_seen);
    }

    #[test]
    fn cookie_build_and_parse_round_trip() {
        let set = set_session_cookie("sess-abc");
        let s = set.to_str().unwrap();
        assert!(s.starts_with("aurora_oauth_session=sess-abc"));
        assert!(s.contains("HttpOnly") && s.contains("Secure") && s.contains("SameSite=Lax"));
        assert!(s.contains("Path=/oauth"));

        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "other=1; aurora_oauth_session=sess-abc; more=2"
                .parse()
                .unwrap(),
        );
        assert_eq!(read_session_cookie(&headers).as_deref(), Some("sess-abc"));

        let empty = HeaderMap::new();
        assert!(read_session_cookie(&empty).is_none());

        let clear = clear_session_cookie();
        assert!(clear.to_str().unwrap().contains("Max-Age=0"));
    }
}
