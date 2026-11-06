//! Backup runs, the delete job's skip rule while one runs, SQLite
//! snapshots, and the database changes a restore makes.

use locus_core::{CoreError, CoreResult};

use crate::blob::{Place, sql};
use crate::{Store, Tx, db_err};

macro_rules! run {
    ($store:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
            Store::Postgres(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
        }
        .map_err(db_err)
    };
}

macro_rules! exec {
    ($store:expr, $q:expr, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
            Store::Postgres(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

macro_rules! tx_run {
    ($tx:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
        }
        .map_err(db_err)
    };
}

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

/// The lease a running backup holds.
pub const BACKUP_LEASE: &str = "backup";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BackupRun {
    pub id: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub dir: String,
    pub delete_mark: Option<i64>,
    pub objects: i64,
    pub missing: i64,
    pub with_keys: bool,
    pub started_by: String,
    pub error: Option<String>,
}

/// An account, as a restore sees it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AccountBrief {
    pub did: String,
    pub handle: String,
    pub status: String,
    pub did_method: String,
}

const RUN_COLS: &str = "id, started_at, finished_at, status, dir, delete_mark, objects, missing, \
     with_keys, started_by, error";

fn pg(s: &Store) -> bool {
    matches!(s, Store::Postgres(_))
}

impl Store {
    pub async fn start_backup_run(&self, id: &str, dir: &str, by: &str) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO backup_run (id, started_at, status, dir, started_by) VALUES ({1}, {now}, 'running', {2}, {3})",
            pg(self),
        );
        exec!(self, q, id, dir, by)?;
        Ok(())
    }

    pub async fn set_backup_mark(&self, id: &str, mark: i64) -> CoreResult<()> {
        let q = sql(
            "UPDATE backup_run SET delete_mark = {2} WHERE id = {1}",
            pg(self),
        );
        exec!(self, q, id, mark)?;
        Ok(())
    }

    pub async fn finish_backup_run(
        &self,
        id: &str,
        status: &str,
        objects: i64,
        missing: i64,
        with_keys: bool,
        error: Option<&str>,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE backup_run SET status = {2}, finished_at = {now}, objects = {3}, missing = {4}, \
             with_keys = {5}, error = {6} WHERE id = {1}",
            pg(self),
        );
        exec!(self, q, id, status, objects, missing, with_keys, error)?;
        Ok(())
    }

    /// The latest backup runs, newest first.
    pub async fn backup_runs(&self, limit: i64) -> CoreResult<Vec<BackupRun>> {
        let q = sql(
            &format!("SELECT {RUN_COLS} FROM backup_run ORDER BY started_at DESC LIMIT {{1}}"),
            pg(self),
        );
        run!(self, q, fetch_all, limit)
    }

    /// The delete job's rule while a backup runs: None when none does (or
    /// its lease lapsed); Some(None) until its delete mark is recorded
    /// (delete nothing); Some(Some(mark)) after (only rows up to it).
    pub async fn backup_delete_limit(&self) -> CoreResult<Option<Option<i64>>> {
        let q = sql(
            &format!(
                "SELECT r.delete_mark FROM backup_run r JOIN job_lease l ON l.name = '{BACKUP_LEASE}' \
                 AND l.expires_at > {{now}} WHERE r.status = 'running' ORDER BY r.started_at DESC LIMIT 1"
            ),
            pg(self),
        );
        let row: Option<(Option<i64>,)> = run!(self, q, fetch_optional,)?;
        Ok(row.map(|r| r.0))
    }

    /// A consistent copy of a SQLite database at `path` (refused on
    /// Postgres, whose backups are PITR).
    pub async fn sqlite_snapshot(&self, path: &std::path::Path) -> CoreResult<()> {
        let Store::Sqlite(p) = self else {
            return Err(CoreError::Invalid(
                "backup create is for SQLite installs; on Postgres, use point-in-time recovery and \
                 versioned buckets, with `aurora-locus keys backup` for the keys"
                    .into(),
            ));
        };
        let target = path
            .to_str()
            .ok_or_else(|| CoreError::Invalid("the backup path isn't UTF-8".into()))?;
        sqlx::query("VACUUM INTO ?1")
            .bind(target)
            .execute(p)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// The highest queued delete's id (0 for none).
    pub async fn max_blob_delete_id(&self) -> CoreResult<i64> {
        let q = "SELECT COALESCE(MAX(id), 0) FROM blob_delete";
        let (n,): (i64,) = run!(self, q, fetch_one,)?;
        Ok(n)
    }

    /// Every tracked storage key (blob and branding rows), after `after`,
    /// in order.
    pub async fn tracked_keys_after(&self, after: &str, limit: i64) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT k FROM (SELECT storage_key AS k FROM blob UNION SELECT storage_key AS k FROM branding_asset) t \
             WHERE k > {1} ORDER BY k LIMIT {2}",
            pg(self),
        );
        let rows: Vec<(String,)> = run!(self, q, fetch_all, after, limit)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Every pending move's target key, after `after`.
    pub async fn pending_keys_after(&self, after: &str, limit: i64) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT pending_move FROM blob WHERE pending_move IS NOT NULL AND pending_move > {1} \
             ORDER BY pending_move LIMIT {2}",
            pg(self),
        );
        let rows: Vec<(String,)> = run!(self, q, fetch_all, after, limit)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Accounts after `after` (by DID), as a restore re-announces them.
    pub async fn accounts_after(&self, after: &str, limit: i64) -> CoreResult<Vec<AccountBrief>> {
        let q = sql(
            "SELECT did, handle, status, did_method FROM account WHERE did > {1} ORDER BY did LIMIT {2}",
            pg(self),
        );
        run!(self, q, fetch_all, after, limit)
    }

    /// Whether `did` is a did:web this PDS serves the document of.
    pub async fn hosts_did_web(&self, did: &str) -> CoreResult<bool> {
        let q = sql("SELECT 1 FROM hosted_did_web WHERE did = {1}", pg(self));
        let r: Option<(i32,)> = run!(self, q, fetch_optional, did)?;
        Ok(r.is_some())
    }
}

impl Tx {
    /// After a restore or a PITR: the backend that holds the objects is
    /// active (with `disk_root`), not paused, no `migrate blobs` job, the epoch bumped when `new_backend`, and
    /// every queued delete aimed at it. Running backups the snapshot held
    /// are ended, and every lease it held released.
    pub async fn settle_restored_blobs(
        &mut self,
        active: &str,
        disk_root: Option<&str>,
        new_backend: bool,
    ) -> CoreResult<i64> {
        let pg = matches!(self, Tx::Postgres(_));
        let q = sql(
            "UPDATE blob_backend SET active = {1}, disk_root = {2}, paused = {3}, migration_job = NULL, \
             epoch = epoch + CASE WHEN {4} THEN 1 ELSE 0 END WHERE id = 1",
            pg,
        );
        tx_exec!(self, q, active, disk_root, false, new_backend)?;
        let (epoch,): (i64,) = tx_run!(
            self,
            "SELECT epoch FROM blob_backend WHERE id = 1",
            fetch_one,
        )?;
        let q = sql("UPDATE blob_delete SET backend_epoch = {1}", pg);
        tx_exec!(self, q, epoch)?;
        tx_exec!(
            self,
            "UPDATE backup_run SET status = 'ended' WHERE status = 'running'",
        )?;
        // A blob migration the snapshot held ends; its copy records may
        // name objects its target no longer has.
        tx_exec!(
            self,
            "UPDATE blob_migration SET phase = 'cancelled' WHERE phase IN ('copying', 'post_flip')",
        )?;
        tx_exec!(self, "DELETE FROM blob_copy",)?;
        // Every lease the restored rows hold belongs to a node of the old
        // install (clock ids, jobs, the backup's own); none runs now.
        tx_exec!(self, "DELETE FROM job_lease",)?;
        Ok(epoch)
    }

    /// Restarts every pending move from its `storage_key`: a fresh target
    /// key in the same store, the old target queued for deletion (its copy
    /// may be half-written or gone). Returns how many.
    pub async fn restart_pending_moves(&mut self, epoch: i64) -> CoreResult<u64> {
        let pg = matches!(self, Tx::Postgres(_));
        let q = "SELECT did, cid, pending_move FROM blob WHERE pending_move IS NOT NULL";
        let rows: Vec<(String, String, String)> = tx_run!(self, q, fetch_all,)?;
        for (did, cid, old) in &rows {
            let fresh = Place::of_key(old).new_key(did, cid);
            let q = sql(
                "UPDATE blob SET pending_move = {3}, move_epoch = {4} WHERE did = {1} AND cid = {2}",
                pg,
            );
            tx_exec!(self, q, did, cid, &fresh, epoch)?;
            self.queue_blob_delete(old, epoch).await?;
        }
        Ok(rows.len() as u64)
    }

    /// Makes the next seq at least `floor + 1`.
    pub async fn raise_seq_floor(&mut self, floor: i64) -> CoreResult<i64> {
        let pg = matches!(self, Tx::Postgres(_));
        let q = sql(
            "UPDATE seq_state SET last_seq = CASE WHEN last_seq < {1} THEN {1} ELSE last_seq END WHERE id = 1",
            pg,
        );
        tx_exec!(self, q, floor)?;
        let (n,): (i64,) = tx_run!(
            self,
            "SELECT last_seq FROM seq_state WHERE id = 1",
            fetch_one,
        )?;
        Ok(n)
    }
}
