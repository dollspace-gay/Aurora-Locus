//! Arc 16f §9.6.3.4 — STRICT/TOLERANT swap mechanism for Phase B blob
//! promotion. The [`BlobPromoter`] trait abstracts over Arc 16b's
//! [`BlobStore::verify_blob_and_make_permanent`] (record-write path,
//! STRICT) and Arc 16f's [`BlobStore::verify_blob_tolerant_or_signal`]
//! (CAR-import path, TOLERANT). `apply_writes`' Phase B inner loop
//! calls `promote(...)` and branches on [`PromoteOutcome`] without
//! caring which discipline is in effect.
//!
//! The promoter is the load-bearing seam for Arc 16f's Path B (eager
//! re-fetch from origin via TOLERANT): the inner loop returns
//! `NeedsFetch` for the importing caller's fetch-and-retry loop to
//! consume per §9.6.3.5.
//!
//! **Signature note for skydeval** (extends design's pseudocode at
//! §9.6.3.4): the trait method takes `&BlobStore` and
//! `now: DateTime<Utc>` in addition to the design's `tx`/`cid`/
//! `record_uri` params. Both delegate targets (Arc 16b's STRICT and
//! Arc 16f's TOLERANT) are methods on `BlobStore` and accept `now` for
//! the `record_blob.indexed_at` write per Arc 16b §9.2.3.2 / Arc 16e
//! §9.5.3.2.2 round-4 F11 closure (`now` captured once at Phase B
//! entry, shared across all promotion calls). The trait method has to
//! plumb both through to the delegate.

use crate::blob_store::store::{BlobStore, QuarantinePublicReason, TolerantOutcome};
use crate::error::{PdsError, PdsResult};
use async_trait::async_trait;
use proto_blue::lex_data::Cid;

/// Arc 16f §9.6.3.4 — Phase B per-CID promotion abstraction.
///
/// `apply_writes`' inner loop holds an `Arc<dyn BlobPromoter>` and
/// calls `promote(...)` for each CID's promotion step; the impl
/// determines whether the absent-row case is an error (STRICT) or a
/// `NeedsFetch` signal (TOLERANT).
#[async_trait]
pub trait BlobPromoter: Send + Sync {
    /// Promote a CID's `blob_metadata` row state per the promoter's
    /// discipline. `Done` is the only outcome [`StrictPromoter`]
    /// returns; `NeedsFetch` and `Quarantined` are
    /// [`TolerantPromoter`]-only outcomes (STRICT surfaces those
    /// conditions as `PdsError` instead).
    async fn promote<'tx>(
        &self,
        blob_store: &BlobStore,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &Cid,
        record_uri: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<PromoteOutcome>;
}

/// Arc 16f §9.6.3.4 — outcome of [`BlobPromoter::promote`]. Phase B
/// branches on this:
///
/// - `Done` → continue inner loop;
/// - `NeedsFetch` → accumulate into pending-fetch set per §9.6.3.4
///   inner-loop discipline; propagated as
///   [`PdsError::NeedsBlobFetch`] after the inner loop drains;
/// - `Quarantined` → propagated as
///   [`PdsError::QuarantinedBlobReferenced`]; tx rolls back.
#[derive(Debug, Clone)]
pub enum PromoteOutcome {
    /// Promotion landed (or was already permanent). Inner loop
    /// continues.
    Done,
    /// `blob_metadata` row absent; caller must fetch from origin and
    /// retry. TolerantPromoter-only.
    NeedsFetch { cid: Cid },
    /// `blob_quarantine` entry exists for the CID. TolerantPromoter-
    /// only. STRICT surfaces this condition as
    /// [`PdsError::QuarantinedBlobReferenced`] instead — though in
    /// practice STRICT callers expect validate-phase to catch
    /// quarantine before promotion is attempted.
    Quarantined {
        cid: Cid,
        public_reason: QuarantinePublicReason,
    },
}

/// Arc 16f §9.6.3.4 — STRICT discipline marker. Used by Arc 16e's
/// `apply_writes` callers (createRecord / putRecord / deleteRecord /
/// applyWrites) where the absent-row case is a client-input bug
/// (`BlobNotFound`), not a fetch-from-origin signal.
///
/// Delegates to Arc 16b's [`BlobStore::verify_blob_and_make_permanent`]
/// — the same call site Arc 16e Step 2 wired into Phase B directly.
/// Arc 16f's signature extension routes the call through this
/// `Arc<dyn BlobPromoter>` indirection so the import path can swap
/// in `TolerantPromoter` without touching the per-CID loop body.
pub struct StrictPromoter;

#[async_trait]
impl BlobPromoter for StrictPromoter {
    async fn promote<'tx>(
        &self,
        blob_store: &BlobStore,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &Cid,
        record_uri: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<PromoteOutcome> {
        // Arc 16b's helper takes `cid: &str`; the typed Cid converts
        // via Display per proto_blue::lex_data::Cid::to_string_base32.
        let cid_str = cid.to_string();
        blob_store
            .verify_blob_and_make_permanent(tx, &cid_str, record_uri, now)
            .await
            .map(|()| PromoteOutcome::Done)
        // Any STRICT error (BlobNotFound on absent row, Database on
        // sqlx failure, etc.) propagates as-is — caller Phase B
        // rolls back the tx and surfaces the error.
    }
}

/// Arc 16f §9.6.3.4 — TOLERANT discipline marker. Used by the
/// importRepo handler (Path B per V05_DESIGN.md §9.6 header) where
/// absent-row signals `NeedsFetch` for the caller-driven re-fetch
/// loop.
///
/// Delegates to Arc 16f's
/// [`BlobStore::verify_blob_tolerant_or_signal`], which itself
/// delegates present-row branches to Arc 16b's STRICT (no
/// duplication of the present-row state machine).
pub struct TolerantPromoter;

#[async_trait]
impl BlobPromoter for TolerantPromoter {
    async fn promote<'tx>(
        &self,
        blob_store: &BlobStore,
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        cid: &Cid,
        record_uri: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> PdsResult<PromoteOutcome> {
        match blob_store
            .verify_blob_tolerant_or_signal(tx, cid, record_uri, now)
            .await?
        {
            TolerantOutcome::Promoted | TolerantOutcome::AlreadyPermanent => {
                Ok(PromoteOutcome::Done)
            }
            TolerantOutcome::NeedsFetch { cid } => Ok(PromoteOutcome::NeedsFetch { cid }),
            TolerantOutcome::Quarantined {
                cid,
                public_reason,
            } => Ok(PromoteOutcome::Quarantined {
                cid,
                public_reason,
            }),
        }
    }
}

// `PdsError` is imported above but only flows through `?`-propagation
// and `verify_blob_and_make_permanent`'s return type; the `use` keeps
// the rustdoc cross-link `[PdsError]` resolvable.
#[allow(dead_code)]
const _: Option<PdsError> = None;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_store::{BlobBackendType, BlobStorageConfig, BlobStoreConfig};
    use std::sync::Once;

    /// Mirror of `arc16b_store` + `blob_quarantine` schema from
    /// `src/blob_store/store.rs` tests. Duplicated here because the
    /// store-side fixture lives in a private `mod tests` and is not
    /// re-exportable.
    async fn promoter_fixture() -> (BlobStore, sqlx::AnyPool) {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE blob_metadata (
                cid TEXT PRIMARY KEY,
                mime_type TEXT NOT NULL,
                size INTEGER NOT NULL,
                creator_did TEXT NOT NULL,
                created_at TEXT NOT NULL,
                width INTEGER,
                height INTEGER,
                alt_text TEXT,
                thumbnail_cid TEXT,
                temp_key TEXT NULL CHECK (temp_key IS NULL OR temp_key = '1')
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE record_blob (
                blob_cid TEXT NOT NULL,
                record_uri TEXT NOT NULL,
                indexed_at TEXT NOT NULL,
                PRIMARY KEY (blob_cid, record_uri)
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE blob_quarantine (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cid TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                quarantined_by TEXT NOT NULL,
                quarantined_at TEXT NOT NULL,
                restored_at TEXT,
                restored_by TEXT,
                legal_reference TEXT
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config = BlobStoreConfig {
            storage: BlobStorageConfig {
                backend: BlobBackendType::Disk {
                    location: dir.path().to_path_buf(),
                },
                max_blob_size: 1024 * 1024,
                temp_dir: dir.path().join("tmp"),
            },
        };
        let store = BlobStore::new(config, pool.clone()).await.unwrap();
        (store, pool)
    }

    async fn seed_untethered_row(pool: &sqlx::AnyPool, cid: &str) {
        sqlx::query(
            "INSERT INTO blob_metadata (cid, mime_type, size, creator_did, created_at, temp_key) \
             VALUES ($1, 'image/png', 100, 'did:plc:alice', '2026-01-01T00:00:00Z', '1')",
        )
        .bind(cid)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn seed_quarantine(pool: &sqlx::AnyPool, cid: &str, reason: &str) {
        sqlx::query(
            "INSERT INTO blob_quarantine (cid, reason, quarantined_by, quarantined_at) \
             VALUES ($1, $2, 'did:plc:admin', '2026-01-01T00:00:00Z')",
        )
        .bind(cid)
        .bind(reason)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn strict_promoter_done_on_present_untethered_row() {
        let (store, pool) = promoter_fixture().await;
        let cid = Cid::for_raw(b"arc16f-promoter-strict-1");
        seed_untethered_row(&pool, &cid.to_string()).await;
        let mut tx = pool.begin().await.unwrap();
        let outcome = StrictPromoter
            .promote(
                &store,
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(matches!(outcome, PromoteOutcome::Done));
    }

    #[tokio::test]
    async fn strict_promoter_errors_blob_not_found_on_absent_row() {
        let (store, pool) = promoter_fixture().await;
        let cid = Cid::for_raw(b"arc16f-promoter-strict-absent");
        let mut tx = pool.begin().await.unwrap();
        let result = StrictPromoter
            .promote(
                &store,
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await;
        // STRICT surfaces absent-row as PdsError::BlobNotFound; never
        // returns NeedsFetch.
        match result {
            Err(PdsError::BlobNotFound(s)) => assert_eq!(s, cid.to_string()),
            other => panic!("expected BlobNotFound, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tolerant_promoter_needs_fetch_on_absent_row() {
        let (store, pool) = promoter_fixture().await;
        let cid = Cid::for_raw(b"arc16f-promoter-tolerant-nf");
        let mut tx = pool.begin().await.unwrap();
        let outcome = TolerantPromoter
            .promote(
                &store,
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        match outcome {
            PromoteOutcome::NeedsFetch { cid: c } => assert_eq!(c, cid),
            other => panic!("expected NeedsFetch, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn tolerant_promoter_done_on_present_row() {
        let (store, pool) = promoter_fixture().await;
        let cid = Cid::for_raw(b"arc16f-promoter-tolerant-done");
        seed_untethered_row(&pool, &cid.to_string()).await;
        let mut tx = pool.begin().await.unwrap();
        let outcome = TolerantPromoter
            .promote(
                &store,
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(matches!(outcome, PromoteOutcome::Done));
    }

    #[tokio::test]
    async fn tolerant_promoter_quarantined_takes_precedence() {
        let (store, pool) = promoter_fixture().await;
        let cid = Cid::for_raw(b"arc16f-promoter-tolerant-q");
        let cid_str = cid.to_string();
        seed_quarantine(&pool, &cid_str, "dmca").await;
        seed_untethered_row(&pool, &cid_str).await;
        let mut tx = pool.begin().await.unwrap();
        let outcome = TolerantPromoter
            .promote(
                &store,
                &mut tx,
                &cid,
                "at://did:plc:alice/coll/rkey",
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        match outcome {
            PromoteOutcome::Quarantined { cid: c, public_reason } => {
                assert_eq!(c, cid);
                assert_eq!(public_reason, QuarantinePublicReason::Legal);
            }
            other => panic!("expected Quarantined, got {:?}", other),
        }
    }
}
