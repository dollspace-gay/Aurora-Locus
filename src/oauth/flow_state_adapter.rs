//! `DistributedStore` adapter over the existing
//! `authorization_request` table (Arc 7 Step 2, chainlink #53).
//!
//! Step 0 recon Q1 found Aurora-Locus's OAuth flow state already
//! lives in the `authorization_request` table within `account_db`
//! — DB-backed, shared across instances, with the
//! cross-instance-coherence story already working at the
//! storage layer. Arc 7's Step-2 work is to surface that table
//! through the `DistributedStore` trait so OAuth consumers
//! depend on the same trait surface as DPoP / rate-limit
//! consumers do.
//!
//! No schema migration. The adapter operates against the
//! pre-existing columns; the trait surface's opaque-bytes
//! `value` parameter is JSON-encoded
//! [`AuthorizationRequestData`] on insert and
//! [`AuthorizationRequest`] on read. That asymmetry mirrors the
//! existing direct-SQL API (`create_authorization_request`
//! takes the input data; `get_authorization_request` returns
//! the full row).
//!
//! Per-method semantics:
//! - `insert(table="oauth_flow_state", key=request_id, value,
//!   Some(lease))` — INSERT a new authorization_request row.
//!   `value` decodes to `AuthorizationRequestData`; `lease`
//!   becomes the row's `expires_at`. Returns `KeyExists` on
//!   primary-key collision.
//! - `get(table="oauth_flow_state", key=request_id)` —
//!   SELECT the row, filtering out consumed (`code_used =
//!   true`) and expired rows. Returns
//!   `AuthorizationRequest`-JSON bytes.
//! - `delete(table="oauth_flow_state", key=request_id)` — the
//!   **consume** semantic: UPDATE `code_used = true,
//!   code_used_at = now` WHERE `code_used = false`. Returns
//!   `true` if the row was flipped (cross-instance: exactly
//!   one instance's `delete` returns true). Returns `false` if
//!   no unconsumed row matched.
//! - `cas` — unsupported. OAuth state has no version column;
//!   per V04_DESIGN.md §6.3.2 unsupported-CAS is honest, not
//!   silent.
//! - `reap_expired(table="oauth_flow_state", now_epoch_ms)` —
//!   DELETE WHERE `expires_at < now`. Equivalent to the
//!   pre-existing `cleanup_expired_requests` sweep.
//!
//! Operations the adapter does NOT cover (deliberately):
//! - Adding an authorization_code to an existing row (phase-b
//!   of the OAuth flow at `/oauth/consent`). That's an
//!   OAuth-internal state transition; consent.rs handles it as
//!   direct SQL.
//! - Physical DELETE of a row (used by `/oauth/deny`). The
//!   trait's `delete` is the *consume* semantic; physical
//!   deletion stays direct SQL.
//! - Secondary-key lookup by `authorization_code` (used by the
//!   token endpoint). The trait keys uniformly on `request_id`
//!   (primary key); the secondary-key lookup
//!   `get_request_by_code` stays direct SQL. Cross-instance
//!   correctness is preserved at the consume step
//!   (`store.delete(request_id)`), which races atomically
//!   against the `code_used = false` predicate.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use sqlx::{AnyPool, Row};

use crate::distributed::{
    DistributedError, DistributedStore, Lease,
};
use crate::oauth::models::{AuthorizationRequest, AuthorizationRequestData};

const TABLE_NAME: &str = "oauth_flow_state";

/// `DistributedStore` impl over the existing `authorization_request`
/// table. Wraps the application's primary `account_db` pool — not
/// the substrate's maintenance pool — because the table is an
/// OAuth-domain artifact, not substrate state.
pub struct OAuthFlowStateAdapter {
    account_db: Arc<AnyPool>,
}

impl OAuthFlowStateAdapter {
    pub fn new(account_db: Arc<AnyPool>) -> Self {
        Self { account_db }
    }
}

/// Parse RFC3339 string into `DateTime<Utc>`. Local helper —
/// the OAuth handlers have an identical helper of their own;
/// duplicating here avoids cross-module coupling for a tiny
/// utility.
fn parse_ts(s: &str) -> Result<chrono::DateTime<Utc>, sqlx::Error> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

fn unique_violation(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        matches!(
            db_err.code().as_deref(),
            Some("23505") | Some("1555") | Some("2067")
        )
    } else {
        false
    }
}

#[async_trait]
impl DistributedStore for OAuthFlowStateAdapter {
    async fn insert(
        &self,
        table: &str,
        key: &str,
        value: &[u8],
        lease: Option<Lease>,
    ) -> Result<(), DistributedError> {
        if table != TABLE_NAME {
            return Err(DistributedError::UnsupportedTable(table.to_string()));
        }
        let data: AuthorizationRequestData = serde_json::from_slice(value)
            .map_err(|e| DistributedError::Database(sqlx::Error::Decode(Box::new(e))))?;

        // OAuth state needs a real expiry. The existing path
        // hard-coded 10 minutes (authorize.rs:238); if a lease
        // is provided, we use it; otherwise default to 10
        // minutes for backwards compatibility with callers that
        // pre-date the trait-routed path.
        let now = Utc::now();
        let expires_at = match lease {
            Some(l) => chrono::DateTime::from_timestamp_millis(l.expires_at_epoch_ms())
                .unwrap_or_else(|| now + chrono::Duration::minutes(10)),
            None => now + chrono::Duration::minutes(10),
        };

        let result = sqlx::query(
            r#"
            INSERT INTO authorization_request (
                request_id, did, client_id, code_challenge, code_challenge_method,
                scope, redirect_uri, state, created_at, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
        )
        .bind(key)
        .bind(&data.did)
        .bind(&data.client_id)
        .bind(&data.code_challenge)
        .bind(&data.code_challenge_method)
        .bind(&data.scope)
        .bind(&data.redirect_uri)
        .bind(&data.state)
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .execute(self.account_db.as_ref())
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) if unique_violation(&e) => Err(DistributedError::KeyExists {
                table: TABLE_NAME.to_string(),
                key: key.to_string(),
            }),
            Err(e) => Err(DistributedError::Database(e)),
        }
    }

    async fn get(
        &self,
        table: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DistributedError> {
        if table != TABLE_NAME {
            return Err(DistributedError::UnsupportedTable(table.to_string()));
        }
        let now_rfc3339 = Utc::now().to_rfc3339();
        // `id` and `code_used_at` are model fields whose columns
        // don't exist in the 0001_initial schema — Step 0 Q1
        // recon copied the model verbatim and missed the
        // pre-existing inconsistency. The adapter SELECTs only
        // the columns that actually exist; the model's `id`
        // and `code_used_at` are populated with synthetic
        // defaults (0 / None) in `row_to_authorization_request`.
        // Adding the columns would require a schema migration
        // which is out of scope for Step 2 (kickoff §scope).
        // Tracked for v0.6 cleanup as part of the
        // schema/model audit.
        let row = sqlx::query(
            r#"
            SELECT
                request_id, did, client_id, code_challenge, code_challenge_method,
                authorization_code, scope, redirect_uri, state, created_at, expires_at,
                code_used
            FROM authorization_request
            WHERE request_id = $1
              AND code_used = FALSE
              AND expires_at > $2
            "#,
        )
        .bind(key)
        .bind(&now_rfc3339)
        .fetch_optional(self.account_db.as_ref())
        .await
        .map_err(DistributedError::Database)?;

        let Some(row) = row else { return Ok(None) };

        let request = row_to_authorization_request(&row)
            .map_err(DistributedError::Database)?;
        let bytes = serde_json::to_vec(&request)
            .map_err(|e| DistributedError::Database(sqlx::Error::Encode(Box::new(e))))?;
        Ok(Some(bytes))
    }

    async fn delete(
        &self,
        table: &str,
        key: &str,
    ) -> Result<bool, DistributedError> {
        if table != TABLE_NAME {
            return Err(DistributedError::UnsupportedTable(table.to_string()));
        }
        // Consume semantic: flip code_used false → true
        // atomically. Returns rows_affected = 1 on first
        // consumer, 0 on every subsequent caller (including
        // replays from a sibling instance racing the same
        // request_id). The atomic transition through the
        // `code_used = FALSE` predicate IS the cross-instance
        // single-use guarantee.
        //
        // `code_used_at` would be the natural column to record
        // the consumption timestamp, but it doesn't exist in
        // the 0001_initial schema — see `row_to_authorization_request`
        // for the cross-reference. The UPDATE writes only
        // `code_used`; the model's `code_used_at` field stays
        // None.
        let result = sqlx::query(
            r#"
            UPDATE authorization_request
            SET code_used = TRUE
            WHERE request_id = $1 AND code_used = FALSE
            "#,
        )
        .bind(key)
        .execute(self.account_db.as_ref())
        .await
        .map_err(DistributedError::Database)?;

        Ok(result.rows_affected() > 0)
    }

    async fn reap_expired(
        &self,
        table: &str,
        _now_epoch_ms: i64,
    ) -> Result<usize, DistributedError> {
        if table != TABLE_NAME {
            return Err(DistributedError::UnsupportedTable(table.to_string()));
        }
        // Cross-backend portability: the schema's expires_at
        // column is TEXT RFC3339 (per migrations/0001_initial.sql),
        // not BIGINT epoch-ms like the substrate's other tables.
        // Lexicographic comparison on RFC3339 strings sorts
        // correctly within a single timezone, so we compare
        // against `now()` as an RFC3339 string. The
        // `_now_epoch_ms` parameter is accepted for trait
        // uniformity but not used here — the caller can pass
        // anything; the DELETE filters against wall-clock-now
        // regardless.
        let result = sqlx::query(
            r#"
            DELETE FROM authorization_request
            WHERE expires_at < $1
            "#,
        )
        .bind(Utc::now().to_rfc3339())
        .execute(self.account_db.as_ref())
        .await
        .map_err(DistributedError::Database)?;

        Ok(result.rows_affected() as usize)
    }
}

/// Decode one `authorization_request` row into the
/// `AuthorizationRequest` model. Mirrors the inline decoding
/// in `crate::oauth::authorize::get_authorization_request`;
/// duplicated here so the adapter is self-contained.
fn row_to_authorization_request(
    row: &sqlx::any::AnyRow,
) -> Result<AuthorizationRequest, sqlx::Error> {
    // `id` and `code_used_at` are model fields whose columns
    // don't exist in the 0001_initial schema. Synthetic
    // defaults (0 / None) — they're effectively dead fields
    // and would be removed in a model audit. See the SELECT
    // above for the cross-reference.
    Ok(AuthorizationRequest {
        id: 0,
        request_id: row.try_get("request_id")?,
        did: row.try_get("did")?,
        client_id: row.try_get("client_id")?,
        code_challenge: row.try_get("code_challenge")?,
        code_challenge_method: row.try_get("code_challenge_method")?,
        authorization_code: row.try_get("authorization_code")?,
        scope: row.try_get("scope")?,
        redirect_uri: row.try_get("redirect_uri")?,
        state: row.try_get("state")?,
        created_at: parse_ts(row.try_get::<String, _>("created_at")?.as_str())?,
        expires_at: parse_ts(row.try_get::<String, _>("expires_at")?.as_str())?,
        code_used: crate::db::read_bool(row, "code_used")?,
        code_used_at: None,
    })
}

/// JSON-encode an `AuthorizationRequestData` for the trait's
/// `value: &[u8]` parameter. Exposed for callers that need to
/// build the bytes before the trait call.
pub fn encode_request_data(data: &AuthorizationRequestData) -> Vec<u8> {
    // serialize_with cannot fail for this struct — all fields
    // are `String`/`Option<String>` (no Map<NonString, _>, no
    // f32/f64 that could be NaN). Unwrap is therefore safe.
    serde_json::to_vec(data).expect("AuthorizationRequestData JSON serialization cannot fail")
}

/// Decode the trait's `value: Vec<u8>` from a successful
/// `get("oauth_flow_state", _)` back into the typed
/// `AuthorizationRequest`. Symmetric with the adapter's
/// internal `serde_json::to_vec(&request)` on the read path.
pub fn decode_request(bytes: &[u8]) -> Result<AuthorizationRequest, serde_json::Error> {
    serde_json::from_slice(bytes)
}

#[cfg(test)]
mod tests {
    //! Unit tests against in-memory SQLite via `sqlx::Any`.
    //! Same pattern Step 1's `PostgresCasStore` tests use: the
    //! adapter's SQL is portable across both backends, so SQLite
    //! coverage exercises the same dispatch and translation
    //! logic. Cross-instance correctness against real Postgres
    //! is exercised by `tests/distributed_substrate_test.rs`.
    use super::*;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Once;

    async fn fresh_pool() -> Arc<AnyPool> {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        // Mirror migrations/0001_initial.sql verbatim for the
        // authorization_request table — TEXT PRIMARY KEY on
        // request_id, no `id` or `code_used_at` columns. The
        // model's `id` / `code_used_at` fields are pre-existing
        // dead state (see row_to_authorization_request).
        sqlx::query(
            "CREATE TABLE authorization_request (
                request_id               TEXT PRIMARY KEY,
                did                      TEXT NOT NULL,
                client_id                TEXT NOT NULL,
                code_challenge           TEXT NOT NULL,
                code_challenge_method    TEXT NOT NULL,
                scope                    TEXT NOT NULL,
                redirect_uri             TEXT NOT NULL,
                state                    TEXT,
                authorization_code       TEXT,
                code_used                INTEGER NOT NULL DEFAULT 0,
                created_at               TEXT NOT NULL,
                expires_at               TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create authorization_request");

        Arc::new(pool)
    }

    fn sample_data() -> AuthorizationRequestData {
        AuthorizationRequestData {
            did: "did:web:alice.example.com".to_string(),
            client_id: "https://client.example.com/metadata.json".to_string(),
            code_challenge: "abc123_challenge".to_string(),
            code_challenge_method: "S256".to_string(),
            scope: "atproto:read".to_string(),
            redirect_uri: "https://client.example.com/cb".to_string(),
            state: Some("client-csrf-state".to_string()),
        }
    }

    fn sample_value() -> Vec<u8> {
        encode_request_data(&sample_data())
    }

    fn lease_in_future() -> Lease {
        Lease::from_now(chrono::Duration::minutes(10))
    }

    // ---------- insert ----------

    #[tokio::test]
    async fn insert_first_sighting_succeeds() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        adapter
            .insert("oauth_flow_state", "req-1", &sample_value(), Some(lease_in_future()))
            .await
            .expect("first insert succeeds");
    }

    #[tokio::test]
    async fn insert_replay_returns_key_exists() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        adapter
            .insert("oauth_flow_state", "req-dup", &sample_value(), Some(lease_in_future()))
            .await
            .unwrap();
        let err = adapter
            .insert("oauth_flow_state", "req-dup", &sample_value(), Some(lease_in_future()))
            .await
            .expect_err("duplicate insert must fail");
        match err {
            DistributedError::KeyExists { table, key } => {
                assert_eq!(table, "oauth_flow_state");
                assert_eq!(key, "req-dup");
            }
            other => panic!("expected KeyExists, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn insert_without_lease_falls_back_to_ten_minutes() {
        // The trait surface allows lease=None for tables that
        // don't strictly require it. OAuth state needs a real
        // expiry, so the adapter falls back to the pre-Arc-7
        // default (10 minutes) when called without a lease.
        // Verifies the row is visible immediately (i.e., not
        // already-expired).
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(Arc::clone(&pool));
        adapter
            .insert("oauth_flow_state", "req-no-lease", &sample_value(), None)
            .await
            .expect("insert without lease succeeds");
        let got = adapter
            .get("oauth_flow_state", "req-no-lease")
            .await
            .unwrap();
        assert!(got.is_some(), "default-TTL row is visible immediately");
    }

    // ---------- get ----------

    #[tokio::test]
    async fn get_returns_full_authorization_request_after_insert() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        adapter
            .insert(
                "oauth_flow_state",
                "req-get",
                &sample_value(),
                Some(lease_in_future()),
            )
            .await
            .unwrap();
        let bytes = adapter
            .get("oauth_flow_state", "req-get")
            .await
            .unwrap()
            .expect("row present");
        let request = decode_request(&bytes).unwrap();
        assert_eq!(request.request_id, "req-get");
        assert_eq!(request.did, "did:web:alice.example.com");
        assert!(request.authorization_code.is_none());
        assert!(!request.code_used);
        assert!(request.code_used_at.is_none());
    }

    #[tokio::test]
    async fn get_returns_none_when_lease_expired() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        // Insert with a past lease — row exists but expired.
        let past_lease = Lease::until(chrono::Utc::now().timestamp_millis() - 60_000);
        adapter
            .insert("oauth_flow_state", "req-old", &sample_value(), Some(past_lease))
            .await
            .unwrap();
        assert!(
            adapter
                .get("oauth_flow_state", "req-old")
                .await
                .unwrap()
                .is_none(),
            "lease-expired row filtered from get"
        );
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_key() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        assert!(
            adapter
                .get("oauth_flow_state", "no-such-request")
                .await
                .unwrap()
                .is_none()
        );
    }

    // ---------- delete (consume) ----------

    #[tokio::test]
    async fn delete_first_call_returns_true_second_returns_false() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        adapter
            .insert("oauth_flow_state", "req-consume", &sample_value(), Some(lease_in_future()))
            .await
            .unwrap();
        assert!(
            adapter.delete("oauth_flow_state", "req-consume").await.unwrap(),
            "first consume returns true"
        );
        assert!(
            !adapter.delete("oauth_flow_state", "req-consume").await.unwrap(),
            "second consume returns false (already used)"
        );
    }

    #[tokio::test]
    async fn delete_after_consume_makes_get_return_none() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        adapter
            .insert("oauth_flow_state", "req-c2", &sample_value(), Some(lease_in_future()))
            .await
            .unwrap();
        adapter.delete("oauth_flow_state", "req-c2").await.unwrap();
        assert!(
            adapter
                .get("oauth_flow_state", "req-c2")
                .await
                .unwrap()
                .is_none(),
            "consumed row no longer visible to get"
        );
    }

    #[tokio::test]
    async fn delete_unknown_key_returns_false() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        assert!(
            !adapter
                .delete("oauth_flow_state", "ghost-req")
                .await
                .unwrap()
        );
    }

    // ---------- reap_expired ----------

    #[tokio::test]
    async fn reap_expired_deletes_only_past_rows() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(Arc::clone(&pool));

        let past = Lease::until(chrono::Utc::now().timestamp_millis() - 60_000);
        let future = Lease::from_now(chrono::Duration::minutes(10));
        adapter
            .insert("oauth_flow_state", "old-1", &sample_value(), Some(past))
            .await
            .unwrap();
        adapter
            .insert("oauth_flow_state", "old-2", &sample_value(), Some(past))
            .await
            .unwrap();
        adapter
            .insert("oauth_flow_state", "live", &sample_value(), Some(future))
            .await
            .unwrap();

        let swept = adapter
            .reap_expired("oauth_flow_state", chrono::Utc::now().timestamp_millis())
            .await
            .expect("reap ok");
        assert_eq!(swept, 2);

        // Live row survived.
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM authorization_request")
                .fetch_one(pool.as_ref())
                .await
                .unwrap();
        assert_eq!(remaining, 1);
    }

    // ---------- unknown table routing ----------

    #[tokio::test]
    async fn unknown_table_routes_to_unsupported_table() {
        let pool = fresh_pool().await;
        let adapter = OAuthFlowStateAdapter::new(pool);
        assert!(matches!(
            adapter
                .insert("zorblax", "k", b"{}", None)
                .await
                .expect_err("unknown table"),
            DistributedError::UnsupportedTable(_)
        ));
        assert!(matches!(
            adapter
                .get("zorblax", "k")
                .await
                .expect_err("unknown table"),
            DistributedError::UnsupportedTable(_)
        ));
        assert!(matches!(
            adapter
                .delete("zorblax", "k")
                .await
                .expect_err("unknown table"),
            DistributedError::UnsupportedTable(_)
        ));
        assert!(matches!(
            adapter
                .reap_expired("zorblax", 0)
                .await
                .expect_err("unknown table"),
            DistributedError::UnsupportedTable(_)
        ));
    }

    // ---------- round-trip encode/decode ----------

    #[test]
    fn encode_decode_request_data_round_trip() {
        let data = sample_data();
        let bytes = encode_request_data(&data);
        let decoded: AuthorizationRequestData = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.did, data.did);
        assert_eq!(decoded.state.as_deref(), Some("client-csrf-state"));
    }
}

