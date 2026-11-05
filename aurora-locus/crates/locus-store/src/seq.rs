//! The sequenced event log: moving outbox rows into `seq_event`, reading
//! it for the firehose, retention, and purging deleted accounts' history.

use locus_core::CoreResult;

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// The Postgres channel the sequencer notifies after each batch.
pub const SEQ_CHANNEL: &str = "locus_seq";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    pub id: i64,
    pub did: String,
    pub kind: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeqEvent {
    pub seq: i64,
    pub did: String,
    pub kind: String,
    pub payload: Vec<u8>,
    pub sequenced_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeqState {
    pub last_seq: i64,
    pub pruned_through: i64,
}

fn sql(template: &str, pg: bool) -> String {
    let mut out = template.replace("{now}", if pg { PG_NOW_MS } else { SQLITE_NOW_MS });
    for i in (1..=9).rev() {
        out = out.replace(
            &format!("{{{i}}}"),
            &format!("{}{i}", if pg { '$' } else { '?' }),
        );
    }
    out
}

macro_rules! on_tx {
    ($self:expr, $q:expr, |$qq:ident, $ex:ident| $body:expr) => {{
        let pg = matches!($self, Tx::Postgres(_));
        let $qq = sql($q, pg);
        let $qq: &str = &$qq;
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
    }};
}

macro_rules! on_pool {
    ($self:expr, $q:expr, |$qq:ident, $p:ident| $body:expr) => {{
        let pg = matches!($self, Store::Postgres(_));
        let $qq = sql($q, pg);
        let $qq: &str = &$qq;
        match $self {
            Store::Sqlite($p) => $body,
            Store::Postgres($p) => $body,
        }
    }};
}

impl Tx {
    /// Locks the sequencer state (Postgres `FOR UPDATE`; SQLite already
    /// holds the write lock) before the outbox is read.
    pub async fn lock_seq_state(&mut self) -> CoreResult<SeqState> {
        let pg = matches!(self, Tx::Postgres(_));
        let q = if pg {
            "SELECT last_seq, pruned_through FROM seq_state WHERE id = 1 FOR UPDATE"
        } else {
            "SELECT last_seq, pruned_through FROM seq_state WHERE id = 1"
        };
        let (last_seq, pruned_through): (i64, i64) = on_tx!(self, q, |q, ex| sqlx::query_as(q)
            .fetch_one(ex)
            .await
            .map_err(db_err))?;
        Ok(SeqState {
            last_seq,
            pruned_through,
        })
    }

    /// Every visible outbox row, oldest id first, up to `limit`.
    pub async fn outbox_batch(&mut self, limit: i64) -> CoreResult<Vec<OutboxRow>> {
        let rows: Vec<(i64, String, String, Vec<u8>)> = on_tx!(
            self,
            "SELECT id, did, kind, payload FROM outbox ORDER BY id LIMIT {1}",
            |q, ex| sqlx::query_as(q)
                .bind(limit)
                .fetch_all(ex)
                .await
                .map_err(db_err)
        )?;
        Ok(rows
            .into_iter()
            .map(|(id, did, kind, payload)| OutboxRow {
                id,
                did,
                kind,
                payload,
            })
            .collect())
    }

    /// Moves one outbox row to the log as `seq`.
    pub async fn sequence_row(&mut self, row: &OutboxRow, seq: i64) -> CoreResult<()> {
        on_tx!(
            self,
            "INSERT INTO seq_event (seq, did, kind, payload, sequenced_at) VALUES ({1}, {2}, {3}, {4}, {now})",
            |q, ex| sqlx::query(q)
                .bind(seq)
                .bind(&row.did)
                .bind(&row.kind)
                .bind(&row.payload)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )?;
        on_tx!(self, "DELETE FROM outbox WHERE id = {1}", |q, ex| {
            sqlx::query(q)
                .bind(row.id)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        })
    }

    /// Records the highest seq assigned, and on Postgres notifies every
    /// node's firehose (delivered at commit).
    pub async fn set_last_seq(&mut self, seq: i64) -> CoreResult<()> {
        on_tx!(
            self,
            "UPDATE seq_state SET last_seq = {1} WHERE id = 1",
            |q, ex| sqlx::query(q)
                .bind(seq)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )?;
        if let Tx::Postgres(t) = self {
            sqlx::query("SELECT pg_notify($1, $2)")
                .bind(SEQ_CHANNEL)
                .bind(seq.to_string())
                .execute(&mut **t)
                .await
                .map_err(db_err)?;
        }
        Ok(())
    }

    /// Schedules removing a deleted account's events before `before_seq`,
    /// `after_ms` from now.
    pub async fn queue_seq_purge(
        &mut self,
        did: &str,
        before_seq: i64,
        after_ms: i64,
    ) -> CoreResult<()> {
        on_tx!(
            self,
            "INSERT INTO seq_purge (did, before_seq, due_at) VALUES ({1}, {2}, {now} + {3}) \
             ON CONFLICT (did) DO UPDATE SET before_seq = excluded.before_seq, due_at = excluded.due_at",
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(before_seq)
                .bind(after_ms)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }
}

impl Store {
    pub async fn seq_state(&self) -> CoreResult<SeqState> {
        let (last_seq, pruned_through): (i64, i64) = on_pool!(
            self,
            "SELECT last_seq, pruned_through FROM seq_state WHERE id = 1",
            |q, p| sqlx::query_as(q).fetch_one(p).await.map_err(db_err)
        )?;
        Ok(SeqState {
            last_seq,
            pruned_through,
        })
    }

    /// Events after `cursor`, in seq order.
    pub async fn seq_events_after(&self, cursor: i64, limit: i64) -> CoreResult<Vec<SeqEvent>> {
        let rows: Vec<(i64, String, String, Vec<u8>, i64)> = on_pool!(
            self,
            "SELECT seq, did, kind, payload, sequenced_at FROM seq_event WHERE seq > {1} ORDER BY seq LIMIT {2}",
            |q, p| sqlx::query_as(q)
                .bind(cursor)
                .bind(limit)
                .fetch_all(p)
                .await
                .map_err(db_err)
        )?;
        Ok(rows
            .into_iter()
            .map(|(seq, did, kind, payload, sequenced_at)| SeqEvent {
                seq,
                did,
                kind,
                payload,
                sequenced_at,
            })
            .collect())
    }

    /// The oldest retained seq, if any event is retained.
    pub async fn oldest_seq(&self) -> CoreResult<Option<i64>> {
        on_pool!(self, "SELECT MIN(seq) FROM seq_event", |q, p| {
            sqlx::query_scalar(q).fetch_one(p).await.map_err(db_err)
        })
    }

    /// Removes events older than `retention_ms` and records the highest
    /// seq removed. Returns how many went.
    pub async fn prune_seq_events(&self, retention_ms: i64) -> CoreResult<u64> {
        let mut tx = self.begin().await?;
        let state = tx.lock_seq_state().await?;
        let highest: Option<i64> = on_tx!(
            &mut tx,
            "SELECT MAX(seq) FROM seq_event WHERE sequenced_at < {now} - {1}",
            |q, ex| sqlx::query_scalar(q)
                .bind(retention_ms)
                .fetch_one(ex)
                .await
                .map_err(db_err)
        )?;
        let Some(highest) = highest else {
            tx.rollback().await?;
            return Ok(0);
        };
        let n = on_tx!(
            &mut tx,
            "DELETE FROM seq_event WHERE seq <= {1}",
            |q, ex| sqlx::query(q)
                .bind(highest)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
                .map_err(db_err)
        )?;
        on_tx!(
            &mut tx,
            "UPDATE seq_state SET pruned_through = {1} WHERE id = 1",
            |q, ex| sqlx::query(q)
                .bind(highest.max(state.pruned_through))
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )?;
        tx.commit().await?;
        Ok(n)
    }

    /// Runs the account-deletion purges that are due. Returns events removed.
    pub async fn run_seq_purges(&self) -> CoreResult<u64> {
        let due: Vec<(String, i64)> = on_pool!(
            self,
            "SELECT did, before_seq FROM seq_purge WHERE due_at <= {now}",
            |q, p| sqlx::query_as(q).fetch_all(p).await.map_err(db_err)
        )?;
        let mut total = 0;
        for (did, before) in due {
            let mut tx = self.begin().await?;
            total += on_tx!(
                &mut tx,
                "DELETE FROM seq_event WHERE did = {1} AND seq < {2}",
                |q, ex| sqlx::query(q)
                    .bind(&did)
                    .bind(before)
                    .execute(ex)
                    .await
                    .map(|r| r.rows_affected())
                    .map_err(db_err)
            )?;
            on_tx!(
                &mut tx,
                "DELETE FROM seq_purge WHERE did = {1} AND before_seq = {2}",
                |q, ex| sqlx::query(q)
                    .bind(&did)
                    .bind(before)
                    .execute(ex)
                    .await
                    .map(|_| ())
                    .map_err(db_err)
            )?;
            tx.commit().await?;
        }
        Ok(total)
    }
}
