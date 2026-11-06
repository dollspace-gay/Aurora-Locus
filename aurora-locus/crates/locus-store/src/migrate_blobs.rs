//! `migrate blobs`: the job's row, its copy records, the mark each pass
//! starts with, and the pause, flip and cancel transactions. Every one of
//! them is fenced on the job's lease. The flip reads no blob rows and does
//! no blob-store I/O.

use locus_core::lease::LeaseToken;
use locus_core::{CoreError, CoreResult};

use crate::blob::sql;
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

/// The lease the migration job runs under.
pub const MIGRATE_LEASE: &str = "migrate-blobs";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BlobMigration {
    pub id: String,
    pub target: String,
    pub disk_root: Option<String>,
    pub phase: String,
    pub copied_through: i64,
    pub verified_through: i64,
    pub passes: i64,
    pub started_at: i64,
    pub started_by: String,
    pub finished_at: Option<i64>,
    pub note: Option<String>,
}

/// A tracked row a pass copies: its key, CID (None for a branding
/// upload) and key sequence value.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Tracked {
    pub storage_key: String,
    pub cid: Option<String>,
    pub key_seq: i64,
}

const COLS: &str = "id, target, disk_root, phase, copied_through, verified_through, passes, started_at, \
     started_by, finished_at, note";

fn pg(s: &Store) -> bool {
    matches!(s, Store::Postgres(_))
}

fn tx_pg(t: &Tx) -> bool {
    matches!(t, Tx::Postgres(_))
}

impl Tx {
    /// `blob_backend`, locked for update (SQLite: writers are serialized):
    /// (active, epoch, paused, migration_job).
    async fn lock_backend(&mut self) -> CoreResult<(String, i64, bool, Option<String>)> {
        let q = if tx_pg(self) {
            "SELECT active, epoch, paused, migration_job FROM blob_backend WHERE id = 1 FOR UPDATE"
        } else {
            "SELECT active, epoch, paused, migration_job FROM blob_backend WHERE id = 1"
        };
        tx_run!(self, q, fetch_one,)
    }
}

impl Store {
    /// Starts a migration to `target` (with its disk root). Refused while
    /// another runs, or when `target` is already the active backend.
    pub async fn start_blob_migration(
        &self,
        id: &str,
        target: &str,
        disk_root: Option<&str>,
        by: &str,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let (active, _, _, job) = tx.lock_backend().await?;
        if job.is_some() {
            return Err(CoreError::Conflict(
                "a blob migration is already running".into(),
            ));
        }
        let (root,): (Option<String>,) = tx_run!(
            &mut tx,
            "SELECT disk_root FROM blob_backend WHERE id = 1",
            fetch_one,
        )?;
        if active == target && (target != "disk" || root.as_deref() == disk_root) {
            return Err(CoreError::Invalid(format!("blobs are already on {target}")));
        }
        let q = sql(
            "INSERT INTO blob_migration (id, target, disk_root, phase, started_at, started_by) \
             VALUES ({1}, {2}, {3}, 'copying', {now}, {4})",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, id, target, disk_root, by)?;
        let q = sql(
            "UPDATE blob_backend SET migration_job = {1} WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, id)?;
        tx.commit().await
    }

    /// The migration not yet done or cancelled, if any.
    pub async fn active_blob_migration(&self) -> CoreResult<Option<BlobMigration>> {
        let q = format!(
            "SELECT {COLS} FROM blob_migration WHERE phase IN ('copying', 'post_flip') \
             ORDER BY started_at DESC LIMIT 1"
        );
        run!(self, q, fetch_optional,)
    }

    /// The latest migration, whatever its phase.
    pub async fn latest_blob_migration(&self) -> CoreResult<Option<BlobMigration>> {
        let q = format!("SELECT {COLS} FROM blob_migration ORDER BY started_at DESC LIMIT 1");
        run!(self, q, fetch_optional,)
    }

    /// A pass's mark: the key sequence's current value, read while
    /// `blob_backend` is locked, so every value up to it is committed or
    /// abandoned.
    pub async fn key_seq_mark(&self, lease: &LeaseToken) -> CoreResult<i64> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        tx.lock_backend().await?;
        let q = if tx_pg(&tx) {
            "SELECT last_value FROM blob_key_sequence"
        } else {
            "SELECT value FROM key_seq_counter WHERE id = 1"
        };
        let (mark,): (i64,) = tx_run!(&mut tx, q, fetch_one,)?;
        tx.commit().await?;
        Ok(mark)
    }

    /// Tracked rows with key_seq in (`after`, `through`], in key_seq order,
    /// up to `limit`.
    pub async fn tracked_between(
        &self,
        after: i64,
        through: i64,
        limit: i64,
    ) -> CoreResult<Vec<Tracked>> {
        let q = sql(
            "SELECT storage_key, cid, key_seq FROM ( \
             SELECT storage_key, cid, key_seq FROM blob WHERE key_seq > {1} AND key_seq <= {2} \
             UNION ALL SELECT storage_key, NULL AS cid, key_seq FROM branding_asset WHERE key_seq > {1} AND key_seq <= {2} \
             ) t ORDER BY key_seq LIMIT {3}",
            pg(self),
        );
        run!(self, q, fetch_all, after, through, limit)
    }

    /// Tracked rows with key_seq in (`after`, `through`] that have no copy
    /// record that counts: `copied`, or a `missing_source` (acknowledged,
    /// when `acked_only`).
    pub async fn uncopied_between(
        &self,
        after: i64,
        through: i64,
        acked_only: bool,
        limit: i64,
    ) -> CoreResult<Vec<Tracked>> {
        let q = sql(
            "SELECT t.storage_key, t.cid, t.key_seq FROM ( \
             SELECT storage_key, cid, key_seq FROM blob WHERE key_seq > {1} AND key_seq <= {2} \
             UNION ALL SELECT storage_key, NULL AS cid, key_seq FROM branding_asset WHERE key_seq > {1} AND key_seq <= {2} \
             ) t LEFT JOIN blob_copy c ON c.storage_key = t.storage_key \
             WHERE c.storage_key IS NULL OR (c.outcome = 'missing_source' AND {3} AND NOT c.acknowledged) \
             ORDER BY t.key_seq LIMIT {4}",
            pg(self),
        );
        run!(self, q, fetch_all, after, through, acked_only, limit)
    }

    /// The copy record of a key: (outcome, acknowledged).
    pub async fn copy_record(&self, key: &str) -> CoreResult<Option<(String, bool)>> {
        let q = sql(
            "SELECT outcome, acknowledged FROM blob_copy WHERE storage_key = {1}",
            pg(self),
        );
        run!(self, q, fetch_optional, key)
    }

    /// Records a copy's outcome, fenced on the job's lease.
    pub async fn record_copy(
        &self,
        lease: &LeaseToken,
        key: &str,
        outcome: &str,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        let q = sql(
            "INSERT INTO blob_copy (storage_key, outcome) VALUES ({1}, {2}) \
             ON CONFLICT (storage_key) DO UPDATE SET outcome = excluded.outcome",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, key, outcome)?;
        tx.commit().await
    }

    /// Every `missing_source` key a tracked row still names, with whether
    /// it's acknowledged; entries no row names any more are dropped.
    pub async fn missing_sources(&self) -> CoreResult<Vec<(String, bool)>> {
        let q = "DELETE FROM blob_copy WHERE outcome = 'missing_source' AND storage_key NOT IN \
                 (SELECT storage_key FROM blob UNION SELECT storage_key FROM branding_asset)";
        match self {
            Store::Sqlite(p) => sqlx::query(q).execute(p).await.map(|_| ()),
            Store::Postgres(p) => sqlx::query(q).execute(p).await.map(|_| ()),
        }
        .map_err(db_err)?;
        let q = "SELECT storage_key, acknowledged FROM blob_copy WHERE outcome = 'missing_source' ORDER BY storage_key";
        run!(self, q, fetch_all,)
    }

    /// Acknowledges every listed `missing_source`. Returns how many.
    pub async fn acknowledge_missing_sources(&self) -> CoreResult<u64> {
        let q = sql(
            "UPDATE blob_copy SET acknowledged = {1} WHERE outcome = 'missing_source' AND NOT acknowledged",
            pg(self),
        );
        match self {
            Store::Sqlite(p) => sqlx::query(&q)
                .bind(true)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
            Store::Postgres(p) => sqlx::query(&q)
                .bind(true)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    }

    /// Records the job's progress (fenced).
    pub async fn set_migration_marks(
        &self,
        lease: &LeaseToken,
        id: &str,
        copied_through: i64,
        verified_through: i64,
        note: Option<&str>,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        let q = sql(
            "UPDATE blob_migration SET copied_through = {2}, verified_through = {3}, passes = passes + 1, \
             note = {4} WHERE id = {1}",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, id, copied_through, verified_through, note)?;
        tx.commit().await
    }

    /// The pause: from its commit no transaction can set a key.
    pub async fn pause_for_cutover(&self, lease: &LeaseToken, id: &str) -> CoreResult<bool> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        let (_, _, _, job) = tx.lock_backend().await?;
        if job.as_deref() != Some(id) {
            tx.rollback().await?;
            return Ok(false);
        }
        let q = sql(
            "UPDATE blob_backend SET paused = {1} WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, true)?;
        tx.commit().await?;
        Ok(true)
    }

    /// Ends a pause without flipping (the job keeps the migration).
    pub async fn clear_pause(&self, lease: &LeaseToken) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        tx.lock_backend().await?;
        let q = sql(
            "UPDATE blob_backend SET paused = {1} WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, false)?;
        tx.commit().await
    }

    /// The flip: if the job still owns a paused migration, the target
    /// becomes active with a new epoch, and every node is told to reload.
    /// Returns the new epoch.
    pub async fn flip_blob_backend(
        &self,
        lease: &LeaseToken,
        m: &BlobMigration,
    ) -> CoreResult<Option<i64>> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        let (_, epoch, paused, job) = tx.lock_backend().await?;
        if !paused || job.as_deref() != Some(m.id.as_str()) {
            tx.rollback().await?;
            return Ok(None);
        }
        let q = sql(
            "UPDATE blob_backend SET active = {1}, disk_root = {2}, epoch = epoch + 1, paused = {3}, \
             migration_job = NULL WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, &m.target, &m.disk_root, false)?;
        let q = sql(
            "UPDATE blob_migration SET phase = 'post_flip', note = NULL WHERE id = {1}",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, &m.id)?;
        tx.bump_key_generation().await?;
        tx.commit().await?;
        Ok(Some(epoch + 1))
    }

    /// After the flip: moves begun under an older epoch restart on the new
    /// backend, deletes queued at an older epoch are queued for it too, the
    /// copy records go, and the job is done.
    pub async fn finish_blob_migration(&self, lease: &LeaseToken, id: &str) -> CoreResult<u64> {
        let mut tx = self.begin().await?;
        tx.fence(lease).await?;
        let epoch: (i64,) = tx_run!(
            &mut tx,
            "SELECT epoch FROM blob_backend WHERE id = 1",
            fetch_one,
        )?;
        let moves = tx.restart_pending_moves_before(epoch.0).await?;
        let q = sql(
            "INSERT INTO blob_delete (storage_key, backend_epoch, enqueued_at) \
             SELECT storage_key, {1}, {now} FROM blob_delete WHERE backend_epoch < {1}",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, epoch.0)?;
        tx_exec!(&mut tx, "DELETE FROM blob_copy",)?;
        let q = sql(
            "UPDATE blob_migration SET phase = 'done', finished_at = {now} WHERE id = {1}",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, id)?;
        tx.commit().await?;
        Ok(moves)
    }

    /// `migrate blobs --cancel`: ends the job without flipping; the target's
    /// copies are left for the operator.
    pub async fn cancel_blob_migration(&self) -> CoreResult<Option<String>> {
        let mut tx = self.begin().await?;
        let (_, _, _, job) = tx.lock_backend().await?;
        let Some(id) = job else {
            tx.rollback().await?;
            return Ok(None);
        };
        let q = sql(
            "UPDATE blob_backend SET paused = {1}, migration_job = NULL WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, false)?;
        let q = sql(
            "UPDATE blob_migration SET phase = 'cancelled', finished_at = {now} WHERE id = {1}",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, &id)?;
        // A job fenced on its lease can't commit after this: its lease goes.
        let q = sql("DELETE FROM job_lease WHERE name = {1}", tx_pg(&tx));
        tx_exec!(&mut tx, q, MIGRATE_LEASE)?;
        tx_exec!(&mut tx, "DELETE FROM blob_copy",)?;
        tx.commit().await?;
        Ok(Some(id))
    }
}

impl Tx {
    /// Restarts the pending moves begun under an epoch before `epoch` (see
    /// `restart_pending_moves`).
    pub async fn restart_pending_moves_before(&mut self, epoch: i64) -> CoreResult<u64> {
        let pg = tx_pg(self);
        let q = sql(
            "SELECT did, cid, pending_move FROM blob WHERE pending_move IS NOT NULL \
             AND (move_epoch IS NULL OR move_epoch < {1})",
            pg,
        );
        let rows: Vec<(String, String, String)> = tx_run!(self, q, fetch_all, epoch)?;
        for (did, cid, old) in &rows {
            let place = crate::blob::Place::of_key(old);
            let fresh = place.new_key(did, cid);
            let q = sql(
                "UPDATE blob SET pending_move = {3}, move_epoch = {4} WHERE did = {1} AND cid = {2}",
                pg,
            );
            tx_exec!(self, q, did, cid, &fresh, epoch)?;
            self.queue_blob_delete(old, epoch - 1).await?;
        }
        Ok(rows.len() as u64)
    }
}
