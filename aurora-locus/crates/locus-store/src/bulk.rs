//! Bulk jobs: persisted, resumable work on one account that never runs as
//! one long transaction (account deletion; later imports and rebuilds).
//!
//! A job sets the account's write block when it starts (refused if another
//! job holds it), holds itself like a lease, and fences every batch: a
//! batch commits only while `holder`, `epoch` and an unexpired
//! `expires_at` still match, together with the job's progress.

use locus_core::{CoreError, CoreResult};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// A job's own lease. Renewed every batch; a resumer takes over a job
/// whose lease ran out.
pub const BULK_JOB_TTL_MS: i64 = 90_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkJob {
    pub id: String,
    pub did: String,
    pub kind: String,
    pub phase: String,
    pub cursor: Option<String>,
    pub holder: String,
    pub epoch: i64,
}

type JobRow = (String, String, String, String, Option<String>, String, i64);

fn job(r: JobRow) -> BulkJob {
    BulkJob {
        id: r.0,
        did: r.1,
        kind: r.2,
        phase: r.3,
        cursor: r.4,
        holder: r.5,
        epoch: r.6,
    }
}

const JOB_COLS: &str = "id, did, kind, phase, cursor, holder, epoch";

/// The 503 a blocked write gets (retryable).
#[must_use]
pub fn write_blocked() -> CoreError {
    CoreError::Conflict("RepoWriteBlocked".into())
}

macro_rules! on_tx {
    ($self:expr, $sqlite:expr, $pg:expr, |$q:ident, $ex:ident| $body:expr) => {
        match $self {
            Tx::Sqlite(t) => {
                let $q: &str = &$sqlite;
                let $ex = &mut **t;
                $body
            }
            Tx::Postgres(t) => {
                let $q: &str = &$pg;
                let $ex = &mut **t;
                $body
            }
        }
    };
}

macro_rules! on_pool {
    ($self:expr, $sqlite:expr, $pg:expr, |$q:ident, $pool:ident| $body:expr) => {
        match $self {
            Store::Sqlite($pool) => {
                let $q: &str = &$sqlite;
                $body
            }
            Store::Postgres($pool) => {
                let $q: &str = &$pg;
                $body
            }
        }
    };
}

impl Tx {
    /// Starts a job held by `holder`: sets the account's write block (only
    /// if unset, or already this job's) and records the job. Another job's
    /// block is [`write_blocked`].
    pub async fn start_bulk_job(
        &mut self,
        id: &str,
        did: &str,
        kind: &str,
        phase: &str,
        holder: &str,
    ) -> CoreResult<BulkJob> {
        let blocked = on_tx!(
            self,
            "UPDATE account SET write_block_owner = ?2, write_block_op = ?3 \
             WHERE did = ?1 AND (write_block_op IS NULL OR write_block_op = ?3)",
            "UPDATE account SET write_block_owner = $2, write_block_op = $3 \
             WHERE did = $1 AND (write_block_op IS NULL OR write_block_op = $3)",
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(kind)
                .bind(id)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err)
        )?;
        if blocked != 1 {
            return Err(write_blocked());
        }
        let ins = |now: &str, p: [&str; 5]| {
            format!(
                "INSERT INTO bulk_job (id, did, kind, phase, holder, epoch, expires_at, created_at, updated_at) \
                 VALUES ({}, {}, {}, {}, {}, 1, {now} + {BULK_JOB_TTL_MS}, {now}, {now})",
                p[0], p[1], p[2], p[3], p[4]
            )
        };
        on_tx!(
            self,
            ins(SQLITE_NOW_MS, ["?1", "?2", "?3", "?4", "?5"]),
            ins(PG_NOW_MS, ["$1", "$2", "$3", "$4", "$5"]),
            |q, ex| sqlx::query(q)
                .bind(id)
                .bind(did)
                .bind(kind)
                .bind(phase)
                .bind(holder)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )?;
        Ok(BulkJob {
            id: id.to_owned(),
            did: did.to_owned(),
            kind: kind.to_owned(),
            phase: phase.to_owned(),
            cursor: None,
            holder: holder.to_owned(),
            epoch: 1,
        })
    }

    /// Fails with [`CoreError::LeaseLost`] unless `job` still holds itself.
    pub async fn fence_bulk_job(&mut self, job: &BulkJob) -> CoreResult<()> {
        let held: Option<i64> = on_tx!(
            self,
            format!(
                "SELECT 1 FROM bulk_job WHERE id = ?1 AND holder = ?2 AND epoch = ?3 AND expires_at > {SQLITE_NOW_MS}"
            ),
            format!(
                "SELECT 1::bigint FROM bulk_job WHERE id = $1 AND holder = $2 AND epoch = $3 \
                 AND expires_at > {PG_NOW_MS} FOR UPDATE"
            ),
            |q, ex| sqlx::query_scalar(q)
                .bind(&job.id)
                .bind(&job.holder)
                .bind(job.epoch)
                .fetch_optional(ex)
                .await
                .map_err(db_err)
        )?;
        held.map(|_| ()).ok_or_else(|| {
            CoreError::LeaseLost(format!("bulk job {} (epoch {})", job.id, job.epoch))
        })
    }

    pub async fn set_bulk_progress(
        &mut self,
        id: &str,
        phase: &str,
        cursor: Option<&str>,
    ) -> CoreResult<()> {
        on_tx!(
            self,
            format!(
                "UPDATE bulk_job SET phase = ?2, cursor = ?3, updated_at = {SQLITE_NOW_MS} WHERE id = ?1"
            ),
            format!(
                "UPDATE bulk_job SET phase = $2, cursor = $3, updated_at = {PG_NOW_MS} WHERE id = $1"
            ),
            |q, ex| sqlx::query(q)
                .bind(id)
                .bind(phase)
                .bind(cursor)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Ends a job (`done` or `failed`) and clears the account's write block
    /// if it's still this job's.
    pub async fn finish_bulk_job(
        &mut self,
        id: &str,
        phase: &str,
        error: Option<&str>,
    ) -> CoreResult<()> {
        on_tx!(
            self,
            format!(
                "UPDATE bulk_job SET phase = ?2, error = ?3, updated_at = {SQLITE_NOW_MS} WHERE id = ?1"
            ),
            format!(
                "UPDATE bulk_job SET phase = $2, error = $3, updated_at = {PG_NOW_MS} WHERE id = $1"
            ),
            |q, ex| sqlx::query(q)
                .bind(id)
                .bind(phase)
                .bind(error)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )?;
        on_tx!(
            self,
            "UPDATE account SET write_block_owner = NULL, write_block_op = NULL WHERE write_block_op = ?1",
            "UPDATE account SET write_block_owner = NULL, write_block_op = NULL WHERE write_block_op = $1",
            |q, ex| sqlx::query(q)
                .bind(id)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Whether the account's writes are blocked by a bulk job.
    pub async fn write_blocked(&mut self, did: &str) -> CoreResult<bool> {
        let op: Option<Option<String>> = on_tx!(
            self,
            "SELECT write_block_op FROM account WHERE did = ?1",
            "SELECT write_block_op FROM account WHERE did = $1",
            |q, ex| sqlx::query_scalar(q)
                .bind(did)
                .fetch_optional(ex)
                .await
                .map_err(db_err)
        )?;
        Ok(matches!(op, Some(Some(_))))
    }

    /// One batch of an account deletion: removes up to `limit` rows of the
    /// account's data, table by table. Returns how many went; 0 means only
    /// the final step is left ([`Tx::delete_account_finally`]).
    pub async fn delete_account_batch(&mut self, did: &str, limit: i64) -> CoreResult<u64> {
        // Large tables in bounded slices.
        // Blobs first: their keys are queued for deletion in this batch.
        let epoch = self.blob_backend().await?.epoch;
        let n = self.delete_account_blobs(did, limit, epoch).await?;
        if n > 0 {
            return Ok(n);
        }
        let sliced = [
            "DELETE FROM repo_block WHERE did = {1} AND cid IN (SELECT cid FROM repo_block WHERE did = {1} LIMIT {2})",
            "DELETE FROM record WHERE did = {1} AND (collection, rkey) IN \
             (SELECT collection, rkey FROM record WHERE did = {1} LIMIT {2})",
            "DELETE FROM record_blob WHERE did = {1} AND (blob_cid, collection, rkey) IN \
             (SELECT blob_cid, collection, rkey FROM record_blob WHERE did = {1} LIMIT {2})",
            "DELETE FROM backlink WHERE did = {1} AND (collection, rkey, path) IN \
             (SELECT collection, rkey, path FROM backlink WHERE did = {1} LIMIT {2})",
            "DELETE FROM kr_private_record WHERE did = {1} AND (collection, rkey) IN \
             (SELECT collection, rkey FROM kr_private_record WHERE did = {1} LIMIT {2})",
            "DELETE FROM kr_audience WHERE did = {1} AND rkey IN \
             (SELECT rkey FROM kr_audience WHERE did = {1} LIMIT {2})",
        ];
        for template in sliced {
            let sqlite = template.replace("{1}", "?1").replace("{2}", "?2");
            let pg = template.replace("{1}", "$1").replace("{2}", "$2");
            let n = on_tx!(self, sqlite, pg, |q, ex| sqlx::query(q)
                .bind(did)
                .bind(limit)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err))?;
            if n > 0 {
                return Ok(n);
            }
        }
        // Small per-account tables at once.
        let mut total = 0;
        for table_col in [
            "session WHERE did",
            "app_password WHERE did",
            "invite_code_use WHERE used_by",
            "invite_code WHERE for_account",
            "email_token WHERE did",
            "plc_key_listing WHERE did",
            "identity_repair WHERE did",
            "did_cache WHERE did",
            "account_pref WHERE did",
            "oauth_used_refresh WHERE token_id IN (SELECT id FROM oauth_token WHERE did",
            "oauth_token WHERE did",
            "oauth_account_device WHERE did",
            "oauth_authorized_client WHERE did",
            "kr_permission WHERE did",
            "kr_audience_alias WHERE did",
            "kr_audience_tombstone WHERE did",
            "kr_audience_attempt WHERE did",
            "kr_audience_sync WHERE did",
        ] {
            let close = if table_col.contains("(SELECT") {
                ")"
            } else {
                ""
            };
            let sqlite = format!("DELETE FROM {table_col} = ?1{close}");
            let pg = format!("DELETE FROM {table_col} = $1{close}");
            total += on_tx!(self, sqlite, pg, |q, ex| sqlx::query(q)
                .bind(did)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err))?;
        }
        Ok(total)
    }

    /// The last step of an account deletion: the repository root, the
    /// signing key and the account row go; a hosted did:web document is
    /// emptied but its row kept, so the host stays tied to the DID.
    pub async fn delete_account_finally(&mut self, did: &str) -> CoreResult<()> {
        for table in ["repo_root", "signing_key", "account"] {
            let sqlite = format!("DELETE FROM {table} WHERE did = ?1");
            let pg = format!("DELETE FROM {table} WHERE did = $1");
            on_tx!(self, sqlite, pg, |q, ex| sqlx::query(q)
                .bind(did)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        self.set_hosted_did_web_doc(did, None).await?;
        Ok(())
    }

    /// Deactivates an account (by its holder): an active account becomes
    /// `deactivated`; a taken-down or suspended one keeps that status, but
    /// is marked deactivated too. Returns the resulting status, or None if
    /// there's no such account.
    pub async fn deactivate_account(
        &mut self,
        did: &str,
        delete_after: Option<i64>,
    ) -> CoreResult<Option<crate::accounts::Status>> {
        let status: Option<String> = on_tx!(
            self,
            format!(
                "UPDATE account SET deactivated_at = {SQLITE_NOW_MS}, delete_after = ?2, \
                 status = CASE WHEN status = 'active' THEN 'deactivated' ELSE status END \
                 WHERE did = ?1 AND status <> 'deleted' RETURNING status"
            ),
            format!(
                "UPDATE account SET deactivated_at = {PG_NOW_MS}, delete_after = $2, \
                 status = CASE WHEN status = 'active' THEN 'deactivated' ELSE status END \
                 WHERE did = $1 AND status <> 'deleted' RETURNING status"
            ),
            |q, ex| sqlx::query_scalar(q)
                .bind(did)
                .bind(delete_after)
                .fetch_optional(ex)
                .await
                .map_err(db_err)
        )?;
        status
            .map(|s| crate::accounts::Status::parse(&s))
            .transpose()
    }

    /// Reactivates a deactivated or active account; a taken-down,
    /// suspended or deleted one can't be. Returns whether it changed.
    pub async fn activate_account(&mut self, did: &str) -> CoreResult<bool> {
        let n = on_tx!(
            self,
            "UPDATE account SET deactivated_at = NULL, delete_after = NULL, status = 'active' \
             WHERE did = ?1 AND status IN ('active', 'deactivated')",
            "UPDATE account SET deactivated_at = NULL, delete_after = NULL, status = 'active' \
             WHERE did = $1 AND status IN ('active', 'deactivated')",
            |q, ex| sqlx::query(q)
                .bind(did)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err)
        )?;
        Ok(n == 1)
    }
}

impl Store {
    /// Extends a job's own lease; false if it isn't this holder's any more.
    pub async fn renew_bulk_job(&self, job: &BulkJob) -> CoreResult<bool> {
        let n = on_pool!(
            self,
            format!(
                "UPDATE bulk_job SET expires_at = {SQLITE_NOW_MS} + {BULK_JOB_TTL_MS} \
                 WHERE id = ?1 AND holder = ?2 AND epoch = ?3 AND expires_at > {SQLITE_NOW_MS}"
            ),
            format!(
                "UPDATE bulk_job SET expires_at = {PG_NOW_MS} + {BULK_JOB_TTL_MS} \
                 WHERE id = $1 AND holder = $2 AND epoch = $3 AND expires_at > {PG_NOW_MS}"
            ),
            |q, p| sqlx::query(q)
                .bind(&job.id)
                .bind(&job.holder)
                .bind(job.epoch)
                .execute(p)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err)
        )?;
        Ok(n == 1)
    }

    /// Takes over one unfinished job whose lease ran out (never a job a live
    /// node still holds), bumping its epoch.
    pub async fn take_over_bulk_job(&self, holder: &str) -> CoreResult<Option<BulkJob>> {
        let row: Option<JobRow> = on_pool!(
            self,
            format!(
                "UPDATE bulk_job SET holder = ?1, epoch = epoch + 1, \
                 expires_at = {SQLITE_NOW_MS} + {BULK_JOB_TTL_MS}, updated_at = {SQLITE_NOW_MS} \
                 WHERE id = (SELECT id FROM bulk_job WHERE phase NOT IN ('done', 'failed') \
                 AND expires_at <= {SQLITE_NOW_MS} ORDER BY expires_at LIMIT 1) \
                 RETURNING {JOB_COLS}"
            ),
            format!(
                "UPDATE bulk_job SET holder = $1, epoch = epoch + 1, \
                 expires_at = {PG_NOW_MS} + {BULK_JOB_TTL_MS}, updated_at = {PG_NOW_MS} \
                 WHERE id = (SELECT id FROM bulk_job WHERE phase NOT IN ('done', 'failed') \
                 AND expires_at <= {PG_NOW_MS} ORDER BY expires_at LIMIT 1 FOR UPDATE SKIP LOCKED) \
                 RETURNING {JOB_COLS}"
            ),
            |q, p| sqlx::query_as(q)
                .bind(holder)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(job))
    }

    pub async fn bulk_job(&self, id: &str) -> CoreResult<Option<BulkJob>> {
        let row: Option<JobRow> = on_pool!(
            self,
            format!("SELECT {JOB_COLS} FROM bulk_job WHERE id = ?1"),
            format!("SELECT {JOB_COLS} FROM bulk_job WHERE id = $1"),
            |q, p| sqlx::query_as(q)
                .bind(id)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(job))
    }

    /// Deactivated accounts whose `deleteAfter` has passed and that no bulk
    /// job holds.
    pub async fn accounts_due_for_deletion(&self, limit: i64) -> CoreResult<Vec<String>> {
        on_pool!(
            self,
            format!(
                "SELECT did FROM account WHERE delete_after IS NOT NULL AND delete_after <= {SQLITE_NOW_MS} \
                 AND deactivated_at IS NOT NULL AND status <> 'deleted' AND write_block_op IS NULL LIMIT ?1"
            ),
            format!(
                "SELECT did FROM account WHERE delete_after IS NOT NULL AND delete_after <= {PG_NOW_MS} \
                 AND deactivated_at IS NOT NULL AND status <> 'deleted' AND write_block_op IS NULL LIMIT $1"
            ),
            |q, p| sqlx::query_scalar(q)
                .bind(limit)
                .fetch_all(p)
                .await
                .map_err(db_err)
        )
    }

    /// Runs (or resumes) an account deletion job to the end, fencing every
    /// batch and renewing the job between batches.
    pub async fn run_delete_job(&self, job: &BulkJob, batch: i64) -> CoreResult<()> {
        loop {
            if !self.renew_bulk_job(job).await? {
                return Err(CoreError::LeaseLost(format!("bulk job {}", job.id)));
            }
            let mut tx = self.begin().await?;
            tx.fence_bulk_job(job).await?;
            let n = tx.delete_account_batch(&job.did, batch).await?;
            if n == 0 {
                tx.delete_account_finally(&job.did).await?;
                tx.finish_bulk_job(&job.id, "done", None).await?;
                tx.commit().await?;
                return Ok(());
            }
            tx.set_bulk_progress(&job.id, "deleting", None).await?;
            tx.commit().await?;
        }
    }
}
