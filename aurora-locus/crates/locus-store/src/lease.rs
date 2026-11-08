//! Leases for singleton work. A node acquires a named lease with a
//! conditional update that increments its epoch, renews it while it holds it,
//! and loses it if it doesn't renew in time. Transactions under a lease call
//! [`crate::Tx::fence`] so a stale holder never commits.

use std::time::Duration;

use locus_core::CoreResult;
use locus_core::lease::LeaseToken;

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, db_err};

fn ttl_ms(ttl: Duration) -> i64 {
    i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX)
}

impl Store {
    /// Tries to acquire `name` for `holder`. Succeeds when the lease is free,
    /// expired, or already held by `holder` (a restarted node reclaiming its
    /// own lease); every success increments the epoch, which fences out any
    /// earlier token.
    pub async fn try_acquire_lease(
        &self,
        name: &str,
        holder: &str,
        ttl: Duration,
    ) -> CoreResult<Option<LeaseToken>> {
        let ttl_ms = ttl_ms(ttl);
        let epoch: Option<i64> = match self {
            Self::Sqlite(p) => {
                let mut t = p.begin_with("BEGIN IMMEDIATE").await.map_err(db_err)?;
                sqlx::query(
                    "INSERT INTO job_lease (name, holder, epoch, expires_at) VALUES (?1, '', 0, 0) \
                     ON CONFLICT (name) DO NOTHING",
                )
                .bind(name)
                .execute(&mut *t)
                .await
                .map_err(db_err)?;
                let epoch = sqlx::query_scalar(&format!(
                    "UPDATE job_lease SET holder = ?2, epoch = epoch + 1, expires_at = {SQLITE_NOW_MS} + ?3 \
                     WHERE name = ?1 AND (expires_at <= {SQLITE_NOW_MS} OR holder = ?2) RETURNING epoch"
                ))
                .bind(name)
                .bind(holder)
                .bind(ttl_ms)
                .fetch_optional(&mut *t)
                .await
                .map_err(db_err)?;
                t.commit().await.map_err(db_err)?;
                epoch
            }
            Self::Postgres(p) => {
                let mut t = p.begin().await.map_err(db_err)?;
                sqlx::query(
                    "INSERT INTO job_lease (name, holder, epoch, expires_at) VALUES ($1, '', 0, 0) \
                     ON CONFLICT (name) DO NOTHING",
                )
                .bind(name)
                .execute(&mut *t)
                .await
                .map_err(db_err)?;
                let epoch = sqlx::query_scalar(&format!(
                    "UPDATE job_lease SET holder = $2, epoch = epoch + 1, expires_at = {PG_NOW_MS} + $3 \
                     WHERE name = $1 AND (expires_at <= {PG_NOW_MS} OR holder = $2) RETURNING epoch"
                ))
                .bind(name)
                .bind(holder)
                .bind(ttl_ms)
                .fetch_optional(&mut *t)
                .await
                .map_err(db_err)?;
                t.commit().await.map_err(db_err)?;
                epoch
            }
        };
        Ok(epoch.map(|epoch| LeaseToken {
            name: name.to_owned(),
            holder: holder.to_owned(),
            epoch,
            ttl,
        }))
    }

    /// Extends a held lease. Returns false if it was lost (taken over, or
    /// already expired): the holder must stop its work.
    pub async fn renew_lease(&self, token: &LeaseToken) -> CoreResult<bool> {
        let ttl_ms = ttl_ms(token.ttl);
        let rows = match self {
            Self::Sqlite(p) => sqlx::query(&format!(
                "UPDATE job_lease SET expires_at = {SQLITE_NOW_MS} + ?4 \
                 WHERE name = ?1 AND holder = ?2 AND epoch = ?3 AND expires_at > {SQLITE_NOW_MS}"
            ))
            .bind(&token.name)
            .bind(&token.holder)
            .bind(token.epoch)
            .bind(ttl_ms)
            .execute(p)
            .await
            .map_err(db_err)?
            .rows_affected(),
            Self::Postgres(p) => sqlx::query(&format!(
                "UPDATE job_lease SET expires_at = {PG_NOW_MS} + $4 \
                 WHERE name = $1 AND holder = $2 AND epoch = $3 AND expires_at > {PG_NOW_MS}"
            ))
            .bind(&token.name)
            .bind(&token.holder)
            .bind(token.epoch)
            .bind(ttl_ms)
            .execute(p)
            .await
            .map_err(db_err)?
            .rows_affected(),
        };
        Ok(rows == 1)
    }

    /// Gives a lease up early so another node can take it at once.
    pub async fn release_lease(&self, token: &LeaseToken) -> CoreResult<()> {
        let sql_sqlite =
            "UPDATE job_lease SET expires_at = 0 WHERE name = ?1 AND holder = ?2 AND epoch = ?3";
        let sql_pg =
            "UPDATE job_lease SET expires_at = 0 WHERE name = $1 AND holder = $2 AND epoch = $3";
        match self {
            Self::Sqlite(p) => sqlx::query(sql_sqlite)
                .bind(&token.name)
                .bind(&token.holder)
                .bind(token.epoch)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err),
            Self::Postgres(p) => sqlx::query(sql_pg)
                .bind(&token.name)
                .bind(&token.holder)
                .bind(token.epoch)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err),
        }
    }

    /// Claims the lowest free slot `0..slots` of a slot family (e.g. TID clock
    /// ids), as the lease `<family>/<n>`. A node that holds a clock-id slot
    /// counts as live.
    pub async fn claim_slot(
        &self,
        family: &str,
        slots: u32,
        holder: &str,
        ttl: Duration,
    ) -> CoreResult<Option<(u32, LeaseToken)>> {
        for n in 0..slots {
            if let Some(token) = self
                .try_acquire_lease(&format!("{family}/{n}"), holder, ttl)
                .await?
            {
                return Ok(Some((n, token)));
            }
        }
        Ok(None)
    }

    /// Node ids that currently hold an unexpired slot of `family`.
    pub async fn live_slot_holders(&self, family: &str) -> CoreResult<Vec<String>> {
        let pattern = format!("{family}/%");
        match self {
            Self::Sqlite(p) => sqlx::query_scalar(&format!(
                "SELECT DISTINCT holder FROM job_lease WHERE name LIKE ?1 AND expires_at > {SQLITE_NOW_MS}"
            ))
            .bind(pattern)
            .fetch_all(p)
            .await,
            Self::Postgres(p) => sqlx::query_scalar(&format!(
                "SELECT DISTINCT holder FROM job_lease WHERE name LIKE $1 AND expires_at > {PG_NOW_MS}"
            ))
            .bind(pattern)
            .fetch_all(p)
            .await,
        }
        .map_err(db_err)
    }
}
