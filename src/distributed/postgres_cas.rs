//! Postgres-CAS (and SQLite-compatible) implementation of the
//! [`DistributedStore`](super::DistributedStore) trait.
//!
//! Built on `sqlx::Any` so the same implementation serves both
//! SQLite (single-instance development deployments) and
//! Postgres (multi-instance production deployments). The schema
//! it operates against was introduced by
//! `migrations/0007_distributed_state.sql` and stays within the
//! cross-backend portable subset (TEXT primary keys + BIGINT
//! columns; no `EXTRACT`, `TIMESTAMPTZ`, `BOOLEAN`, or
//! `BIGSERIAL`).
//!
//! Per-table dispatch:
//! - `dpop_jti_replay`: insert (with `KeyExists` on collision),
//!   get, delete, reap_expired. CAS is unsupported — the table
//!   has no `version` column.
//! - `rate_limit_buckets`: insert, get, delete, cas. `reap_expired`
//!   is unsupported by design (V04_DESIGN.md §6.3.7 — buckets
//!   are stateful across windows; inactivity-based GC is a
//!   Step-3 follow-up decision).
//! - Anything else: returns
//!   [`DistributedError::UnsupportedTable`](super::DistributedError::UnsupportedTable).
//!
//! Backend-specific unique-violation detection lives in
//! [`is_unique_violation`]; mirrors the `read_bool` helper
//! pattern in `src/db/mod.rs` (one place per backend-specific
//! quirk).
//!
//! The substrate parses the `value: &[u8]` parameter as JSON
//! per table. The trait surface stays opaque-bytes uniform so
//! a future Redis backend can serialize values into Redis blobs
//! without changing consumer code; the Postgres backend
//! decomposes the JSON into the fine-grained columns the
//! Step-0.6 schema chose. Per-table value schemas are documented
//! on [`DpopJtiReplayValue`] and [`RateLimitBucketValue`] below.
//!
//! Some impl methods on the `DistributedStore` trait surface have
//! no production consumer yet — file-level allow mirrors the trait-
//! side allow in `src/distributed/mod.rs`.
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};

use super::{DistributedError, DistributedStore, Lease};

/// Postgres-CAS backend for the [`DistributedStore`] trait.
///
/// Holds an `Arc<AnyPool>` rather than borrowing one because the
/// substrate is shared across the application: it's parked on
/// `AppContext` and cloned into background reaper tasks and
/// per-request consumers. Cloning an `AnyPool` is cheap (it
/// Arc-wraps internal state); cloning an `Arc<AnyPool>` is
/// cheaper still and avoids the double-Arc cost of constructing
/// a fresh `AnyPool` clone per use site.
#[derive(Clone)]
pub struct PostgresCasStore {
    pool: Arc<AnyPool>,
    /// Inactivity threshold for `rate_limit_buckets` sweeps, in
    /// milliseconds. Buckets whose `window_start_at_epoch_ms`
    /// hasn't been touched in this duration are presumed cold and
    /// deleted by the reaper. Default is 7 days
    /// ([`DEFAULT_RATE_LIMIT_RETENTION_MS`]); operator-tunable via
    /// `PDS_RATE_LIMIT_BUCKETS_RETENTION_DAYS` per V06 batch tail
    /// G7.2 (closes the v0.4-era "in-code constant" deferral
    /// flagged at the previous reap-arm site).
    rate_limit_retention_ms: i64,
}

/// 7-day default for the `rate_limit_buckets` inactivity sweep —
/// the v0.4 in-code constant, now the default for the operator-
/// tunable config. A bucket whose window_start hasn't been touched
/// in 7 days is presumed cold; deleting it costs the next
/// first-touch one extra INSERT (the bucket self-reconstructs at
/// full max_tokens) but bounds table growth for deployments
/// accumulating many unique bucket keys (one row per
/// (client_id, endpoint_class) ever seen).
pub const DEFAULT_RATE_LIMIT_RETENTION_MS: i64 = 7 * 24 * 3600 * 1000;

impl PostgresCasStore {
    pub fn new(pool: Arc<AnyPool>) -> Self {
        Self {
            pool,
            rate_limit_retention_ms: DEFAULT_RATE_LIMIT_RETENTION_MS,
        }
    }

    /// Override the `rate_limit_buckets` inactivity threshold (the
    /// G7.2 operator-tunable). `days` is whole days; multiplied by
    /// 86_400_000 ms internally. Production wiring at
    /// [`crate::context::AppContext`] reads the value from
    /// `config.rate_limit.buckets_retention_days`.
    pub fn with_rate_limit_retention_days(mut self, days: u32) -> Self {
        self.rate_limit_retention_ms = i64::from(days) * 24 * 3600 * 1000;
        self
    }
}

/// Wire shape for `dpop_jti_replay` row values.
///
/// The trait surface receives `value: &[u8]`; the substrate
/// parses it as JSON of this shape. `jti` and the epoch-ms
/// timestamps live elsewhere (key + lease + substrate-derived
/// `now`); only `jkt` is carried in the opaque value.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DpopJtiReplayValue {
    /// JWK thumbprint of the key that signed the DPoP proof.
    /// Observability only; not consulted on the hot path.
    jkt: String,
}

/// Wire shape for `rate_limit_buckets` row values.
///
/// The substrate manages `version` itself (CAS-incremented);
/// consumers don't set it explicitly.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RateLimitBucketValue {
    tokens_remaining: i64,
    max_tokens: i64,
    refill_rate: i64,
    window_start_at_epoch_ms: i64,
}

/// Detects whether a `sqlx::Error` represents a unique- or
/// primary-key-violation on the underlying backend. Centralised
/// here because the relevant error code differs per backend and
/// is awkward to compare against inline:
///
/// - Postgres: SQLSTATE `23505` (`unique_violation`).
/// - SQLite: extended code `1555` (`SQLITE_CONSTRAINT_PRIMARYKEY`)
///   or `2067` (`SQLITE_CONSTRAINT_UNIQUE`). Aurora-Locus's
///   `dpop_jti_replay` uses a TEXT PRIMARY KEY so the actual
///   code is usually 1555, but accepting both makes the helper
///   robust against schema changes that might introduce a
///   separate UNIQUE index.
///
/// Mirrors the [`crate::db::read_bool`] pattern: one place per
/// backend-specific quirk.
fn is_unique_violation(err: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_err) = err {
        matches!(
            db_err.code().as_deref(),
            Some("23505") | Some("1555") | Some("2067")
        )
    } else {
        false
    }
}

/// Current wall-clock in epoch milliseconds. The substrate uses
/// this for the `lease-expired -> None` filter in [`get`]; the
/// reaper job passes its own `now_epoch_ms` to
/// [`reap_expired`] so sweep semantics are testable with
/// controlled clocks.
fn now_epoch_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[async_trait]
impl DistributedStore for PostgresCasStore {
    async fn insert(
        &self,
        table: &str,
        key: &str,
        value: &[u8],
        lease: Option<Lease>,
    ) -> Result<(), DistributedError> {
        match table {
            "dpop_jti_replay" => self.insert_dpop_jti(key, value, lease).await,
            "rate_limit_buckets" => self.insert_rate_limit_bucket(key, value).await,
            other => Err(DistributedError::UnsupportedTable(other.to_string())),
        }
    }

    async fn get(
        &self,
        table: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DistributedError> {
        match table {
            "dpop_jti_replay" => self.get_dpop_jti(key).await,
            "rate_limit_buckets" => self.get_rate_limit_bucket(key).await,
            other => Err(DistributedError::UnsupportedTable(other.to_string())),
        }
    }

    async fn delete(
        &self,
        table: &str,
        key: &str,
    ) -> Result<bool, DistributedError> {
        let (sql, key_col) = match table {
            "dpop_jti_replay" => ("DELETE FROM dpop_jti_replay WHERE jti = $1", "jti"),
            "rate_limit_buckets" => (
                "DELETE FROM rate_limit_buckets WHERE bucket_key = $1",
                "bucket_key",
            ),
            other => return Err(DistributedError::UnsupportedTable(other.to_string())),
        };
        let _ = key_col; // documented in match arm for readability
        let result = sqlx::query(sql)
            .bind(key)
            .execute(self.pool.as_ref())
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn reap_expired(
        &self,
        table: &str,
        now_epoch_ms: i64,
    ) -> Result<usize, DistributedError> {
        match table {
            "dpop_jti_replay" => {
                let result = sqlx::query(
                    "DELETE FROM dpop_jti_replay WHERE exp_at_epoch_ms < $1",
                )
                .bind(now_epoch_ms)
                .execute(self.pool.as_ref())
                .await?;
                Ok(result.rows_affected() as usize)
            }
            // Inactivity-based GC for rate_limit_buckets. Step 1
            // returned UnsupportedTable here because the policy
            // wasn't decided; Step 3 (V04_DESIGN.md §6.4.3
            // post-recon resolution) committed to a 7-day default;
            // v0.6 batch tail G7.2 made the threshold operator-
            // tunable via `PDS_RATE_LIMIT_BUCKETS_RETENTION_DAYS`
            // (default 7) — see [`PostgresCasStore::with_rate_limit_retention_days`].
            "rate_limit_buckets" => {
                let cutoff = now_epoch_ms - self.rate_limit_retention_ms;
                let result = sqlx::query(
                    "DELETE FROM rate_limit_buckets WHERE window_start_at_epoch_ms < $1",
                )
                .bind(cutoff)
                .execute(self.pool.as_ref())
                .await?;
                Ok(result.rows_affected() as usize)
            }
            other => Err(DistributedError::UnsupportedTable(other.to_string())),
        }
    }
}

// ============================================================================
// Per-table implementations
// ============================================================================

impl PostgresCasStore {
    async fn insert_dpop_jti(
        &self,
        jti: &str,
        value: &[u8],
        lease: Option<Lease>,
    ) -> Result<(), DistributedError> {
        // The lease IS the row's exp_at_epoch_ms — it isn't
        // optional for this table. None is a consumer-side bug.
        let lease = lease.ok_or_else(|| {
            DistributedError::UnsupportedTable(
                "dpop_jti_replay requires a lease (no lease => no exp_at_epoch_ms)".to_string(),
            )
        })?;
        let parsed: DpopJtiReplayValue = serde_json::from_slice(value).map_err(|e| {
            // Treat consumer-side JSON malformation as
            // database-class error; the substrate isn't a
            // typed-DSL contract. A dedicated error variant
            // here would create surface-area churn for a
            // rare and operator-fixable failure mode.
            sqlx::Error::Decode(Box::new(e))
        })?;

        let result = sqlx::query(
            "INSERT INTO dpop_jti_replay (jti, jkt, exp_at_epoch_ms, created_at_epoch_ms) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(jti)
        .bind(&parsed.jkt)
        .bind(lease.expires_at_epoch_ms())
        .bind(now_epoch_ms())
        .execute(self.pool.as_ref())
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => Err(DistributedError::KeyExists {
                table: "dpop_jti_replay".to_string(),
                key: jti.to_string(),
            }),
            Err(e) => Err(DistributedError::Database(e)),
        }
    }

    async fn get_dpop_jti(&self, jti: &str) -> Result<Option<Vec<u8>>, DistributedError> {
        let now = now_epoch_ms();
        let row = sqlx::query(
            "SELECT jkt FROM dpop_jti_replay WHERE jti = $1 AND exp_at_epoch_ms >= $2",
        )
        .bind(jti)
        .bind(now)
        .fetch_optional(self.pool.as_ref())
        .await?;

        let Some(row) = row else { return Ok(None) };
        let jkt: String = row.try_get("jkt")?;
        let value = DpopJtiReplayValue { jkt };
        // serde_json::to_vec on a struct with a single String
        // field never fails in practice; surface any Err defensively.
        let bytes = serde_json::to_vec(&value)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        Ok(Some(bytes))
    }

    async fn insert_rate_limit_bucket(
        &self,
        bucket_key: &str,
        value: &[u8],
    ) -> Result<(), DistributedError> {
        let parsed: RateLimitBucketValue = serde_json::from_slice(value).map_err(|e| {
            sqlx::Error::Decode(Box::new(e))
        })?;

        let result = sqlx::query(
            "INSERT INTO rate_limit_buckets \
                (bucket_key, tokens_remaining, max_tokens, refill_rate, \
                 window_start_at_epoch_ms, version) \
             VALUES ($1, $2, $3, $4, $5, 0)",
        )
        .bind(bucket_key)
        .bind(parsed.tokens_remaining)
        .bind(parsed.max_tokens)
        .bind(parsed.refill_rate)
        .bind(parsed.window_start_at_epoch_ms)
        .execute(self.pool.as_ref())
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) if is_unique_violation(&e) => Err(DistributedError::KeyExists {
                table: "rate_limit_buckets".to_string(),
                key: bucket_key.to_string(),
            }),
            Err(e) => Err(DistributedError::Database(e)),
        }
    }

    async fn get_rate_limit_bucket(
        &self,
        bucket_key: &str,
    ) -> Result<Option<Vec<u8>>, DistributedError> {
        let row = sqlx::query(
            "SELECT tokens_remaining, max_tokens, refill_rate, window_start_at_epoch_ms \
             FROM rate_limit_buckets WHERE bucket_key = $1",
        )
        .bind(bucket_key)
        .fetch_optional(self.pool.as_ref())
        .await?;

        let Some(row) = row else { return Ok(None) };
        let value = RateLimitBucketValue {
            tokens_remaining: row.try_get("tokens_remaining")?,
            max_tokens: row.try_get("max_tokens")?,
            refill_rate: row.try_get("refill_rate")?,
            window_start_at_epoch_ms: row.try_get("window_start_at_epoch_ms")?,
        };
        let bytes = serde_json::to_vec(&value)
            .map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        Ok(Some(bytes))
    }

}

#[cfg(test)]
mod tests {
    //! Unit tests against in-memory SQLite via `sqlx::Any`. The
    //! substrate's SQL is intentionally backend-portable, so
    //! coverage at the SQLite layer exercises the same dispatch
    //! and translation logic. Postgres-side cross-instance
    //! behavior is exercised by `tests/multi_instance_test.rs`.
    use super::*;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Once;

    /// Spin up an in-memory SQLite `AnyPool` with the 0007
    /// migration's tables created. Each test gets its own
    /// pool (separate database) so concurrent tests don't
    /// trample each other.
    async fn fresh_pool() -> Arc<AnyPool> {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");

        // Schema mirrors migrations/0007_distributed_state.sql.
        sqlx::query(
            "CREATE TABLE dpop_jti_replay (
                jti                  TEXT PRIMARY KEY,
                jkt                  TEXT NOT NULL,
                exp_at_epoch_ms      BIGINT NOT NULL,
                created_at_epoch_ms  BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .expect("create dpop_jti_replay");

        sqlx::query(
            "CREATE TABLE rate_limit_buckets (
                bucket_key                 TEXT PRIMARY KEY,
                tokens_remaining           BIGINT NOT NULL,
                max_tokens                 BIGINT NOT NULL,
                refill_rate                BIGINT NOT NULL,
                window_start_at_epoch_ms   BIGINT NOT NULL,
                version                    BIGINT NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await
        .expect("create rate_limit_buckets");

        Arc::new(pool)
    }

    fn jti_value(jkt: &str) -> Vec<u8> {
        serde_json::to_vec(&DpopJtiReplayValue {
            jkt: jkt.to_string(),
        })
        .unwrap()
    }

    fn bucket_value(remaining: i64, max: i64, refill: i64, window_ms: i64) -> Vec<u8> {
        serde_json::to_vec(&RateLimitBucketValue {
            tokens_remaining: remaining,
            max_tokens: max,
            refill_rate: refill,
            window_start_at_epoch_ms: window_ms,
        })
        .unwrap()
    }

    // ---------- dpop_jti_replay path ----------

    #[tokio::test]
    async fn dpop_insert_first_sighting_succeeds() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        let lease = Lease::until(now_epoch_ms() + 60_000);
        store
            .insert("dpop_jti_replay", "jti-A", &jti_value("thumb1"), Some(lease))
            .await
            .expect("first sighting accepted");
    }

    #[tokio::test]
    async fn dpop_insert_replay_returns_key_exists() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        let lease = Lease::until(now_epoch_ms() + 60_000);
        store
            .insert("dpop_jti_replay", "jti-replay", &jti_value("thumb"), Some(lease))
            .await
            .unwrap();
        let err = store
            .insert("dpop_jti_replay", "jti-replay", &jti_value("thumb"), Some(lease))
            .await
            .expect_err("replay must be rejected");
        match err {
            DistributedError::KeyExists { table, key } => {
                assert_eq!(table, "dpop_jti_replay");
                assert_eq!(key, "jti-replay");
            }
            other => panic!("expected KeyExists, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn dpop_insert_requires_lease() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        let err = store
            .insert("dpop_jti_replay", "jti-no-lease", &jti_value("thumb"), None)
            .await
            .expect_err("missing lease must error");
        assert!(matches!(err, DistributedError::UnsupportedTable(_)));
    }

    #[tokio::test]
    async fn dpop_get_returns_some_after_insert_none_after_delete() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        let lease = Lease::until(now_epoch_ms() + 60_000);
        store
            .insert("dpop_jti_replay", "jti-X", &jti_value("thumbX"), Some(lease))
            .await
            .unwrap();
        let got = store.get("dpop_jti_replay", "jti-X").await.unwrap();
        assert!(got.is_some());
        let parsed: DpopJtiReplayValue = serde_json::from_slice(&got.unwrap()).unwrap();
        assert_eq!(parsed.jkt, "thumbX");

        let deleted = store.delete("dpop_jti_replay", "jti-X").await.unwrap();
        assert!(deleted, "first delete returns true");
        let got2 = store.get("dpop_jti_replay", "jti-X").await.unwrap();
        assert!(got2.is_none(), "post-delete get is None");

        let deleted_again = store.delete("dpop_jti_replay", "jti-X").await.unwrap();
        assert!(!deleted_again, "idempotent retry returns false");
    }

    #[tokio::test]
    async fn dpop_get_filters_lease_expired_rows() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        // Lease in the past — already expired.
        let lease = Lease::until(now_epoch_ms() - 1_000);
        store
            .insert("dpop_jti_replay", "jti-old", &jti_value("thumb"), Some(lease))
            .await
            .unwrap();
        let got = store.get("dpop_jti_replay", "jti-old").await.unwrap();
        assert!(
            got.is_none(),
            "lease-expired row must be None even before reaper sweeps"
        );
    }

    #[tokio::test]
    async fn dpop_reap_expired_deletes_only_expired() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(Arc::clone(&pool));
        let now = now_epoch_ms();
        store
            .insert("dpop_jti_replay", "j-old1", &jti_value("k1"), Some(Lease::until(now - 1_000)))
            .await
            .unwrap();
        store
            .insert("dpop_jti_replay", "j-old2", &jti_value("k2"), Some(Lease::until(now - 500)))
            .await
            .unwrap();
        store
            .insert(
                "dpop_jti_replay",
                "j-future",
                &jti_value("k3"),
                Some(Lease::until(now + 60_000)),
            )
            .await
            .unwrap();

        let swept = store
            .reap_expired("dpop_jti_replay", now)
            .await
            .expect("reaper sweep ok");
        assert_eq!(swept, 2, "expected to sweep both expired rows");

        // The future-lease row survives.
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM dpop_jti_replay")
                .fetch_one(pool.as_ref())
                .await
                .unwrap();
        assert_eq!(remaining, 1);
    }

    // ---------- rate_limit_buckets path ----------

    #[tokio::test]
    async fn rate_limit_insert_and_get_round_trip() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        store
            .insert(
                "rate_limit_buckets",
                "bucket-A",
                &bucket_value(50, 100, 10, now_epoch_ms()),
                None,
            )
            .await
            .expect("insert");
        let got = store.get("rate_limit_buckets", "bucket-A").await.unwrap();
        let parsed: RateLimitBucketValue =
            serde_json::from_slice(&got.expect("row present")).unwrap();
        assert_eq!(parsed.tokens_remaining, 50);
        assert_eq!(parsed.max_tokens, 100);
        assert_eq!(parsed.refill_rate, 10);
    }

    #[tokio::test]
    async fn rate_limit_reap_expired_sweeps_only_stale_buckets() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(Arc::clone(&pool));
        let now = now_epoch_ms();
        let eight_days_ago = now - 8 * 24 * 3600 * 1000;
        let one_day_ago = now - 24 * 3600 * 1000;

        // Stale bucket: should be swept.
        store
            .insert(
                "rate_limit_buckets",
                "stale-bucket",
                &bucket_value(50, 100, 10, eight_days_ago),
                None,
            )
            .await
            .unwrap();
        // Active bucket: should survive.
        store
            .insert(
                "rate_limit_buckets",
                "active-bucket",
                &bucket_value(50, 100, 10, one_day_ago),
                None,
            )
            .await
            .unwrap();

        let swept = store
            .reap_expired("rate_limit_buckets", now)
            .await
            .expect("reaper sweep ok");
        assert_eq!(swept, 1, "exactly the stale bucket is swept");

        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM rate_limit_buckets")
                .fetch_one(pool.as_ref())
                .await
                .unwrap();
        assert_eq!(remaining, 1, "active bucket survives");
    }

    // ---------- unknown-table dispatch ----------

    #[tokio::test]
    async fn unknown_table_routes_to_unsupported_table() {
        let pool = fresh_pool().await;
        let store = PostgresCasStore::new(pool);
        let err = store
            .insert("zorblax", "k", b"{}", None)
            .await
            .expect_err("unknown table");
        assert!(matches!(err, DistributedError::UnsupportedTable(_)));
        let err = store
            .get("zorblax", "k")
            .await
            .expect_err("unknown table");
        assert!(matches!(err, DistributedError::UnsupportedTable(_)));
        let err = store
            .delete("zorblax", "k")
            .await
            .expect_err("unknown table");
        assert!(matches!(err, DistributedError::UnsupportedTable(_)));
        let err = store
            .reap_expired("zorblax", 0)
            .await
            .expect_err("unknown table");
        assert!(matches!(err, DistributedError::UnsupportedTable(_)));
    }
}
