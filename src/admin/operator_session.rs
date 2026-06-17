//! Per-operator session store (Arc E 0.9.3, §8.1.7 / chainlink #271).
//!
//! Backs per-operator session management: listing active operator sessions,
//! SuperAdmin force-logout (revoke), and refresh-token rotation-on-use. The
//! unit is an *operator session* keyed by an opaque session id (`sid`, a
//! UUID) that the admin access + refresh tokens carry as a claim.
//!
//! This exists because AS-only admin operators — those authenticating via
//! their atproto identity with no local `actor` row — had no server-side
//! session state at all (their tokens were stateless HS256 JWTs). The
//! existing `session`/`refresh_token` tables FK to `actor(did)` and so
//! cannot hold them; `operator_session` is the FK-free, dedicated store.
//!
//! Time comes from an injected [`Clock`] so expiry/last-active logic is
//! deterministic under test — the same precedent `DidCache` and `NonceStore`
//! follow (chainlink #269). Production wires [`SystemClock`].
//!
//! Scope (chainlink #271): this ticket lands the store + creation at login +
//! the per-request validate/touch hook in the admin auth path. Rotation
//! advances `current_refresh_id`/`prev_refresh_id` (#272); `revoke` + the
//! listing/force-logout XRPC surface land in #273.

use crate::error::PdsResult;
use crate::identity::clock::{Clock, SystemClock};
use chrono::{DateTime, Duration, Utc};
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use uuid::Uuid;

/// A persisted operator session row.
#[derive(Debug, Clone)]
pub struct OperatorSession {
    pub id: String,
    pub did: String,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub source_ip: Option<String>,
    pub user_agent: Option<String>,
    pub current_refresh_id: Option<String>,
    pub prev_refresh_id: Option<String>,
    pub revoked: bool,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<String>,
}

/// Manager over the `operator_session` table. Mirrors `AdminRoleManager`'s
/// shape (a thin handle over the shared `AnyPool`) plus a `Clock` for
/// deterministic expiry under test.
pub struct OperatorSessionStore {
    db: AnyPool,
    clock: Arc<dyn Clock>,
}

impl OperatorSessionStore {
    /// Construct with the wall-clock `SystemClock` (the production path).
    pub fn new(db: AnyPool) -> Self {
        Self {
            db,
            clock: Arc::new(SystemClock),
        }
    }

    /// Override the time source. Production keeps the `SystemClock` default;
    /// tests inject a `MockClock`. Mirrors `DidCache`/`NonceStore`.
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Create a new operator session, returning its `sid`. `refresh_id` is
    /// the id embedded in the session's first refresh token (the rotation
    /// chain head, consumed by #272). `lifetime` sets `expires_at` from the
    /// injected clock — tie it to the refresh-token TTL so the session row
    /// outlives the short access token.
    pub async fn create(
        &self,
        did: &str,
        source_ip: Option<&str>,
        user_agent: Option<&str>,
        refresh_id: &str,
        lifetime: Duration,
    ) -> PdsResult<String> {
        let sid = Uuid::new_v4().to_string();
        let now = self.clock.now();
        let expires_at = now + lifetime;
        sqlx::query(
            "INSERT INTO operator_session \
             (id, did, created_at, last_active_at, expires_at, source_ip, user_agent, \
              current_refresh_id, prev_refresh_id, revoked) \
             VALUES ($1, $2, $3, $3, $4, $5, $6, $7, NULL, FALSE)",
        )
        .bind(&sid)
        .bind(did)
        .bind(now.to_rfc3339())
        .bind(expires_at.to_rfc3339())
        .bind(source_ip)
        .bind(user_agent)
        .bind(refresh_id)
        .execute(&self.db)
        .await?;
        Ok(sid)
    }

    /// Fetch a session by `sid`. Returns `None` for an unknown id.
    pub async fn get(&self, sid: &str) -> PdsResult<Option<OperatorSession>> {
        let row = sqlx::query(
            "SELECT id, did, created_at, last_active_at, expires_at, source_ip, user_agent, \
                    current_refresh_id, prev_refresh_id, revoked, revoked_at, revoked_by \
             FROM operator_session WHERE id = $1",
        )
        .bind(sid)
        .fetch_optional(&self.db)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(Self::row_to_session(&row)?))
    }

    /// The per-request hot path used by the admin auth layer: returns `true`
    /// (and bumps `last_active_at`) when `sid` names a live session — present,
    /// not revoked, not expired — and `false` otherwise. A `false` result
    /// means the caller must reject the request (the operator reauthenticates).
    pub async fn validate_and_touch(&self, sid: &str) -> PdsResult<bool> {
        let now = self.clock.now();
        let Some(session) = self.get(sid).await? else {
            return Ok(false); // unknown sid
        };
        if session.revoked {
            return Ok(false); // force-logged-out (#273 writer)
        }
        if session.expires_at <= now {
            return Ok(false); // session lifetime elapsed
        }
        // Best-effort activity bump; a lost race here only costs precision in
        // the "last active" column, never correctness of the validity gate.
        sqlx::query("UPDATE operator_session SET last_active_at = $1 WHERE id = $2")
            .bind(now.to_rfc3339())
            .bind(sid)
            .execute(&self.db)
            .await?;
        Ok(true)
    }

    fn row_to_session(row: &sqlx::any::AnyRow) -> PdsResult<OperatorSession> {
        let parse = |s: &str| -> PdsResult<DateTime<Utc>> {
            DateTime::parse_from_rfc3339(s)
                .map(|d| d.with_timezone(&Utc))
                .map_err(|e| {
                    crate::error::PdsError::Internal(format!(
                        "operator_session: bad timestamp '{}': {}",
                        s, e
                    ))
                })
        };
        let created_at: String = row.get("created_at");
        let last_active_at: String = row.get("last_active_at");
        let expires_at: String = row.get("expires_at");
        let revoked_at: Option<String> = row.get("revoked_at");
        Ok(OperatorSession {
            id: row.get("id"),
            did: row.get("did"),
            created_at: parse(&created_at)?,
            last_active_at: parse(&last_active_at)?,
            expires_at: parse(&expires_at)?,
            source_ip: row.get("source_ip"),
            user_agent: row.get("user_agent"),
            current_refresh_id: row.get("current_refresh_id"),
            prev_refresh_id: row.get("prev_refresh_id"),
            revoked: crate::db::read_bool(row, "revoked")?,
            revoked_at: match revoked_at {
                Some(s) => Some(parse(&s)?),
                None => None,
            },
            revoked_by: row.get("revoked_by"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::clock::MockClock;

    async fn open_test_pool() -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE operator_session (\
                id TEXT PRIMARY KEY, did TEXT NOT NULL, created_at TEXT NOT NULL, \
                last_active_at TEXT NOT NULL, expires_at TEXT NOT NULL, source_ip TEXT, \
                user_agent TEXT, current_refresh_id TEXT, prev_refresh_id TEXT, \
                revoked INTEGER NOT NULL DEFAULT 0, revoked_at TEXT, revoked_by TEXT, \
                revoke_reason TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn anchor() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2020-06-15T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[tokio::test]
    async fn create_then_get_roundtrips() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock);
        let sid = store
            .create("did:plc:op", Some("203.0.113.7"), Some("curl/8"), "rid-1", Duration::days(30))
            .await
            .unwrap();
        let s = store.get(&sid).await.unwrap().expect("present");
        assert_eq!(s.did, "did:plc:op");
        assert_eq!(s.source_ip.as_deref(), Some("203.0.113.7"));
        assert_eq!(s.current_refresh_id.as_deref(), Some("rid-1"));
        assert!(!s.revoked);
        assert_eq!(s.created_at, anchor());
        assert_eq!(s.expires_at, anchor() + Duration::days(30));
    }

    #[tokio::test]
    async fn validate_touches_last_active_and_advances() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock.clone());
        let sid = store
            .create("did:plc:op", None, None, "rid-1", Duration::days(30))
            .await
            .unwrap();

        clock.advance(Duration::hours(3));
        assert!(store.validate_and_touch(&sid).await.unwrap());
        let s = store.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.last_active_at, anchor() + Duration::hours(3));
    }

    #[tokio::test]
    async fn validate_rejects_unknown_sid() {
        let store = OperatorSessionStore::new(open_test_pool().await);
        assert!(!store.validate_and_touch("nope").await.unwrap());
    }

    #[tokio::test]
    async fn validate_rejects_expired_session() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock.clone());
        let sid = store
            .create("did:plc:op", None, None, "rid-1", Duration::days(30))
            .await
            .unwrap();
        clock.advance(Duration::days(31)); // past the 30d lifetime
        assert!(!store.validate_and_touch(&sid).await.unwrap());
    }

    /// Wiring-assertion test (per the wiring-tripwire discipline): the
    /// per-request gate must reject a revoked session even though the public
    /// `revoke()` writer doesn't exist until #273. Flip the flag directly to
    /// prove the *check* is live now, not inert.
    #[tokio::test]
    async fn validate_rejects_revoked_session() {
        let store = OperatorSessionStore::new(open_test_pool().await);
        let sid = store
            .create("did:plc:op", None, None, "rid-1", Duration::days(30))
            .await
            .unwrap();
        assert!(store.validate_and_touch(&sid).await.unwrap());
        // Simulate the #273 revoke writer.
        sqlx::query("UPDATE operator_session SET revoked = TRUE WHERE id = $1")
            .bind(&sid)
            .execute(&store.db)
            .await
            .unwrap();
        assert!(
            !store.validate_and_touch(&sid).await.unwrap(),
            "revoked session must fail the per-request gate immediately"
        );
    }
}
