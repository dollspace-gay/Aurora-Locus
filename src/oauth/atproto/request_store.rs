//! Persistence for the atproto-OAuth provider's authorization requests
//! (Arc 2 Phase β.3, chainlink #420 / LOCKED design §3.2).
//!
//! Backs the `atproto_authorization_request` table (sqlite migration `0030` /
//! pg `0031`) — a table dedicated to the `/oauth/atproto/*` provider, parallel
//! to the legacy `authorization_request` table (strangler-fig: SD-A2 = (c)).
//! All SQL touching that table lives here so the authorize / consent / token /
//! par endpoints share one row contract.
//!
//! Lifecycle of a row:
//!   1. `insert` — created by `authorize` (direct, `did` bound) or `par`
//!      (deferred, `did` NULL, `request_uri` set).
//!   2. `bind_did` — `authorize` binds the session DID onto a PAR row.
//!   3. `set_code_hash` — `consent/approve` records the minted code's hash.
//!   4. `claim_code` — `token` atomically marks the code used (single-use CAS).
//!      or `mark_denied` — `consent/deny` tombstones the row.

use chrono::{DateTime, Utc};
use sqlx::{AnyPool, FromRow};

use crate::error::{PdsError, PdsResult};

/// One `atproto_authorization_request` row. Mirrors the migration column-for-
/// column; nullable columns are `Option`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct AtprotoAuthorizationRequest {
    pub request_id: String,
    pub request_uri: Option<String>,
    pub client_id: String,
    pub redirect_uri: String,
    pub scope: String,
    pub state: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub did: Option<String>,
    pub code_hash: Option<String>,
    pub code_used_at: Option<String>,
    pub denied_at: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

const COLUMNS: &str = "request_id, request_uri, client_id, redirect_uri, scope, state, \
     code_challenge, code_challenge_method, did, code_hash, code_used_at, denied_at, \
     created_at, expires_at";

impl AtprotoAuthorizationRequest {
    /// True iff `now` is at or past `expires_at`. A malformed timestamp is
    /// treated as expired (fail-closed) rather than surfaced as an error — an
    /// undecodable expiry must never extend a request's life.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        match DateTime::parse_from_rfc3339(&self.expires_at) {
            Ok(exp) => now >= exp.with_timezone(&Utc),
            Err(_) => true,
        }
    }

    /// True iff the authorization code has already been redeemed.
    pub fn code_is_used(&self) -> bool {
        self.code_used_at.is_some()
    }

    /// True iff consent was denied.
    pub fn is_denied(&self) -> bool {
        self.denied_at.is_some()
    }
}

/// Insert a fresh authorization request.
pub async fn insert(db: &AnyPool, req: &AtprotoAuthorizationRequest) -> PdsResult<()> {
    sqlx::query(
        r#"
        INSERT INTO atproto_authorization_request (
            request_id, request_uri, client_id, redirect_uri, scope, state,
            code_challenge, code_challenge_method, did, code_hash, code_used_at,
            denied_at, created_at, expires_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(&req.request_id)
    .bind(&req.request_uri)
    .bind(&req.client_id)
    .bind(&req.redirect_uri)
    .bind(&req.scope)
    .bind(&req.state)
    .bind(&req.code_challenge)
    .bind(&req.code_challenge_method)
    .bind(&req.did)
    .bind(&req.code_hash)
    .bind(&req.code_used_at)
    .bind(&req.denied_at)
    .bind(&req.created_at)
    .bind(&req.expires_at)
    .execute(db)
    .await
    .map_err(PdsError::Database)?;
    Ok(())
}

/// Fetch by `request_id` (the consent-form correlation key).
pub async fn get_by_request_id(
    db: &AnyPool,
    request_id: &str,
) -> PdsResult<Option<AtprotoAuthorizationRequest>> {
    fetch_one_where(db, "request_id", request_id).await
}

/// Fetch by the PAR `request_uri`.
pub async fn get_by_request_uri(
    db: &AnyPool,
    request_uri: &str,
) -> PdsResult<Option<AtprotoAuthorizationRequest>> {
    fetch_one_where(db, "request_uri", request_uri).await
}

/// Fetch by the issued authorization code's hash (token redemption lookup).
pub async fn get_by_code_hash(
    db: &AnyPool,
    code_hash: &str,
) -> PdsResult<Option<AtprotoAuthorizationRequest>> {
    fetch_one_where(db, "code_hash", code_hash).await
}

async fn fetch_one_where(
    db: &AnyPool,
    column: &str,
    value: &str,
) -> PdsResult<Option<AtprotoAuthorizationRequest>> {
    // `column` is one of three internal string literals (never user input), so
    // interpolating it is safe; the value is always bound.
    let sql = format!(
        "SELECT {COLUMNS} FROM atproto_authorization_request WHERE {column} = $1"
    );
    sqlx::query_as::<_, AtprotoAuthorizationRequest>(&sql)
        .bind(value)
        .fetch_optional(db)
        .await
        .map_err(PdsError::Database)
}

/// Promote a (PAR-created) request to an active authorize request: bind the
/// holder DID that just authenticated AND reset the expiry to the consent
/// window. A PAR row is minted with a short push→authorize TTL (~60s); once
/// the holder reaches the authorize step the consent screen needs the longer
/// (~10min) window, so binding and re-expiry happen together.
pub async fn promote_par_request(
    db: &AnyPool,
    request_id: &str,
    did: &str,
    expires_at: &str,
) -> PdsResult<()> {
    sqlx::query(
        "UPDATE atproto_authorization_request SET did = $1, expires_at = $2 \
         WHERE request_id = $3",
    )
    .bind(did)
    .bind(expires_at)
    .bind(request_id)
    .execute(db)
    .await
    .map_err(PdsError::Database)?;
    Ok(())
}

/// Record the minted authorization code's hash (consent/approve).
pub async fn set_code_hash(db: &AnyPool, request_id: &str, code_hash: &str) -> PdsResult<()> {
    sqlx::query("UPDATE atproto_authorization_request SET code_hash = $1 WHERE request_id = $2")
        .bind(code_hash)
        .bind(request_id)
        .execute(db)
        .await
        .map_err(PdsError::Database)?;
    Ok(())
}

/// Tombstone the request as denied (consent/deny).
pub async fn mark_denied(db: &AnyPool, request_id: &str, now: &str) -> PdsResult<()> {
    sqlx::query("UPDATE atproto_authorization_request SET denied_at = $1 WHERE request_id = $2")
        .bind(now)
        .bind(request_id)
        .execute(db)
        .await
        .map_err(PdsError::Database)?;
    Ok(())
}

/// Atomically claim the authorization code for redemption (single-use).
///
/// This is the security-critical single-use gate (LOCKED §3.2 token step 5):
/// the `WHERE code_used_at IS NULL` predicate plus the affected-row count make
/// redemption a compare-and-set. The first caller to redeem a given code sees
/// `rows_affected == 1` and proceeds; any concurrent or replayed redemption
/// sees `0` and is rejected. Returns `true` iff this call claimed the code.
pub async fn claim_code(db: &AnyPool, request_id: &str, now: &str) -> PdsResult<bool> {
    let result = sqlx::query(
        "UPDATE atproto_authorization_request SET code_used_at = $1 \
         WHERE request_id = $2 AND code_used_at IS NULL",
    )
    .bind(now)
    .bind(request_id)
    .execute(db)
    .await
    .map_err(PdsError::Database)?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ctx() -> crate::context::AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    fn sample(request_id: &str) -> AtprotoAuthorizationRequest {
        let now = Utc::now();
        AtprotoAuthorizationRequest {
            request_id: request_id.to_string(),
            request_uri: None,
            client_id: "https://app.example.com/client-metadata.json".to_string(),
            redirect_uri: "https://app.example.com/cb".to_string(),
            scope: "atproto transition:generic".to_string(),
            state: Some("opaque-state".to_string()),
            code_challenge: "challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            did: Some("did:web:alice.example.com".to_string()),
            code_hash: None,
            code_used_at: None,
            denied_at: None,
            created_at: now.to_rfc3339(),
            expires_at: (now + chrono::Duration::minutes(10)).to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn insert_and_fetch_round_trip() {
        let ctx = ctx().await;
        let req = sample("req-1");
        insert(&ctx.account_db, &req).await.unwrap();
        let got = get_by_request_id(&ctx.account_db, "req-1")
            .await
            .unwrap()
            .expect("row present");
        assert_eq!(got, req);
        assert!(get_by_request_id(&ctx.account_db, "missing")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn par_row_binds_did_and_resolves_by_request_uri() {
        let ctx = ctx().await;
        let mut req = sample("req-par");
        req.did = None;
        req.request_uri = Some("urn:ietf:params:oauth:request_uri:abc".to_string());
        insert(&ctx.account_db, &req).await.unwrap();

        let by_uri =
            get_by_request_uri(&ctx.account_db, "urn:ietf:params:oauth:request_uri:abc")
                .await
                .unwrap()
                .expect("row present");
        assert_eq!(by_uri.request_id, "req-par");
        assert!(by_uri.did.is_none());

        let new_expiry = (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();
        promote_par_request(
            &ctx.account_db,
            "req-par",
            "did:web:bob.example.com",
            &new_expiry,
        )
        .await
        .unwrap();
        let bound = get_by_request_id(&ctx.account_db, "req-par")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(bound.did.as_deref(), Some("did:web:bob.example.com"));
        assert_eq!(bound.expires_at, new_expiry);
    }

    #[tokio::test]
    async fn set_code_hash_then_lookup_by_hash() {
        let ctx = ctx().await;
        insert(&ctx.account_db, &sample("req-code")).await.unwrap();
        set_code_hash(&ctx.account_db, "req-code", "deadbeef")
            .await
            .unwrap();
        let by_hash = get_by_code_hash(&ctx.account_db, "deadbeef")
            .await
            .unwrap()
            .expect("row present");
        assert_eq!(by_hash.request_id, "req-code");
    }

    #[tokio::test]
    async fn claim_code_is_single_use() {
        let ctx = ctx().await;
        insert(&ctx.account_db, &sample("req-cas")).await.unwrap();
        let now = Utc::now().to_rfc3339();
        // First claim wins.
        assert!(claim_code(&ctx.account_db, "req-cas", &now).await.unwrap());
        // Second claim of the same code loses (already used).
        assert!(!claim_code(&ctx.account_db, "req-cas", &now).await.unwrap());

        let row = get_by_request_id(&ctx.account_db, "req-cas")
            .await
            .unwrap()
            .unwrap();
        assert!(row.code_is_used());
    }

    #[tokio::test]
    async fn mark_denied_tombstones() {
        let ctx = ctx().await;
        insert(&ctx.account_db, &sample("req-deny")).await.unwrap();
        let now = Utc::now().to_rfc3339();
        mark_denied(&ctx.account_db, "req-deny", &now).await.unwrap();
        let row = get_by_request_id(&ctx.account_db, "req-deny")
            .await
            .unwrap()
            .unwrap();
        assert!(row.is_denied());
    }

    #[test]
    fn expiry_is_fail_closed() {
        let mut req = sample("x");
        let now = Utc::now();
        req.expires_at = (now - chrono::Duration::seconds(1)).to_rfc3339();
        assert!(req.is_expired(now));
        req.expires_at = (now + chrono::Duration::minutes(5)).to_rfc3339();
        assert!(!req.is_expired(now));
        // Garbage timestamp → treated as expired.
        req.expires_at = "not-a-timestamp".to_string();
        assert!(req.is_expired(now));
    }
}
