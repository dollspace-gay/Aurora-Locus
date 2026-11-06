//! Hostname migrations: the prepared candidate, the running migration and
//! its per-account moves, and every old host (reserved forever).

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

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct HostnameMigration {
    pub id: String,
    pub old_host: String,
    pub new_host: String,
    pub state: String,
    pub created_at: i64,
    pub created_by: String,
    pub prechecks: Option<String>,
    pub confirmed_at: Option<i64>,
    pub cursor: Option<String>,
    pub job_done_at: Option<i64>,
    pub window_ends_at: Option<i64>,
    pub rechecked_at: Option<i64>,
    pub closed_at: Option<i64>,
    pub former_at: Option<i64>,
}

/// One account the job couldn't move (yet).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Straggler {
    pub did: String,
    pub state: String,
    pub reason: Option<String>,
}

const COLS: &str = "id, old_host, new_host, state, created_at, created_by, prechecks, confirmed_at, \
     cursor, job_done_at, window_ends_at, rechecked_at, closed_at, former_at";

fn pg(s: &Store) -> bool {
    matches!(s, Store::Postgres(_))
}

impl Store {
    /// Every migration that isn't cancelled, oldest first.
    pub async fn hostname_migrations(&self) -> CoreResult<Vec<HostnameMigration>> {
        let q = format!(
            "SELECT {COLS} FROM hostname_migration WHERE state <> 'cancelled' ORDER BY created_at"
        );
        run!(self, q, fetch_all,)
    }

    /// The prepared or running migration, if any (at most one).
    pub async fn open_hostname_migration(&self) -> CoreResult<Option<HostnameMigration>> {
        let q = format!(
            "SELECT {COLS} FROM hostname_migration WHERE state IN ('prepared', 'running') LIMIT 1"
        );
        run!(self, q, fetch_optional,)
    }

    /// Whether `host` is or was a PDS hostname here: a prepared candidate
    /// or any migration's old host (the current one is the caller's).
    pub async fn pds_host_reserved(&self, host: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM hostname_migration WHERE (old_host = {1} AND state <> 'cancelled') \
             OR (new_host = {1} AND state <> 'cancelled') LIMIT 1",
            pg(self),
        );
        let r: Option<(i32,)> = run!(self, q, fetch_optional, host)?;
        Ok(r.is_some())
    }

    /// Records a prepared candidate. Refused while one is prepared or
    /// running.
    pub async fn prepare_hostname_migration(
        &self,
        id: &str,
        old_host: &str,
        new_host: &str,
        by: &str,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = "SELECT id FROM hostname_migration WHERE state IN ('prepared', 'running')";
        let open: Option<(String,)> = match &mut tx {
            Tx::Sqlite(t) => sqlx::query_as(q).fetch_optional(&mut **t).await,
            Tx::Postgres(t) => {
                sqlx::query_as(&format!("{q} FOR UPDATE"))
                    .fetch_optional(&mut **t)
                    .await
            }
        }
        .map_err(db_err)?;
        if open.is_some() {
            return Err(CoreError::Conflict(
                "a hostname migration is already prepared or running".into(),
            ));
        }
        let q = sql(
            "INSERT INTO hostname_migration (id, old_host, new_host, state, created_at, created_by) \
             VALUES ({1}, {2}, {3}, 'prepared', {now}, {4})",
            pg(self),
        );
        tx_exec!(&mut tx, q, id, old_host, new_host, by)?;
        tx.bump_key_generation().await?;
        tx.commit().await
    }

    pub async fn set_hostname_prechecks(&self, id: &str, json: &str) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE hostname_migration SET prechecks = {2} WHERE id = {1}",
            pg(self),
        );
        tx_exec!(&mut tx, q, id, json)?;
        tx.commit().await
    }

    /// Starts a prepared migration: the new hostname becomes current, every
    /// node reloads. `window_ms` is the transition window.
    pub async fn confirm_hostname_migration(&self, id: &str, window_ms: i64) -> CoreResult<bool> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE hostname_migration SET state = 'running', confirmed_at = {now}, \
             window_ends_at = {now} + {2} WHERE id = {1} AND state = 'prepared'",
            pg(self),
        );
        if tx_exec!(&mut tx, q, id, window_ms)? != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        let q = sql(
            "UPDATE setup_state SET hostname = (SELECT new_host FROM hostname_migration WHERE id = {1})",
            pg(self),
        );
        tx_exec!(&mut tx, q, id)?;
        tx.bump_key_generation().await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Clears a prepared candidate.
    pub async fn cancel_hostname_migration(&self) -> CoreResult<Option<String>> {
        let mut tx = self.begin().await?;
        let q = "SELECT id FROM hostname_migration WHERE state = 'prepared'";
        let row: Option<(String,)> = match &mut tx {
            Tx::Sqlite(t) => sqlx::query_as(q).fetch_optional(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(q).fetch_optional(&mut **t).await,
        }
        .map_err(db_err)?;
        let Some((id,)) = row else {
            tx.rollback().await?;
            return Ok(None);
        };
        let q = sql(
            "UPDATE hostname_migration SET state = 'cancelled', closed_at = {now} WHERE id = {1}",
            pg(self),
        );
        tx_exec!(&mut tx, q, &id)?;
        tx.bump_key_generation().await?;
        tx.commit().await?;
        Ok(Some(id))
    }

    /// Records the job's cursor (None: every account is done).
    pub async fn set_hostname_cursor(&self, id: &str, cursor: Option<&str>) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE hostname_migration SET cursor = {2}, \
             job_done_at = CASE WHEN {3} THEN {now} ELSE job_done_at END WHERE id = {1}",
            pg(self),
        );
        tx_exec!(&mut tx, q, id, cursor.unwrap_or(""), cursor.is_none())?;
        tx.commit().await
    }

    /// What the job did for an account.
    pub async fn record_hostname_move(
        &self,
        id: &str,
        did: &str,
        state: &str,
        reason: Option<&str>,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = sql(
            "INSERT INTO hostname_move (migration_id, did, state, reason, checked_at) \
             VALUES ({1}, {2}, {3}, {4}, {now}) ON CONFLICT (migration_id, did) DO UPDATE \
             SET state = excluded.state, reason = excluded.reason, checked_at = excluded.checked_at",
            pg(self),
        );
        tx_exec!(&mut tx, q, id, did, state, reason)?;
        tx.commit().await
    }

    /// Accounts whose documents may still name the migration's old host.
    pub async fn hostname_stragglers(&self, id: &str, limit: i64) -> CoreResult<Vec<Straggler>> {
        let q = sql(
            "SELECT did, state, reason FROM hostname_move WHERE migration_id = {1} AND state <> 'moved' \
             ORDER BY did LIMIT {2}",
            pg(self),
        );
        run!(self, q, fetch_all, id, limit)
    }

    pub async fn hostname_straggler_count(&self, id: &str) -> CoreResult<i64> {
        let q = sql(
            "SELECT COUNT(*) FROM hostname_move WHERE migration_id = {1} AND state <> 'moved'",
            pg(self),
        );
        let (n,): (i64,) = run!(self, q, fetch_one, id)?;
        Ok(n)
    }

    pub async fn set_hostname_rechecked(&self, id: &str) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE hostname_migration SET rechecked_at = {now} WHERE id = {1}",
            pg(self),
        );
        tx_exec!(&mut tx, q, id)?;
        tx.commit().await
    }

    /// Ends a running migration: `closed` when an operator closes it with
    /// stragglers left, otherwise because none is left (the old host then
    /// becomes a former PDS hostname). Nodes reload.
    pub async fn finish_hostname_migration(&self, id: &str, former: bool) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE hostname_migration SET state = 'done', \
             closed_at = COALESCE(closed_at, {now}), \
             former_at = CASE WHEN {2} THEN {now} ELSE former_at END WHERE id = {1}",
            pg(self),
        );
        tx_exec!(&mut tx, q, id, former)?;
        tx.bump_key_generation().await?;
        tx.commit().await
    }
}
