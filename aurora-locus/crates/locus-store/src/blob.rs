//! Blob metadata: the active backend, blob rows, references from records,
//! moves between storage keys, the delete queue and GC.
//!
//! Every stored copy has its own key (`blobs/<did>/<cid>.<generation>`,
//! `tmp/<generation>`, a fresh random generation per write or move), so no
//! step ever overwrites or deletes another copy. Bytes are never deleted
//! directly: the transaction that orphans a key queues it in
//! `blob_delete`, and a job deletes queued keys after a grace period.

use std::collections::HashMap;

use locus_core::{CoreError, CoreResult};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// The active blob backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backend {
    pub active: String,
    pub epoch: i64,
    pub paused: bool,
    pub disk_root: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRow {
    pub did: String,
    pub cid: String,
    pub mime_type: String,
    pub size: i64,
    pub storage_key: String,
    pub state: String,
    pub takedown_ref: Option<String>,
    pub pending_move: Option<String>,
    pub unreferenced_since: Option<i64>,
    pub review_hold: bool,
}

type BlobTuple = (
    String,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    bool,
);

fn row(t: BlobTuple) -> BlobRow {
    BlobRow {
        did: t.0,
        cid: t.1,
        mime_type: t.2,
        size: t.3,
        storage_key: t.4,
        state: t.5,
        takedown_ref: t.6,
        pending_move: t.7,
        unreferenced_since: t.8,
        review_hold: t.9,
    }
}

const BLOB_COLS: &str = "did, cid, mime_type, size, storage_key, state, takedown_ref, pending_move, \
     unreferenced_since, review_hold";

/// A random 128-bit generation, so a key can never repeat.
#[must_use]
pub fn new_generation() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

/// A fresh key for a blob's permanent copy.
#[must_use]
pub fn permanent_key(did: &str, cid: &str) -> String {
    format!("blobs/{did}/{cid}.{}", new_generation())
}

/// A fresh key for an upload in progress.
#[must_use]
pub fn temp_key() -> String {
    format!("tmp/{}", new_generation())
}

/// A blob move a commit or upload left to finish after its transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMove {
    pub did: String,
    pub cid: String,
    pub from: String,
    pub to: String,
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

/// SQL placeholders `?1..` (SQLite) or `$1..` (Postgres) in a template
/// written with `{1}`, `{2}`, … and `{now}`.
pub(crate) fn sql(template: &str, pg: bool) -> String {
    let mut out = template.replace("{now}", if pg { PG_NOW_MS } else { SQLITE_NOW_MS });
    for i in (1..=19).rev() {
        out = out.replace(
            &format!("{{{i}}}"),
            &format!("{}{i}", if pg { '$' } else { '?' }),
        );
    }
    out
}

impl Tx {
    fn is_pg(&self) -> bool {
        matches!(self, Tx::Postgres(_))
    }

    /// The active backend, read before any blob-row lock (Postgres
    /// `FOR SHARE`, so a cutover waits for this transaction).
    pub async fn blob_backend(&mut self) -> CoreResult<Backend> {
        let row: (String, i64, bool, Option<String>) = on_tx!(
            self,
            "SELECT active, epoch, paused, disk_root FROM blob_backend WHERE id = 1",
            "SELECT active, epoch, paused, disk_root FROM blob_backend WHERE id = 1 FOR SHARE",
            |q, ex| sqlx::query_as(q).fetch_one(ex).await.map_err(db_err)
        )?;
        Ok(Backend {
            active: row.0,
            epoch: row.1,
            paused: row.2,
            disk_root: row.3,
        })
    }

    /// The next `key_seq` value, drawn after the backend read.
    pub async fn next_key_seq(&mut self) -> CoreResult<i64> {
        on_tx!(
            self,
            "UPDATE key_seq_counter SET value = value + 1 WHERE id = 1 RETURNING value",
            "SELECT nextval('blob_key_sequence')",
            |q, ex| sqlx::query_scalar(q).fetch_one(ex).await.map_err(db_err)
        )
    }

    /// A blob row, locked for update on Postgres.
    pub async fn blob_for_update(&mut self, did: &str, cid: &str) -> CoreResult<Option<BlobRow>> {
        let pg = self.is_pg();
        let q = sql(
            &format!(
                "SELECT {BLOB_COLS} FROM blob WHERE did = {{1}} AND cid = {{2}}{}",
                if pg { " FOR UPDATE" } else { "" }
            ),
            pg,
        );
        let r: Option<BlobTuple> = on_tx!(self, q, q, |q, ex| sqlx::query_as(q)
            .bind(did)
            .bind(cid)
            .fetch_optional(ex)
            .await
            .map_err(db_err))?;
        Ok(r.map(row))
    }

    /// A new temp row for an upload.
    pub async fn insert_temp_blob(
        &mut self,
        did: &str,
        cid: &str,
        mime: &str,
        size: i64,
        key: &str,
    ) -> CoreResult<()> {
        let seq = self.next_key_seq().await?;
        let q = sql(
            "INSERT INTO blob (did, cid, mime_type, size, storage_key, state, created_at, key_seq) \
             VALUES ({1}, {2}, {3}, {4}, {5}, 'temp', {now}, {6})",
            self.is_pg(),
        );
        on_tx!(self, q, q, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(cid)
            .bind(mime)
            .bind(size)
            .bind(key)
            .bind(seq)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Points a still-temp row at a re-upload's key and restarts its TTL.
    /// Returns the old key, or None if the row isn't temp any more.
    pub async fn repoint_temp_blob(
        &mut self,
        did: &str,
        cid: &str,
        key: &str,
    ) -> CoreResult<Option<String>> {
        let Some(current) = self.blob_for_update(did, cid).await? else {
            return Ok(None);
        };
        if current.state != "temp" {
            return Ok(None);
        }
        let seq = self.next_key_seq().await?;
        let q = sql(
            "UPDATE blob SET storage_key = {3}, created_at = {now}, key_seq = {4} \
             WHERE did = {1} AND cid = {2} AND state = 'temp'",
            self.is_pg(),
        );
        on_tx!(self, q, q, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(cid)
            .bind(key)
            .bind(seq)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))?;
        Ok(Some(current.storage_key))
    }

    /// Restarts an unreferenced blob's grace period (a re-upload).
    pub async fn restart_unreferenced(&mut self, did: &str, cid: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE blob SET unreferenced_since = {now} \
             WHERE did = {1} AND cid = {2} AND unreferenced_since IS NOT NULL",
            self.is_pg(),
        );
        on_tx!(self, q, q, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(cid)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Queues a key's bytes for deletion on the backend of `epoch`.
    pub async fn queue_blob_delete(&mut self, key: &str, epoch: i64) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO blob_delete (storage_key, backend_epoch, enqueued_at) VALUES ({1}, {2}, {now})",
            self.is_pg(),
        );
        on_tx!(self, q, q, |q, ex| sqlx::query(q)
            .bind(key)
            .bind(epoch)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Takes a reference to a blob: refused (None) unless the row exists
    /// and is referenceable (not taken down, quarantined or held). Clears
    /// `unreferenced_since`; a temp blob is promoted (state permanent, a
    /// pending move to `promote_to`). Returns the row as it now is.
    pub async fn reference_blob(
        &mut self,
        did: &str,
        cid: &str,
        promote_to: &str,
        epoch: i64,
    ) -> CoreResult<Option<BlobRow>> {
        let q = sql(
            &format!(
                "UPDATE blob SET unreferenced_since = NULL, \
                 pending_move = CASE WHEN state = 'temp' THEN {{3}} ELSE pending_move END, \
                 move_epoch = CASE WHEN state = 'temp' THEN {{4}} ELSE move_epoch END, \
                 state = 'permanent' \
                 WHERE did = {{1}} AND cid = {{2}} AND takedown_ref IS NULL \
                 AND state IN ('temp', 'permanent') AND NOT review_hold \
                 RETURNING {BLOB_COLS}"
            ),
            self.is_pg(),
        );
        let r: Option<BlobTuple> = on_tx!(self, q, q, |q, ex| sqlx::query_as(q)
            .bind(did)
            .bind(cid)
            .bind(promote_to)
            .bind(epoch)
            .fetch_optional(ex)
            .await
            .map_err(db_err))?;
        Ok(r.map(row))
    }

    /// The blobs each record path references now.
    pub async fn record_blob_cids(
        &mut self,
        did: &str,
        paths: &[(String, String)],
    ) -> CoreResult<HashMap<(String, String), Vec<String>>> {
        let mut out: HashMap<(String, String), Vec<String>> = HashMap::new();
        let q = sql(
            "SELECT blob_cid FROM record_blob WHERE did = {1} AND collection = {2} AND rkey = {3}",
            self.is_pg(),
        );
        for (collection, rkey) in paths {
            let cids: Vec<String> = on_tx!(self, q, q, |q, ex| sqlx::query_scalar(q)
                .bind(did)
                .bind(collection)
                .bind(rkey)
                .fetch_all(ex)
                .await
                .map_err(db_err))?;
            if !cids.is_empty() {
                out.insert((collection.clone(), rkey.clone()), cids);
            }
        }
        Ok(out)
    }

    /// Sets the blobs a record references (none for a deleted record).
    pub async fn replace_record_blobs(
        &mut self,
        did: &str,
        collection: &str,
        rkey: &str,
        cids: &[String],
    ) -> CoreResult<()> {
        let pg = self.is_pg();
        let del = sql(
            "DELETE FROM record_blob WHERE did = {1} AND collection = {2} AND rkey = {3}",
            pg,
        );
        on_tx!(self, del, del, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(collection)
            .bind(rkey)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))?;
        let ins = sql(
            "INSERT INTO record_blob (did, blob_cid, collection, rkey) VALUES ({1}, {2}, {3}, {4}) \
             ON CONFLICT DO NOTHING",
            pg,
        );
        for cid in cids {
            on_tx!(self, ins, ins, |q, ex| sqlx::query(q)
                .bind(did)
                .bind(cid)
                .bind(collection)
                .bind(rkey)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// Sets what a record points at (`(path, target)` pairs).
    pub async fn replace_backlinks(
        &mut self,
        did: &str,
        collection: &str,
        rkey: &str,
        links: &[(String, String)],
    ) -> CoreResult<()> {
        let pg = self.is_pg();
        let del = sql(
            "DELETE FROM backlink WHERE did = {1} AND collection = {2} AND rkey = {3}",
            pg,
        );
        on_tx!(self, del, del, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(collection)
            .bind(rkey)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))?;
        let ins = sql(
            "INSERT INTO backlink (did, collection, rkey, path, link_to) VALUES ({1}, {2}, {3}, {4}, {5}) \
             ON CONFLICT DO NOTHING",
            pg,
        );
        for (path, target) in links {
            on_tx!(self, ins, ins, |q, ex| sqlx::query(q)
                .bind(did)
                .bind(collection)
                .bind(rkey)
                .bind(path)
                .bind(target)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err))?;
        }
        Ok(())
    }

    /// The rkeys of the account's records in `collection` pointing at
    /// `target` through `path`.
    pub async fn backlinked_records(
        &mut self,
        did: &str,
        collection: &str,
        path: &str,
        target: &str,
    ) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT rkey FROM backlink WHERE did = {1} AND collection = {2} AND path = {3} AND link_to = {4} \
             ORDER BY rkey",
            self.is_pg(),
        );
        on_tx!(self, q, q, |q, ex| sqlx::query_scalar(q)
            .bind(did)
            .bind(collection)
            .bind(path)
            .bind(target)
            .fetch_all(ex)
            .await
            .map_err(db_err))
    }

    /// Marks a blob unreferenced if no record references it any more.
    pub async fn orphan_blob_if_unreferenced(&mut self, did: &str, cid: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE blob SET unreferenced_since = {now} WHERE did = {1} AND cid = {2} \
             AND unreferenced_since IS NULL AND state = 'permanent' \
             AND NOT EXISTS (SELECT 1 FROM record_blob r WHERE r.did = {1} AND r.blob_cid = {2})",
            self.is_pg(),
        );
        on_tx!(self, q, q, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(cid)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }

    /// Step 3 of a move: if the row still waits for `to`, it now points at
    /// it and the old key is queued; otherwise `to` itself is queued.
    /// Returns whether the move took effect.
    pub async fn finish_blob_move(
        &mut self,
        did: &str,
        cid: &str,
        to: &str,
        epoch: i64,
    ) -> CoreResult<bool> {
        let current = self.blob_for_update(did, cid).await?;
        match current {
            Some(r) if r.pending_move.as_deref() == Some(to) => {
                let seq = self.next_key_seq().await?;
                let q = sql(
                    "UPDATE blob SET storage_key = {3}, pending_move = NULL, move_epoch = NULL, key_seq = {4} \
                     WHERE did = {1} AND cid = {2}",
                    self.is_pg(),
                );
                on_tx!(self, q, q, |q, ex| sqlx::query(q)
                    .bind(did)
                    .bind(cid)
                    .bind(to)
                    .bind(seq)
                    .execute(ex)
                    .await
                    .map(|_| ())
                    .map_err(db_err))?;
                self.queue_blob_delete(&r.storage_key, epoch).await?;
                Ok(true)
            }
            _ => {
                self.queue_blob_delete(to, epoch).await?;
                Ok(false)
            }
        }
    }

    /// One GC batch: deletes temp blobs never referenced within `temp_ttl_ms`
    /// and permanent ones unreferenced for `grace_ms`, queueing their keys.
    /// Never collects taken-down, quarantined or held blobs, or one with a
    /// move in flight. Returns how many rows went.
    pub async fn gc_blob_batch(
        &mut self,
        temp_ttl_ms: i64,
        grace_ms: i64,
        limit: i64,
        epoch: i64,
    ) -> CoreResult<u64> {
        // A CID an active import's new tree references counts as referenced.
        let not_referenced = format!(
            "NOT EXISTS (SELECT 1 FROM record_blob r WHERE r.did = blob.did AND r.blob_cid = blob.cid) \
             AND NOT EXISTS (SELECT 1 FROM import_blob ib JOIN bulk_job j ON j.id = ib.job_id \
             WHERE j.did = blob.did AND ib.cid = blob.cid AND ib.in_new_tree AND j.kind = 'import' \
             AND j.phase IN {})",
            crate::import::ACTIVE_IMPORT_PHASES
        );
        let collectable = format!(
            "pending_move IS NULL AND takedown_ref IS NULL AND NOT review_hold AND {not_referenced} AND \
             ((state = 'temp' AND created_at < {{now}} - {{1}}) OR \
              (state = 'permanent' AND unreferenced_since IS NOT NULL AND unreferenced_since < {{now}} - {{2}}))"
        );
        let q = sql(
            &format!(
                "DELETE FROM blob WHERE (did, cid) IN (SELECT did, cid FROM blob WHERE {collectable} LIMIT {{3}}) \
                 AND {collectable} RETURNING storage_key"
            ),
            self.is_pg(),
        );
        let keys: Vec<String> = on_tx!(self, q, q, |q, ex| sqlx::query_scalar(q)
            .bind(temp_ttl_ms)
            .bind(grace_ms)
            .bind(limit)
            .fetch_all(ex)
            .await
            .map_err(db_err))?;
        for k in &keys {
            self.queue_blob_delete(k, epoch).await?;
        }
        Ok(keys.len() as u64)
    }

    /// For an account deletion: removes up to `limit` of its blob rows,
    /// queueing their keys (and any pending move's key). Returns how many.
    pub async fn delete_account_blobs(
        &mut self,
        did: &str,
        limit: i64,
        epoch: i64,
    ) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM blob WHERE did = {1} AND cid IN (SELECT cid FROM blob WHERE did = {1} LIMIT {2}) \
             RETURNING storage_key, pending_move",
            self.is_pg(),
        );
        let keys: Vec<(String, Option<String>)> = on_tx!(self, q, q, |q, ex| sqlx::query_as(q)
            .bind(did)
            .bind(limit)
            .fetch_all(ex)
            .await
            .map_err(db_err))?;
        for (k, pending) in &keys {
            self.queue_blob_delete(k, epoch).await?;
            if let Some(p) = pending {
                self.queue_blob_delete(p, epoch).await?;
            }
        }
        Ok(keys.len() as u64)
    }

    pub async fn remove_blob_delete(&mut self, id: i64) -> CoreResult<()> {
        let q = sql("DELETE FROM blob_delete WHERE id = {1}", self.is_pg());
        on_tx!(self, q, q, |q, ex| sqlx::query(q)
            .bind(id)
            .execute(ex)
            .await
            .map(|_| ())
            .map_err(db_err))
    }
}

impl Store {
    fn is_pg_store(&self) -> bool {
        matches!(self, Store::Postgres(_))
    }

    pub async fn blob(&self, did: &str, cid: &str) -> CoreResult<Option<BlobRow>> {
        let q = sql(
            &format!("SELECT {BLOB_COLS} FROM blob WHERE did = {{1}} AND cid = {{2}}"),
            self.is_pg_store(),
        );
        let r: Option<BlobTuple> = on_pool!(self, q, q, |q, p| sqlx::query_as(q)
            .bind(did)
            .bind(cid)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(r.map(row))
    }

    pub async fn blob_backend(&self) -> CoreResult<Backend> {
        let r: (String, i64, bool, Option<String>) = on_pool!(
            self,
            "SELECT active, epoch, paused, disk_root FROM blob_backend WHERE id = 1",
            "SELECT active, epoch, paused, disk_root FROM blob_backend WHERE id = 1",
            |q, p| sqlx::query_as(q).fetch_one(p).await.map_err(db_err)
        )?;
        Ok(Backend {
            active: r.0,
            epoch: r.1,
            paused: r.2,
            disk_root: r.3,
        })
    }

    /// Moves not finished (oldest first), for the mover job.
    pub async fn pending_blob_moves(&self, limit: i64) -> CoreResult<Vec<PendingMove>> {
        let q = sql(
            "SELECT did, cid, storage_key, pending_move FROM blob WHERE pending_move IS NOT NULL LIMIT {1}",
            self.is_pg_store(),
        );
        let rows: Vec<(String, String, String, String)> =
            on_pool!(self, q, q, |q, p| sqlx::query_as(q)
                .bind(limit)
                .fetch_all(p)
                .await
                .map_err(db_err))?;
        Ok(rows
            .into_iter()
            .map(|(did, cid, from, to)| PendingMove { did, cid, from, to })
            .collect())
    }

    /// Queued deletes older than the grace period: (id, key, epoch).
    pub async fn due_blob_deletes(
        &self,
        grace_ms: i64,
        limit: i64,
    ) -> CoreResult<Vec<(i64, String, i64)>> {
        let q = sql(
            "SELECT id, storage_key, backend_epoch FROM blob_delete WHERE enqueued_at < {now} - {1} \
             ORDER BY id LIMIT {2}",
            self.is_pg_store(),
        );
        on_pool!(self, q, q, |q, p| sqlx::query_as(q)
            .bind(grace_ms)
            .bind(limit)
            .fetch_all(p)
            .await
            .map_err(db_err))
    }

    /// Whether any row's current key or pending move names `key`.
    pub async fn blob_key_in_use(&self, key: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM blob WHERE storage_key = {1} OR pending_move = {1} LIMIT 1",
            self.is_pg_store(),
        );
        let hit: Option<i32> = on_pool!(self, q, q, |q, p| sqlx::query_scalar(q)
            .bind(key)
            .fetch_optional(p)
            .await
            .map_err(db_err))?;
        Ok(hit.is_some())
    }

    pub async fn bump_blob_delete(&self, id: i64) -> CoreResult<()> {
        let q = sql(
            "UPDATE blob_delete SET attempts = attempts + 1 WHERE id = {1}",
            self.is_pg_store(),
        );
        on_pool!(self, q, q, |q, p| sqlx::query(q)
            .bind(id)
            .execute(p)
            .await
            .map(|_| ())
            .map_err(db_err))
    }
}

/// A blob row a write refers to doesn't exist or can't be referenced.
#[must_use]
pub fn blob_not_found(cid: &str) -> CoreError {
    CoreError::NotFound(format!("Could not find blob: {cid}"))
}
