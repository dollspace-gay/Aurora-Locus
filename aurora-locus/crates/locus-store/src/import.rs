//! Repository import staging: the per-job tables an import fills while it
//! checks the uploaded repository, and reads back in its later phases
//! (`increments`, `swap`, `reindex`, `decrements`, or a rollback), so each
//! phase resumes from staged rows after a crash.

use std::collections::HashMap;

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, Tx, db_err};

/// Import phases during which the import's blob CIDs count as referenced
/// (uploads promote them, GC skips them). Never `rolling_back`.
pub const ACTIVE_IMPORT_PHASES: &str = "('staging', 'increments', 'swap', 'reindex', 'decrements')";

/// A staged block count change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedBlock {
    pub cid: String,
    pub content: Option<Vec<u8>>,
    pub delta: i64,
}

/// A staged record change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRecord {
    pub collection: String,
    pub rkey: String,
    /// `create`, `update` or `delete`.
    pub change: String,
    pub new_cid: Option<String>,
    pub old_cid: Option<String>,
}

/// A staged blob CID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedBlob {
    pub cid: String,
    pub in_new_tree: bool,
    pub applied: bool,
}

/// An import job as `getImportStatus` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportStatus {
    pub id: String,
    pub phase: String,
    pub progress_done: i64,
    pub progress_total: i64,
    pub error: Option<String>,
    pub notes: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

type RecordRow = (String, String, String, Option<String>, Option<String>);

type StatusRow = (
    String,
    String,
    i64,
    i64,
    Option<String>,
    Option<String>,
    i64,
    i64,
);

macro_rules! on_tx {
    ($self:expr, $q:expr, |$ex:ident| $body:expr) => {
        match $self {
            Tx::Sqlite(t) => {
                let $ex = &mut **t;
                $body
            }
            Tx::Postgres(t) => {
                let $ex = &mut **t;
                $body
            }
        }
    };
}

macro_rules! on_pool {
    ($self:expr, |$pool:ident| $body:expr) => {
        match $self {
            Store::Sqlite($pool) => $body,
            Store::Postgres($pool) => $body,
        }
    };
}

impl Tx {
    fn pg(&self) -> bool {
        matches!(self, Tx::Postgres(_))
    }

    /// Stores blocks read from the uploaded CAR (first copy of a CID wins).
    pub async fn stage_car_blocks(
        &mut self,
        job: &str,
        blocks: &[(String, Vec<u8>)],
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO import_block (job_id, cid, content, delta) VALUES ({1}, {2}, {3}, 0) \
             ON CONFLICT (job_id, cid) DO NOTHING",
            self.pg(),
        );
        for (cid, bytes) in blocks {
            on_tx!(self, q, |ex| sqlx::query(&q)
                .bind(job)
                .bind(cid)
                .bind(bytes)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// Adds count changes, keeping the first bytes seen for a block.
    pub async fn add_import_deltas(&mut self, job: &str, deltas: &[StagedBlock]) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO import_block (job_id, cid, content, delta) VALUES ({1}, {2}, {3}, {4}) \
             ON CONFLICT (job_id, cid) DO UPDATE SET delta = import_block.delta + excluded.delta, \
             content = COALESCE(import_block.content, excluded.content)",
            self.pg(),
        );
        for d in deltas {
            on_tx!(self, q, |ex| sqlx::query(&q)
                .bind(job)
                .bind(&d.cid)
                .bind(d.content.as_deref())
                .bind(d.delta)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// Records the new tree's leaves (provisionally creates).
    pub async fn stage_new_leaves(
        &mut self,
        job: &str,
        leaves: &[(String, String, String)],
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO import_record (job_id, collection, rkey, change, new_cid) \
             VALUES ({1}, {2}, {3}, 'create', {4})",
            self.pg(),
        );
        for (collection, rkey, cid) in leaves {
            on_tx!(self, q, |ex| sqlx::query(&q)
                .bind(job)
                .bind(collection)
                .bind(rkey)
                .bind(cid)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// Merges the old tree's leaves into the diff: an identical leaf drops
    /// out, a changed one becomes an update, a missing one a delete.
    pub async fn stage_old_leaves(
        &mut self,
        job: &str,
        leaves: &[(String, String, String)],
    ) -> CoreResult<()> {
        let pg = self.pg();
        let same = sql(
            "DELETE FROM import_record WHERE job_id = {1} AND collection = {2} AND rkey = {3} AND new_cid = {4}",
            pg,
        );
        let changed = sql(
            "UPDATE import_record SET change = 'update', old_cid = {4} \
             WHERE job_id = {1} AND collection = {2} AND rkey = {3}",
            pg,
        );
        let gone = sql(
            "INSERT INTO import_record (job_id, collection, rkey, change, old_cid) \
             VALUES ({1}, {2}, {3}, 'delete', {4})",
            pg,
        );
        for (collection, rkey, cid) in leaves {
            for q in [&same, &changed, &gone] {
                let n = on_tx!(self, q, |ex| sqlx::query(q)
                    .bind(job)
                    .bind(collection)
                    .bind(rkey)
                    .bind(cid)
                    .execute(ex)
                    .await
                    .map(|r| r.rows_affected())
                    .map_err(db_err))?;
                if n > 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Blob CIDs the new tree's walked records reference, each with whether
    /// that reference is private-tier (a CID is private-only while all its
    /// references are).
    pub async fn stage_new_blobs(&mut self, job: &str, cids: &[(String, bool)]) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO import_blob (job_id, cid, in_new_tree, private_only) VALUES ({1}, {2}, TRUE, {3}) \
             ON CONFLICT (job_id, cid) DO UPDATE SET in_new_tree = TRUE, private_only = \
             CASE WHEN import_blob.in_new_tree THEN import_blob.private_only AND excluded.private_only \
             ELSE excluded.private_only END",
            self.pg(),
        );
        for (cid, private) in cids {
            on_tx!(self, q, |ex| sqlx::query(&q)
                .bind(job)
                .bind(cid)
                .bind(*private)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// Clears `private_only` on new-tree CIDs a public record the import
    /// leaves unchanged references (its subtree wasn't walked).
    pub async fn settle_import_private_only(&mut self, job: &str, did: &str) -> CoreResult<()> {
        let q = sql(
            &format!(
                "UPDATE import_blob SET private_only = FALSE WHERE job_id = {{1}} AND in_new_tree \
                 AND private_only AND EXISTS (SELECT 1 FROM record_blob rb WHERE rb.did = {{2}} \
                 AND rb.blob_cid = import_blob.cid AND rb.collection NOT IN ({}) \
                 AND NOT EXISTS (SELECT 1 FROM import_record ir WHERE ir.job_id = {{1}} \
                 AND ir.collection = rb.collection AND ir.rkey = rb.rkey))",
                crate::scan::PRIVATE_COLLECTIONS
            ),
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(did)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// An upload's reference check, under the repo lock: a blob records
    /// reference, or an active import's new tree does, is made permanent at
    /// once, its gating decided (bytes to the gated store if private-only
    /// with the gate on), and the import's row marked `applied` so a
    /// rollback marks it unreferenced. Returns the move to finish.
    pub async fn promote_upload(
        &mut self,
        did: &str,
        cid: &str,
        gate_on: bool,
        epoch: i64,
    ) -> CoreResult<Option<crate::blob::PendingMove>> {
        let import = self.active_import_for_blob(did, cid).await?;
        if import.is_none() && !self.blob_has_records(did, cid).await? {
            return Ok(None);
        }
        let key = crate::blob::permanent_key(did, cid);
        let mut out = None;
        if self.reference_blob(did, cid, &key, epoch).await?.is_some() {
            self.recompute_private_only(did, cid).await?;
            self.place_blob(did, cid, gate_on, epoch).await?;
            if let Some(r) = self.blob_for_update(did, cid).await?
                && let Some(to) = r.pending_move
            {
                out = Some(crate::blob::PendingMove {
                    did: did.to_owned(),
                    cid: cid.to_owned(),
                    from: r.storage_key,
                    to,
                });
            }
        }
        if let Some(job) = import {
            self.mark_import_blob(&job, cid, true).await?;
        }
        Ok(out)
    }

    /// Blob CIDs of the old tree's changed and deleted records (from their
    /// `record_blob` rows), unless the new tree references them.
    pub async fn stage_old_blobs(&mut self, job: &str, did: &str) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO import_blob (job_id, cid, in_new_tree) \
             SELECT DISTINCT {1}, rb.blob_cid, FALSE FROM record_blob rb JOIN import_record ir \
             ON ir.job_id = {1} AND ir.collection = rb.collection AND ir.rkey = rb.rkey \
             AND ir.change <> 'create' WHERE rb.did = {2} \
             ON CONFLICT (job_id, cid) DO NOTHING",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(did)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    pub async fn set_import_rev(&mut self, job: &str, rev: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE bulk_job SET new_rev = {2} WHERE id = {1}",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(rev)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    pub async fn set_bulk_counts(&mut self, job: &str, done: i64, total: i64) -> CoreResult<()> {
        let q = sql(
            "UPDATE bulk_job SET progress_done = {2}, progress_total = {3}, updated_at = {now} WHERE id = {1}",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(done)
            .bind(total)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Records why a job is failing (kept by a resumer that finishes the
    /// rollback).
    pub async fn set_bulk_error(&mut self, job: &str, error: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE bulk_job SET error = COALESCE(error, {2}) WHERE id = {1}",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(error)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    pub async fn set_bulk_notes(&mut self, job: &str, notes: &str) -> CoreResult<()> {
        let q = sql("UPDATE bulk_job SET notes = {2} WHERE id = {1}", self.pg());
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(notes)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Staged blocks to apply: unapplied with a positive delta (`positive`)
    /// or a negative one, or (`applied_positive`) applied positive ones
    /// for a rollback to undo.
    pub async fn import_blocks_batch(
        &mut self,
        job: &str,
        kind: BlockBatch,
        limit: i64,
    ) -> CoreResult<Vec<StagedBlock>> {
        let cond = match kind {
            BlockBatch::Positive => "delta > 0 AND NOT applied",
            BlockBatch::Negative => "delta < 0 AND NOT applied",
            BlockBatch::AppliedPositive => "delta > 0 AND applied",
        };
        let q = sql(
            &format!(
                "SELECT cid, content, delta FROM import_block WHERE job_id = {{1}} AND {cond} \
                 ORDER BY cid LIMIT {{2}}"
            ),
            self.pg(),
        );
        let rows: Vec<(String, Option<Vec<u8>>, i64)> = on_tx!(self, q, |ex| sqlx::query_as(&q)
            .bind(job)
            .bind(limit)
            .fetch_all(ex)
            .await
            .map_err(db_err))?;
        Ok(rows
            .into_iter()
            .map(|(cid, content, delta)| StagedBlock {
                cid,
                content,
                delta,
            })
            .collect())
    }

    pub async fn mark_import_blocks(
        &mut self,
        job: &str,
        cids: &[String],
        applied: bool,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE import_block SET applied = {3} WHERE job_id = {1} AND cid = {2}",
            self.pg(),
        );
        for cid in cids {
            on_tx!(self, q, |ex| sqlx::query(&q)
                .bind(job)
                .bind(cid)
                .bind(applied)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// A page of the staged blob CIDs after `after`, in CID order.
    pub async fn import_blobs_page(
        &mut self,
        job: &str,
        after: &str,
        limit: i64,
    ) -> CoreResult<Vec<StagedBlob>> {
        let q = sql(
            "SELECT cid, in_new_tree, applied FROM import_blob WHERE job_id = {1} AND cid > {2} \
             ORDER BY cid LIMIT {3}",
            self.pg(),
        );
        let rows: Vec<(String, bool, bool)> = on_tx!(self, q, |ex| sqlx::query_as(&q)
            .bind(job)
            .bind(after)
            .bind(limit)
            .fetch_all(ex)
            .await
            .map_err(db_err))?;
        Ok(rows
            .into_iter()
            .map(|(cid, in_new_tree, applied)| StagedBlob {
                cid,
                in_new_tree,
                applied,
            })
            .collect())
    }

    /// Applied blob CIDs, for a rollback.
    pub async fn applied_import_blobs(&mut self, job: &str, limit: i64) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT cid FROM import_blob WHERE job_id = {1} AND applied ORDER BY cid LIMIT {2}",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query_scalar(&q)
            .bind(job)
            .bind(limit)
            .fetch_all(ex)
            .await
            .map_err(db_err))
    }

    pub async fn mark_import_blob(
        &mut self,
        job: &str,
        cid: &str,
        applied: bool,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE import_blob SET applied = {3} WHERE job_id = {1} AND cid = {2}",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(cid)
            .bind(applied)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Unapplied record changes, in key order.
    pub async fn pending_import_records(
        &mut self,
        job: &str,
        limit: i64,
    ) -> CoreResult<Vec<StagedRecord>> {
        let q = sql(
            "SELECT collection, rkey, change, new_cid, old_cid FROM import_record \
             WHERE job_id = {1} AND NOT applied ORDER BY collection, rkey LIMIT {2}",
            self.pg(),
        );
        let rows: Vec<RecordRow> = on_tx!(self, q, |ex| sqlx::query_as(&q)
            .bind(job)
            .bind(limit)
            .fetch_all(ex)
            .await
            .map_err(db_err))?;
        Ok(rows
            .into_iter()
            .map(
                |(collection, rkey, change, new_cid, old_cid)| StagedRecord {
                    collection,
                    rkey,
                    change,
                    new_cid,
                    old_cid,
                },
            )
            .collect())
    }

    pub async fn mark_import_record(
        &mut self,
        job: &str,
        collection: &str,
        rkey: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE import_record SET applied = TRUE WHERE job_id = {1} AND collection = {2} AND rkey = {3}",
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query(&q)
            .bind(job)
            .bind(collection)
            .bind(rkey)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// The staged bytes of the given blocks (those that have bytes).
    pub async fn staged_contents(
        &mut self,
        job: &str,
        cids: &[String],
    ) -> CoreResult<HashMap<String, Vec<u8>>> {
        let q = sql(
            "SELECT content FROM import_block WHERE job_id = {1} AND cid = {2} AND content IS NOT NULL",
            self.pg(),
        );
        let mut out = HashMap::new();
        for cid in cids {
            let bytes: Option<Vec<u8>> = on_tx!(self, q, |ex| sqlx::query_scalar(&q)
                .bind(job)
                .bind(cid)
                .fetch_optional(ex)
                .await
                .map_err(db_err))?;
            if let Some(b) = bytes {
                out.insert(cid.clone(), b);
            }
        }
        Ok(out)
    }

    /// Removes up to `limit` staging rows of a job. Returns how many went.
    pub async fn drop_import_staging(&mut self, job: &str, limit: i64) -> CoreResult<u64> {
        let pg = self.pg();
        let mut total = 0;
        for (table, key) in [
            ("import_block", "cid"),
            ("import_record", "collection || '/' || rkey"),
            ("import_blob", "cid"),
        ] {
            let q = sql(
                &format!(
                    "DELETE FROM {table} WHERE job_id = {{1}} AND {key} IN \
                     (SELECT {key} FROM {table} WHERE job_id = {{1}} LIMIT {{2}})"
                ),
                pg,
            );
            total += on_tx!(self, q, |ex| sqlx::query(&q)
                .bind(job)
                .bind(limit)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err))?;
            if total > 0 {
                return Ok(total);
            }
        }
        Ok(total)
    }

    /// The active import job whose new tree references this blob, if any.
    pub async fn active_import_for_blob(
        &mut self,
        did: &str,
        cid: &str,
    ) -> CoreResult<Option<String>> {
        let q = sql(
            &format!(
                "SELECT j.id FROM bulk_job j JOIN import_blob b ON b.job_id = j.id \
                 WHERE j.did = {{1}} AND j.kind = 'import' AND j.phase IN {ACTIVE_IMPORT_PHASES} \
                 AND b.cid = {{2}} AND b.in_new_tree LIMIT 1"
            ),
            self.pg(),
        );
        on_tx!(self, q, |ex| sqlx::query_scalar(&q)
            .bind(did)
            .bind(cid)
            .fetch_optional(ex)
            .await
            .map_err(db_err))
    }

    /// Whether any record of the account references the blob.
    pub async fn blob_has_records(&mut self, did: &str, cid: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM record_blob WHERE did = {1} AND blob_cid = {2} LIMIT 1",
            self.pg(),
        );
        let r: Option<i32> = on_tx!(self, q, |ex| sqlx::query_scalar(&q)
            .bind(did)
            .bind(cid)
            .fetch_optional(ex)
            .await
            .map_err(db_err))?;
        Ok(r.is_some())
    }
}

/// Which staged blocks [`Tx::import_blocks_batch`] returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockBatch {
    Positive,
    Negative,
    AppliedPositive,
}

impl Store {
    fn pg(&self) -> bool {
        matches!(self, Store::Postgres(_))
    }

    /// The staged bytes of one block.
    pub async fn staged_block(&self, job: &str, cid: &str) -> CoreResult<Option<Vec<u8>>> {
        let q = sql(
            "SELECT content FROM import_block WHERE job_id = {1} AND cid = {2} AND content IS NOT NULL",
            self.pg(),
        );
        let r: Option<Vec<u8>> = on_pool!(self, |p| sqlx::query_scalar(&q)
            .bind(job)
            .bind(cid)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r)
    }

    /// Whether a block is staged at all (with or without bytes).
    pub async fn is_staged(&self, job: &str, cid: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM import_block WHERE job_id = {1} AND cid = {2}",
            self.pg(),
        );
        let r: Option<i32> = on_pool!(self, |p| sqlx::query_scalar(&q)
            .bind(job)
            .bind(cid)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r.is_some())
    }

    /// Whether a block is in the account's current tree (counted).
    pub async fn is_live_block(&self, did: &str, cid: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM repo_block WHERE did = {1} AND cid = {2} AND ref_count > 0",
            self.pg(),
        );
        let r: Option<i32> = on_pool!(self, |p| sqlx::query_scalar(&q)
            .bind(did)
            .bind(cid)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r.is_some())
    }

    pub async fn import_record_count(&self, job: &str) -> CoreResult<i64> {
        let q = sql(
            "SELECT COUNT(*) FROM import_record WHERE job_id = {1}",
            self.pg(),
        );
        on_pool!(self, |p| sqlx::query_scalar(&q)
            .bind(job)
            .fetch_one(p)
            .await
            .map_err(db_err))
    }

    pub async fn bulk_job_error(&self, job: &str) -> CoreResult<Option<String>> {
        let q = sql("SELECT error FROM bulk_job WHERE id = {1}", self.pg());
        let r: Option<Option<String>> = on_pool!(self, |p| sqlx::query_scalar(&q)
            .bind(job)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r.flatten())
    }

    pub async fn bulk_job_rev(&self, job: &str) -> CoreResult<Option<String>> {
        let q = sql("SELECT new_rev FROM bulk_job WHERE id = {1}", self.pg());
        let r: Option<Option<String>> = on_pool!(self, |p| sqlx::query_scalar(&q)
            .bind(job)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r.flatten())
    }

    /// The account's latest import job.
    pub async fn latest_import(&self, did: &str) -> CoreResult<Option<ImportStatus>> {
        let q = sql(
            "SELECT id, phase, progress_done, progress_total, error, notes, created_at, updated_at \
             FROM bulk_job WHERE did = {1} AND kind = 'import' ORDER BY created_at DESC, id DESC LIMIT 1",
            self.pg(),
        );
        let r: Option<StatusRow> = on_pool!(self, |p| sqlx::query_as(&q)
            .bind(did)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r.map(|r| ImportStatus {
            id: r.0,
            phase: r.1,
            progress_done: r.2,
            progress_total: r.3,
            error: r.4,
            notes: r.5,
            created_at: r.6,
            updated_at: r.7,
        }))
    }

    /// Blobs records reference that have no blob row, one record per blob,
    /// in CID order after `cursor` (as the reference's `listMissingBlobs`).
    pub async fn list_missing_blobs(
        &self,
        did: &str,
        cursor: Option<&str>,
        limit: i64,
    ) -> CoreResult<Vec<(String, String, String)>> {
        let q = sql(
            "SELECT rb.blob_cid, MIN(rb.collection || '/' || rb.rkey) FROM record_blob rb \
             WHERE rb.did = {1} AND rb.blob_cid > {2} \
             AND NOT EXISTS (SELECT 1 FROM blob b WHERE b.did = rb.did AND b.cid = rb.blob_cid) \
             GROUP BY rb.blob_cid ORDER BY rb.blob_cid LIMIT {3}",
            self.pg(),
        );
        let rows: Vec<(String, String)> = on_pool!(self, |p| sqlx::query_as(&q)
            .bind(did)
            .bind(cursor.unwrap_or(""))
            .bind(limit)
            .fetch_all(p)
            .await
            .map_err(db_err))?;
        Ok(rows
            .into_iter()
            .map(|(cid, path)| {
                let (collection, rkey) = path.split_once('/').unwrap_or((path.as_str(), ""));
                (cid.clone(), collection.to_owned(), rkey.to_owned())
            })
            .collect())
    }
}
