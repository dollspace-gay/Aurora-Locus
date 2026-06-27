//! v0.9 Federation runtime-mutability arc §3.6 (#394) — home of the
//! `bulk_diddoc_update_result` table.
//!
//! The table (migration `0025` sqlite / `0026` postgres) tracks the post-restart
//! bulk did:plc DID-doc update per run, per account. D2's save handler writes the
//! initial `pending` rows in the same outer tx as the `bulk-diddoc-update`
//! marker; Phase E2's background task updates each to a terminal status; E4
//! surfaces the most-recent run (`ORDER BY started_at DESC LIMIT 1` — the recency
//! key is `started_at`, NOT the UUID `run_id`, per R3 H-2).
//!
//! C5 ships the schema only. The row read/write helpers land with E2/E4; this
//! module documents the table and pins its constraints under test.

#[cfg(test)]
mod tests {
    /// Mirror of the shipped migration so the constraints can be exercised on an
    /// in-memory pool without the full migration runner. Kept in lockstep with
    /// `migrations/0025_phase_c_bulk_diddoc_update_result.sql`.
    const SCHEMA: &str = "CREATE TABLE bulk_diddoc_update_result (\
        did TEXT NOT NULL, run_id TEXT NOT NULL, started_at TEXT NOT NULL, \
        status TEXT NOT NULL CHECK(status IN \
        ('pending','aligned','failed','unresolvable','skipped_did_web')), \
        reason TEXT, updated_at TEXT NOT NULL, PRIMARY KEY (did, run_id))";

    async fn pool() -> sqlx::AnyPool {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(SCHEMA).execute(&pool).await.unwrap();
        pool
    }

    async fn insert(pool: &sqlx::AnyPool, did: &str, run_id: &str, status: &str) -> bool {
        sqlx::query(
            "INSERT INTO bulk_diddoc_update_result \
             (did, run_id, started_at, status, reason, updated_at) \
             VALUES ($1, $2, $3, $4, NULL, $5)",
        )
        .bind(did)
        .bind(run_id)
        .bind("2026-06-27T00:00:00Z")
        .bind(status)
        .bind("2026-06-27T00:00:01Z")
        .execute(pool)
        .await
        .is_ok()
    }

    #[tokio::test]
    async fn valid_statuses_insert_ok() {
        let p = pool().await;
        for (i, s) in ["pending", "aligned", "failed", "unresolvable", "skipped_did_web"]
            .iter()
            .enumerate()
        {
            assert!(
                insert(&p, &format!("did:plc:a{i}"), "run-1", s).await,
                "status {s} should be accepted"
            );
        }
    }

    #[tokio::test]
    async fn invalid_status_rejected_by_check() {
        let p = pool().await;
        assert!(
            !insert(&p, "did:plc:x", "run-1", "bogus").await,
            "CHECK constraint must reject an unlisted status"
        );
    }

    #[tokio::test]
    async fn primary_key_did_run_id_unique() {
        let p = pool().await;
        assert!(insert(&p, "did:plc:dup", "run-1", "pending").await);
        // Same (did, run_id) again → PK violation.
        assert!(
            !insert(&p, "did:plc:dup", "run-1", "aligned").await,
            "duplicate (did, run_id) must violate the primary key"
        );
        // Same did, different run_id → allowed (a later run).
        assert!(insert(&p, "did:plc:dup", "run-2", "pending").await);
    }
}
