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

use crate::error::{PdsError, PdsResult};
use crate::identity::clock::{Clock, SystemClock};
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};
use std::sync::Arc;
use uuid::Uuid;

/// Opaque keyset cursor for session listing (#273). The shared
/// `CursorPosition` keys on an `i64` id; `operator_session.id` is a UUID
/// string, so this is its string-id sibling — same base64-JSON envelope,
/// same `(created_at DESC, id DESC)` ordering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCursor {
    pub created_at: String,
    pub id: String,
}

impl SessionCursor {
    fn encode(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(self).expect("SessionCursor serialize"))
    }

    pub(crate) fn decode(s: &str) -> PdsResult<Self> {
        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(s.as_bytes())
            .map_err(|_| PdsError::Validation("invalid session cursor".to_string()))?;
        serde_json::from_slice(&raw)
            .map_err(|_| PdsError::Validation("invalid session cursor".to_string()))
    }
}

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
    /// When `current_refresh_id` last advanced — bounds the grace window in
    /// which `prev_refresh_id` is still honoured (#272). `None` until the
    /// session's first rotation.
    pub refresh_rotated_at: Option<DateTime<Utc>>,
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
    /// Window after a rotation during which the immediately-prior refresh
    /// token (`prev_refresh_id`) is still accepted, to survive a client that
    /// refreshed but lost the response and retried (#272). Short by design.
    refresh_grace: Duration,
}

impl OperatorSessionStore {
    /// Construct with the wall-clock `SystemClock` (the production path).
    pub fn new(db: AnyPool) -> Self {
        Self {
            db,
            clock: Arc::new(SystemClock),
            refresh_grace: Duration::seconds(60),
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
                    current_refresh_id, prev_refresh_id, refresh_rotated_at, \
                    revoked, revoked_at, revoked_by \
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

    /// Rotate the refresh token of a live session (#272, rotation-on-use).
    /// `presented_rid` is the `rid` claim from the refresh token the client
    /// presented. Returns the `rid` the caller should embed in the freshly
    /// issued access + refresh tokens, or `None` to reject the refresh (the
    /// client falls back to interactive re-login).
    ///
    /// Three outcomes:
    ///   * `presented_rid` is the current head → rotate (new rid; advance
    ///     current→prev, stamp `refresh_rotated_at`) and return the new rid.
    ///     The advance is a compare-and-swap `UPDATE ... WHERE
    ///     current_refresh_id = presented_rid`, so of two concurrent
    ///     refreshes only one wins the rotation — atomic without a tx.
    ///   * `presented_rid` is the immediately-prior head and the rotation is
    ///     within the grace window (a dropped-response retry, or the loser of
    ///     the CAS race) → return the *current* rid without re-rotating.
    ///   * anything else (stale token, past grace, revoked/expired/missing
    ///     session) → `None`.
    pub async fn rotate(&self, sid: &str, presented_rid: &str) -> PdsResult<Option<String>> {
        let now = self.clock.now();
        let Some(session) = self.get(sid).await? else {
            return Ok(None); // unknown session
        };
        if session.revoked || session.expires_at <= now {
            return Ok(None); // force-logged-out or lifetime elapsed
        }

        // Rotate: only when the presented token is the current head.
        if session.current_refresh_id.as_deref() == Some(presented_rid) {
            let new_rid = Uuid::new_v4().to_string();
            let affected = sqlx::query(
                "UPDATE operator_session \
                 SET prev_refresh_id = current_refresh_id, current_refresh_id = $1, \
                     refresh_rotated_at = $2, last_active_at = $2 \
                 WHERE id = $3 AND current_refresh_id = $4",
            )
            .bind(&new_rid)
            .bind(now.to_rfc3339())
            .bind(sid)
            .bind(presented_rid)
            .execute(&self.db)
            .await?
            .rows_affected();
            if affected == 1 {
                return Ok(Some(new_rid)); // won the rotation
            }
            // Lost the CAS to a concurrent refresh — re-read and fall to the
            // grace path (the winner set prev = presented_rid).
            let Some(session) = self.get(sid).await? else {
                return Ok(None);
            };
            if session.revoked || session.expires_at <= now {
                return Ok(None);
            }
            return Ok(self.grace_current(&session, presented_rid, now));
        }

        // Not the current head: grace, or reject.
        Ok(self.grace_current(&session, presented_rid, now))
    }

    /// Grace check: if `presented_rid` is the session's immediately-prior
    /// head and the last rotation is within the grace window, hand back the
    /// *current* rid (reissue without re-rotating). Otherwise `None`.
    fn grace_current(
        &self,
        session: &OperatorSession,
        presented_rid: &str,
        now: DateTime<Utc>,
    ) -> Option<String> {
        if session.prev_refresh_id.as_deref() == Some(presented_rid) {
            if let Some(rotated_at) = session.refresh_rotated_at {
                if now <= rotated_at + self.refresh_grace {
                    return session.current_refresh_id.clone();
                }
            }
        }
        None
    }

    /// Revoke (force-logout) a session inside an existing transaction so the
    /// flip composes atomically with the audit-chain append (#273, mirroring
    /// `AdminRoleManager::revoke_role_in_tx`). The per-request gate in
    /// `admin_auth_from_token` (#271) already rejects `revoked = true`, so
    /// the operator's next request reauthenticates. Returns the session's
    /// owner `did` (for the audit subject) on success; `NotFound` when no
    /// matching live session exists (unknown sid, or already revoked).
    pub async fn revoke_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        sid: &str,
        revoked_by: &str,
        reason: Option<&str>,
        now: DateTime<Utc>,
    ) -> PdsResult<String> {
        // Read the owner did within the tx so the caller can authorize +
        // audit against it; the UPDATE's `NOT revoked` guard makes the flip
        // idempotent-safe under a concurrent revoke.
        let did: Option<String> =
            sqlx::query_scalar("SELECT did FROM operator_session WHERE id = $1 AND NOT revoked")
                .bind(sid)
                .fetch_optional(&mut **tx)
                .await?;
        let Some(did) = did else {
            return Err(PdsError::NotFound(format!("no active session {}", sid)));
        };
        sqlx::query(
            "UPDATE operator_session \
             SET revoked = TRUE, revoked_at = $1, revoked_by = $2, revoke_reason = $3 \
             WHERE id = $4 AND NOT revoked",
        )
        .bind(now.to_rfc3339())
        .bind(revoked_by)
        .bind(reason)
        .bind(sid)
        .execute(&mut **tx)
        .await?;
        Ok(did)
    }

    /// Revoke EVERY active session for one operator (#338) — the SuperAdmin
    /// bulk force-logout for a compromised or departed operator. "Active" =
    /// not already revoked and not past expiry, so the returned count matches
    /// what `list_by_did` shows and an already-expired session isn't counted
    /// (the #271 per-request gate already rejects it). Idempotent: a second
    /// call revokes nothing and returns 0. The caller authorizes (SuperAdmin)
    /// and audits against `did` before invoking. Session revocation only forces
    /// re-authentication — it does not touch roles — so there is no
    /// last-SuperAdmin lockout to guard here (that guard is for role revocation).
    pub async fn revoke_all_for_did_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        did: &str,
        revoked_by: &str,
        reason: Option<&str>,
        now: DateTime<Utc>,
    ) -> PdsResult<u64> {
        let now_str = now.to_rfc3339();
        let res = sqlx::query(
            "UPDATE operator_session \
             SET revoked = TRUE, revoked_at = $1, revoked_by = $2, revoke_reason = $3 \
             WHERE did = $4 AND NOT revoked AND expires_at > $1",
        )
        .bind(&now_str)
        .bind(revoked_by)
        .bind(reason)
        .bind(did)
        .execute(&mut **tx)
        .await?;
        Ok(res.rows_affected())
    }

    /// List a single operator's active sessions (self-service), newest
    /// first, keyset-paginated. "Active" excludes revoked and expired.
    pub async fn list_by_did(
        &self,
        did: &str,
        limit: u32,
        cursor: Option<SessionCursor>,
    ) -> PdsResult<(Vec<OperatorSession>, Option<String>)> {
        self.list_sessions(Some(did), limit, cursor).await
    }

    /// List active sessions across all operators (SuperAdmin overview),
    /// newest first, keyset-paginated.
    pub async fn list_all(
        &self,
        limit: u32,
        cursor: Option<SessionCursor>,
    ) -> PdsResult<(Vec<OperatorSession>, Option<String>)> {
        self.list_sessions(None, limit, cursor).await
    }

    async fn list_sessions(
        &self,
        did: Option<&str>,
        limit: u32,
        cursor: Option<SessionCursor>,
    ) -> PdsResult<(Vec<OperatorSession>, Option<String>)> {
        let now = self.clock.now().to_rfc3339();
        let lim = limit.clamp(1, 100) as i64;

        // Active-only: not revoked, not past expiry. $1 = now.
        let mut binds: Vec<String> = vec![now];
        let mut where_parts: Vec<String> =
            vec!["revoked = FALSE".to_string(), "expires_at > $1".to_string()];
        if let Some(d) = did {
            binds.push(d.to_string());
            where_parts.push(format!("did = ${}", binds.len()));
        }
        if let Some(c) = &cursor {
            // Three placeholders (created_at bound twice, then id) — mirrors
            // get_audit_trail's keyset, which doesn't reuse a placeholder
            // across backends.
            binds.push(c.created_at.clone());
            let a = binds.len();
            binds.push(c.created_at.clone());
            let b = binds.len();
            binds.push(c.id.clone());
            let cc = binds.len();
            where_parts.push(format!(
                "(created_at < ${} OR (created_at = ${} AND id < ${}))",
                a, b, cc
            ));
        }
        let limit_ph = binds.len() + 1;
        let sql = format!(
            "SELECT id, did, created_at, last_active_at, expires_at, source_ip, user_agent, \
                    current_refresh_id, prev_refresh_id, refresh_rotated_at, \
                    revoked, revoked_at, revoked_by \
             FROM operator_session WHERE {} \
             ORDER BY created_at DESC, id DESC LIMIT ${}",
            where_parts.join(" AND "),
            limit_ph
        );

        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = q.bind(b);
        }
        q = q.bind(lim + 1); // fetch one extra to detect a further page
        let rows = q.fetch_all(&self.db).await?;

        let mut sessions: Vec<OperatorSession> = rows
            .iter()
            .map(Self::row_to_session)
            .collect::<PdsResult<Vec<_>>>()?;
        let next = if sessions.len() as i64 > lim {
            sessions.truncate(lim as usize);
            sessions.last().map(|s| {
                SessionCursor {
                    created_at: s.created_at.to_rfc3339(),
                    id: s.id.clone(),
                }
                .encode()
            })
        } else {
            None
        };
        Ok((sessions, next))
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
        let refresh_rotated_at: Option<String> = row.get("refresh_rotated_at");
        let revoked_at: Option<String> = row.get("revoked_at");
        let parse_opt = |o: Option<String>| -> PdsResult<Option<DateTime<Utc>>> {
            match o {
                Some(s) => Ok(Some(parse(&s)?)),
                None => Ok(None),
            }
        };
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
            refresh_rotated_at: parse_opt(refresh_rotated_at)?,
            revoked: crate::db::read_bool(row, "revoked")?,
            revoked_at: parse_opt(revoked_at)?,
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
                refresh_rotated_at TEXT, \
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

    // ---------- #338 bulk revoke-all-for-operator ----------

    #[tokio::test]
    async fn revoke_all_for_did_revokes_only_that_operators_active_sessions() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock.clone());
        store.create("did:plc:a", None, None, "ra1", Duration::days(30)).await.unwrap();
        store.create("did:plc:a", None, None, "ra2", Duration::days(30)).await.unwrap();
        store.create("did:plc:b", None, None, "rb1", Duration::days(30)).await.unwrap();

        let now = clock.now();
        let mut tx = store.db.begin().await.unwrap();
        let n = OperatorSessionStore::revoke_all_for_did_in_tx(
            &mut tx,
            "did:plc:a",
            "did:plc:super",
            Some("suspected compromise"),
            now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(n, 2, "both of operator A's active sessions revoked");

        let (a, _) = store.list_by_did("did:plc:a", 50, None).await.unwrap();
        assert!(a.is_empty(), "A has no active sessions left");
        let (b, _) = store.list_by_did("did:plc:b", 50, None).await.unwrap();
        assert_eq!(b.len(), 1, "operator B is untouched");

        // Idempotent: a second bulk revoke finds nothing active.
        let mut tx2 = store.db.begin().await.unwrap();
        let n2 = OperatorSessionStore::revoke_all_for_did_in_tx(
            &mut tx2,
            "did:plc:a",
            "did:plc:super",
            None,
            now,
        )
        .await
        .unwrap();
        tx2.commit().await.unwrap();
        assert_eq!(n2, 0, "nothing left to revoke on the second pass");
    }

    #[tokio::test]
    async fn revoke_all_for_did_skips_already_expired_sessions() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock.clone());
        store.create("did:plc:c", None, None, "rc1", Duration::seconds(10)).await.unwrap();
        // Past expiry: the session is already dead to the per-request gate, so a
        // bulk revoke targeting "active" sessions counts it as nothing.
        clock.advance(Duration::seconds(20));
        let now = clock.now();
        let mut tx = store.db.begin().await.unwrap();
        let n = OperatorSessionStore::revoke_all_for_did_in_tx(
            &mut tx, "did:plc:c", "did:plc:super", None, now,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(n, 0, "an already-expired session is not an active revoke target");
    }

    // ---------- #272 rotation-on-use ----------

    #[tokio::test]
    async fn rotate_advances_chain_and_grace_then_invalidates_old() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock.clone());
        let sid = store
            .create("did:plc:op", None, None, "r1", Duration::days(30))
            .await
            .unwrap();

        // Present the current head → rotate to a fresh rid.
        let r2 = store.rotate(&sid, "r1").await.unwrap().expect("rotated");
        assert_ne!(r2, "r1", "rotation issues a new rid");
        let s = store.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.current_refresh_id.as_deref(), Some(r2.as_str()));
        assert_eq!(s.prev_refresh_id.as_deref(), Some("r1"));
        assert_eq!(s.refresh_rotated_at, Some(anchor()));

        // The old token within grace (dropped-response retry / CAS loser):
        // hand back the current rid, do NOT re-rotate.
        clock.advance(Duration::seconds(30));
        let graced = store.rotate(&sid, "r1").await.unwrap().expect("grace");
        assert_eq!(graced, r2, "grace returns current head, no re-rotation");
        let s = store.get(&sid).await.unwrap().unwrap();
        assert_eq!(s.current_refresh_id.as_deref(), Some(r2.as_str()), "not re-rotated");

        // Past the grace window the old token is dead.
        clock.advance(Duration::seconds(31)); // 61s total since rotation
        assert!(store.rotate(&sid, "r1").await.unwrap().is_none(), "old token dead past grace");

        // The current head still rotates normally.
        let r3 = store.rotate(&sid, &r2).await.unwrap().expect("rotate current");
        assert_ne!(r3, r2);
    }

    #[tokio::test]
    async fn rotate_rejects_unknown_rid() {
        let store = OperatorSessionStore::new(open_test_pool().await);
        let sid = store
            .create("did:plc:op", None, None, "r1", Duration::days(30))
            .await
            .unwrap();
        assert!(store.rotate(&sid, "never-issued").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rotate_rejects_unknown_session() {
        let store = OperatorSessionStore::new(open_test_pool().await);
        assert!(store.rotate("no-such-sid", "r1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rotate_rejects_revoked_session() {
        let store = OperatorSessionStore::new(open_test_pool().await);
        let sid = store
            .create("did:plc:op", None, None, "r1", Duration::days(30))
            .await
            .unwrap();
        sqlx::query("UPDATE operator_session SET revoked = TRUE WHERE id = $1")
            .bind(&sid)
            .execute(&store.db)
            .await
            .unwrap();
        assert!(
            store.rotate(&sid, "r1").await.unwrap().is_none(),
            "a revoked session cannot rotate"
        );
    }

    #[tokio::test]
    async fn rotate_rejects_expired_session() {
        let clock = Arc::new(MockClock::new(anchor()));
        let store = OperatorSessionStore::new(open_test_pool().await).with_clock(clock.clone());
        let sid = store
            .create("did:plc:op", None, None, "r1", Duration::days(30))
            .await
            .unwrap();
        clock.advance(Duration::days(31));
        assert!(store.rotate(&sid, "r1").await.unwrap().is_none());
    }
}
