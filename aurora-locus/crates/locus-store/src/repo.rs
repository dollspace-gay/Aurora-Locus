//! Repository storage: per-account roots, reference-counted blocks, the
//! record index and the event outbox. Everything here runs inside a [`Tx`]
//! so a commit's blocks, index changes and event land atomically.
//!
//! CIDs are stored as base32 strings.

use std::collections::HashMap;

use locus_core::{CoreError, CoreResult};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// Advisory-lock class for per-repo serialization on Postgres.
const REPO_LOCK_CLASS: i32 = 0x5245_504f; // "REPO"

/// A repository's current root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRoot {
    pub commit_cid: String,
    pub rev: String,
    pub data_cid: String,
}

/// One block reference-count change.
#[derive(Debug, Clone)]
pub struct BlockDelta {
    pub cid: String,
    /// Positive deltas carry the bytes, so a row GC just removed is re-created.
    pub bytes: Option<Vec<u8>>,
    pub delta: i64,
}

/// One record-index change.
#[derive(Debug, Clone)]
pub enum RecordChange {
    Put {
        collection: String,
        rkey: String,
        cid: String,
    },
    Delete {
        collection: String,
        rkey: String,
    },
}

impl Tx {
    /// Serializes writers of one repository for the rest of this transaction.
    /// Postgres: a transaction-scoped advisory lock (works before the
    /// account's first commit too). SQLite: the transaction already holds the
    /// database write lock.
    pub async fn lock_repo(&mut self, did: &str) -> CoreResult<()> {
        if let Tx::Postgres(t) = self {
            sqlx::query("SELECT pg_advisory_xact_lock($1, hashtext($2))")
                .bind(REPO_LOCK_CLASS)
                .bind(did)
                .execute(&mut **t)
                .await
                .map_err(db_err)?;
        }
        Ok(())
    }

    pub async fn repo_root(&mut self, did: &str) -> CoreResult<Option<RepoRoot>> {
        let row: Option<(String, String, String)> = match self {
            Tx::Sqlite(t) => {
                sqlx::query_as("SELECT commit_cid, rev, data_cid FROM repo_root WHERE did = ?1")
                    .bind(did)
                    .fetch_optional(&mut **t)
                    .await
            }
            Tx::Postgres(t) => {
                sqlx::query_as(
                    "SELECT commit_cid, rev, data_cid FROM repo_root WHERE did = $1 FOR UPDATE",
                )
                .bind(did)
                .fetch_optional(&mut **t)
                .await
            }
        }
        .map_err(db_err)?;
        Ok(row.map(|(commit_cid, rev, data_cid)| RepoRoot {
            commit_cid,
            rev,
            data_cid,
        }))
    }

    pub async fn set_repo_root(&mut self, did: &str, root: &RepoRoot) -> CoreResult<()> {
        match self {
            Tx::Sqlite(t) => sqlx::query(&format!(
                "INSERT INTO repo_root (did, commit_cid, rev, data_cid, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, {SQLITE_NOW_MS}) ON CONFLICT (did) DO UPDATE SET \
                 commit_cid = excluded.commit_cid, rev = excluded.rev, data_cid = excluded.data_cid, \
                 updated_at = excluded.updated_at"
            ))
            .bind(did)
            .bind(&root.commit_cid)
            .bind(&root.rev)
            .bind(&root.data_cid)
            .execute(&mut **t)
            .await
            .map(|_| ()),
            Tx::Postgres(t) => sqlx::query(&format!(
                "INSERT INTO repo_root (did, commit_cid, rev, data_cid, updated_at) \
                 VALUES ($1, $2, $3, $4, {PG_NOW_MS}) ON CONFLICT (did) DO UPDATE SET \
                 commit_cid = excluded.commit_cid, rev = excluded.rev, data_cid = excluded.data_cid, \
                 updated_at = excluded.updated_at"
            ))
            .bind(did)
            .bind(&root.commit_cid)
            .bind(&root.rev)
            .bind(&root.data_cid)
            .execute(&mut **t)
            .await
            .map(|_| ()),
        }
        .map_err(db_err)
    }

    /// The bytes of a live block (ref_count > 0) of this repository.
    pub async fn repo_block(&mut self, did: &str, cid: &str) -> CoreResult<Option<Vec<u8>>> {
        match self {
            Tx::Sqlite(t) => {
                sqlx::query_scalar(
                    "SELECT content FROM repo_block WHERE did = ?1 AND cid = ?2 AND ref_count > 0",
                )
                .bind(did)
                .bind(cid)
                .fetch_optional(&mut **t)
                .await
            }
            Tx::Postgres(t) => {
                sqlx::query_scalar(
                    "SELECT content FROM repo_block WHERE did = $1 AND cid = $2 AND ref_count > 0",
                )
                .bind(did)
                .bind(cid)
                .fetch_optional(&mut **t)
                .await
            }
        }
        .map_err(db_err)
    }

    /// Applies reference-count changes. A block whose count goes from 0 (or
    /// absent) to ≥1 gets `rev_introduced = rev` and `zero_since` cleared; a
    /// block reaching 0 gets `zero_since` = now. A negative delta on a block
    /// that doesn't exist, or that would go below 0, is a corrupt count and
    /// fails the transaction.
    pub async fn apply_block_deltas(
        &mut self,
        did: &str,
        rev: &str,
        deltas: &[BlockDelta],
    ) -> CoreResult<()> {
        for d in deltas {
            if d.delta == 0 {
                continue;
            }
            if d.delta > 0 {
                let bytes = d.bytes.as_deref().ok_or_else(|| {
                    CoreError::Storage(format!(
                        "block {} gains a reference but has no bytes",
                        d.cid
                    ))
                })?;
                let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                match self {
                    Tx::Sqlite(t) => sqlx::query(
                        "INSERT INTO repo_block (did, cid, rev_introduced, size, content, ref_count, zero_since) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL) ON CONFLICT (did, cid) DO UPDATE SET \
                         rev_introduced = CASE WHEN repo_block.ref_count = 0 THEN excluded.rev_introduced \
                             ELSE repo_block.rev_introduced END, \
                         ref_count = repo_block.ref_count + excluded.ref_count, zero_since = NULL",
                    )
                    .bind(did)
                    .bind(&d.cid)
                    .bind(rev)
                    .bind(size)
                    .bind(bytes)
                    .bind(d.delta)
                    .execute(&mut **t)
                    .await
                    .map(|_| ()),
                    Tx::Postgres(t) => sqlx::query(
                        "INSERT INTO repo_block (did, cid, rev_introduced, size, content, ref_count, zero_since) \
                         VALUES ($1, $2, $3, $4, $5, $6, NULL) ON CONFLICT (did, cid) DO UPDATE SET \
                         rev_introduced = CASE WHEN repo_block.ref_count = 0 THEN excluded.rev_introduced \
                             ELSE repo_block.rev_introduced END, \
                         ref_count = repo_block.ref_count + excluded.ref_count, zero_since = NULL",
                    )
                    .bind(did)
                    .bind(&d.cid)
                    .bind(rev)
                    .bind(size)
                    .bind(bytes)
                    .bind(d.delta)
                    .execute(&mut **t)
                    .await
                    .map(|_| ()),
                }
                .map_err(db_err)?;
            } else {
                let dec = -d.delta;
                let rows = match self {
                    Tx::Sqlite(t) => sqlx::query(&format!(
                        "UPDATE repo_block SET ref_count = ref_count - ?3, \
                         zero_since = CASE WHEN ref_count - ?3 = 0 THEN {SQLITE_NOW_MS} ELSE zero_since END \
                         WHERE did = ?1 AND cid = ?2 AND ref_count >= ?3"
                    ))
                    .bind(did)
                    .bind(&d.cid)
                    .bind(dec)
                    .execute(&mut **t)
                    .await
                    .map(|r| r.rows_affected()),
                    Tx::Postgres(t) => sqlx::query(&format!(
                        "UPDATE repo_block SET ref_count = ref_count - $3, \
                         zero_since = CASE WHEN ref_count - $3 = 0 THEN {PG_NOW_MS} ELSE zero_since END \
                         WHERE did = $1 AND cid = $2 AND ref_count >= $3"
                    ))
                    .bind(did)
                    .bind(&d.cid)
                    .bind(dec)
                    .execute(&mut **t)
                    .await
                    .map(|r| r.rows_affected()),
                }
                .map_err(db_err)?;
                if rows != 1 {
                    return Err(CoreError::Storage(format!(
                        "reference count of block {} in {did} would go negative",
                        d.cid
                    )));
                }
            }
        }
        Ok(())
    }

    /// Current CIDs of the given record paths (`collection/rkey`).
    pub async fn record_cids(
        &mut self,
        did: &str,
        paths: &[(String, String)],
    ) -> CoreResult<HashMap<(String, String), String>> {
        let mut out = HashMap::new();
        for (collection, rkey) in paths {
            let cid: Option<String> =
                match self {
                    Tx::Sqlite(t) => sqlx::query_scalar(
                        "SELECT cid FROM record WHERE did = ?1 AND collection = ?2 AND rkey = ?3",
                    )
                    .bind(did)
                    .bind(collection)
                    .bind(rkey)
                    .fetch_optional(&mut **t)
                    .await,
                    Tx::Postgres(t) => sqlx::query_scalar(
                        "SELECT cid FROM record WHERE did = $1 AND collection = $2 AND rkey = $3",
                    )
                    .bind(did)
                    .bind(collection)
                    .bind(rkey)
                    .fetch_optional(&mut **t)
                    .await,
                }
                .map_err(db_err)?;
            if let Some(cid) = cid {
                out.insert((collection.clone(), rkey.clone()), cid);
            }
        }
        Ok(out)
    }

    pub async fn apply_record_changes(
        &mut self,
        did: &str,
        rev: &str,
        changes: &[RecordChange],
    ) -> CoreResult<()> {
        for c in changes {
            match c {
                RecordChange::Put { collection, rkey, cid } => match self {
                    Tx::Sqlite(t) => sqlx::query(&format!(
                        "INSERT INTO record (did, collection, rkey, cid, rev, indexed_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, {SQLITE_NOW_MS}) ON CONFLICT (did, collection, rkey) \
                         DO UPDATE SET cid = excluded.cid, rev = excluded.rev, indexed_at = excluded.indexed_at"
                    ))
                    .bind(did)
                    .bind(collection)
                    .bind(rkey)
                    .bind(cid)
                    .bind(rev)
                    .execute(&mut **t)
                    .await
                    .map(|_| ()),
                    Tx::Postgres(t) => sqlx::query(&format!(
                        "INSERT INTO record (did, collection, rkey, cid, rev, indexed_at) \
                         VALUES ($1, $2, $3, $4, $5, {PG_NOW_MS}) ON CONFLICT (did, collection, rkey) \
                         DO UPDATE SET cid = excluded.cid, rev = excluded.rev, indexed_at = excluded.indexed_at"
                    ))
                    .bind(did)
                    .bind(collection)
                    .bind(rkey)
                    .bind(cid)
                    .bind(rev)
                    .execute(&mut **t)
                    .await
                    .map(|_| ()),
                },
                RecordChange::Delete { collection, rkey } => match self {
                    Tx::Sqlite(t) => sqlx::query("DELETE FROM record WHERE did = ?1 AND collection = ?2 AND rkey = ?3")
                        .bind(did)
                        .bind(collection)
                        .bind(rkey)
                        .execute(&mut **t)
                        .await
                        .map(|_| ()),
                    Tx::Postgres(t) => sqlx::query("DELETE FROM record WHERE did = $1 AND collection = $2 AND rkey = $3")
                        .bind(did)
                        .bind(collection)
                        .bind(rkey)
                        .execute(&mut **t)
                        .await
                        .map(|_| ()),
                },
            }
            .map_err(db_err)?;
        }
        Ok(())
    }

    /// Appends an event to the outbox. Returns its id.
    pub async fn insert_outbox(
        &mut self,
        did: &str,
        kind: &str,
        payload: &[u8],
    ) -> CoreResult<i64> {
        match self {
            Tx::Sqlite(t) => sqlx::query_scalar(&format!(
                "INSERT INTO outbox (did, kind, payload, created_at) VALUES (?1, ?2, ?3, {SQLITE_NOW_MS}) RETURNING id"
            ))
            .bind(did)
            .bind(kind)
            .bind(payload)
            .fetch_one(&mut **t)
            .await,
            Tx::Postgres(t) => sqlx::query_scalar(&format!(
                "INSERT INTO outbox (did, kind, payload, created_at) VALUES ($1, $2, $3, {PG_NOW_MS}) RETURNING id"
            ))
            .bind(did)
            .bind(kind)
            .bind(payload)
            .fetch_one(&mut **t)
            .await,
        }
        .map_err(db_err)
    }
}

impl Store {
    /// Deletes blocks whose count has been 0 for longer than `grace_ms`, up
    /// to `limit` of them. The condition is re-checked by the delete itself,
    /// so a block re-referenced in the meantime survives. Returns how many
    /// were deleted.
    pub async fn gc_zero_blocks(&self, grace_ms: i64, limit: i64) -> CoreResult<u64> {
        match self {
            Self::Sqlite(p) => sqlx::query(&format!(
                "DELETE FROM repo_block WHERE rowid IN (SELECT rowid FROM repo_block \
                 WHERE ref_count = 0 AND zero_since < {SQLITE_NOW_MS} - ?1 LIMIT ?2) \
                 AND ref_count = 0"
            ))
            .bind(grace_ms)
            .bind(limit)
            .execute(p)
            .await
            .map(|r| r.rows_affected()),
            Self::Postgres(p) => sqlx::query(&format!(
                "DELETE FROM repo_block WHERE ctid IN (SELECT ctid FROM repo_block \
                 WHERE ref_count = 0 AND zero_since < {PG_NOW_MS} - $1 LIMIT $2) \
                 AND ref_count = 0 AND zero_since < {PG_NOW_MS} - $1"
            ))
            .bind(grace_ms)
            .bind(limit)
            .execute(p)
            .await
            .map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    }

    /// Every live block of a repository with its count, for checks and tests.
    pub async fn repo_block_counts(&self, did: &str) -> CoreResult<HashMap<String, i64>> {
        let rows: Vec<(String, i64)> = match self {
            Self::Sqlite(p) => {
                sqlx::query_as(
                    "SELECT cid, ref_count FROM repo_block WHERE did = ?1 AND ref_count > 0",
                )
                .bind(did)
                .fetch_all(p)
                .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(
                    "SELECT cid, ref_count FROM repo_block WHERE did = $1 AND ref_count > 0",
                )
                .bind(did)
                .fetch_all(p)
                .await
            }
        }
        .map_err(db_err)?;
        Ok(rows.into_iter().collect())
    }

    /// A repository's root, read without a transaction.
    pub async fn repo_root_of(&self, did: &str) -> CoreResult<Option<RepoRoot>> {
        let row: Option<(String, String, String)> = match self {
            Self::Sqlite(p) => {
                sqlx::query_as("SELECT commit_cid, rev, data_cid FROM repo_root WHERE did = ?1")
                    .bind(did)
                    .fetch_optional(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as("SELECT commit_cid, rev, data_cid FROM repo_root WHERE did = $1")
                    .bind(did)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)?;
        Ok(row.map(|(commit_cid, rev, data_cid)| RepoRoot {
            commit_cid,
            rev,
            data_cid,
        }))
    }

    /// A page of a repository's live blocks (by CID, after `after`), only
    /// those introduced after rev `since` when given: what `getRepo` sends.
    pub async fn repo_blocks_page(
        &self,
        did: &str,
        after: Option<&str>,
        since: Option<&str>,
        limit: i64,
    ) -> CoreResult<Vec<(String, Vec<u8>)>> {
        let q = |p: [&str; 4]| {
            format!(
                "SELECT cid, content FROM repo_block WHERE did = {} AND ref_count > 0 \
                 AND ({} IS NULL OR cid > {}) AND ({} IS NULL OR rev_introduced > {}) \
                 ORDER BY cid LIMIT {}",
                p[0], p[1], p[1], p[2], p[2], p[3]
            )
        };
        match self {
            Self::Sqlite(p) => {
                sqlx::query_as(&q(["?1", "?2", "?3", "?4"]))
                    .bind(did)
                    .bind(after)
                    .bind(since)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(&q(["$1", "$2::text", "$3::text", "$4"]))
                    .bind(did)
                    .bind(after)
                    .bind(since)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// Repositories in creation order after `cursor` (created_at, did):
    /// (did, created_at, commit_cid, rev, status).
    pub async fn list_repos(
        &self,
        limit: i64,
        cursor: Option<(i64, &str)>,
    ) -> CoreResult<Vec<(String, i64, String, String, String)>> {
        let (at, did) = cursor.map_or((None, None), |(a, d)| (Some(a), Some(d)));
        let q = |p: [&str; 3]| {
            format!(
                "SELECT a.did, a.created_at, r.commit_cid, r.rev, a.status FROM account a \
                 JOIN repo_root r ON r.did = a.did WHERE a.status <> 'deleted' \
                 AND ({0} IS NULL OR a.created_at > {0} OR (a.created_at = {0} AND a.did > {1})) \
                 ORDER BY a.created_at, a.did LIMIT {2}",
                p[0], p[1], p[2]
            )
        };
        match self {
            Self::Sqlite(p) => {
                sqlx::query_as(&q(["?1", "?2", "?3"]))
                    .bind(at)
                    .bind(did)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(&q(["$1::bigint", "$2::text", "$3"]))
                    .bind(at)
                    .bind(did)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// The blobs a repository's records reference, by CID after `cursor`,
    /// only from records written after rev `since` when given.
    pub async fn list_blob_cids(
        &self,
        did: &str,
        since: Option<&str>,
        cursor: Option<&str>,
        limit: i64,
    ) -> CoreResult<Vec<String>> {
        let q = |p: [&str; 4]| {
            format!(
                "SELECT DISTINCT rb.blob_cid FROM record_blob rb \
                 JOIN record r ON r.did = rb.did AND r.collection = rb.collection AND r.rkey = rb.rkey \
                 WHERE rb.did = {} AND ({} IS NULL OR r.rev > {}) AND ({} IS NULL OR rb.blob_cid > {}) \
                 ORDER BY rb.blob_cid LIMIT {}",
                p[0], p[1], p[1], p[2], p[2], p[3]
            )
        };
        match self {
            Self::Sqlite(p) => {
                sqlx::query_scalar(&q(["?1", "?2", "?3", "?4"]))
                    .bind(did)
                    .bind(since)
                    .bind(cursor)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_scalar(&q(["$1", "$2::text", "$3::text", "$4"]))
                    .bind(did)
                    .bind(since)
                    .bind(cursor)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// One record's CID and takedown, if the record exists.
    pub async fn record_entry(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
    ) -> CoreResult<Option<(String, Option<String>)>> {
        let q = |p: [&str; 3]| {
            format!(
                "SELECT cid, takedown_ref FROM record WHERE did = {} AND collection = {} AND rkey = {}",
                p[0], p[1], p[2]
            )
        };
        match self {
            Self::Sqlite(p) => {
                sqlx::query_as(&q(["?1", "?2", "?3"]))
                    .bind(did)
                    .bind(collection)
                    .bind(rkey)
                    .fetch_optional(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(&q(["$1", "$2", "$3"]))
                    .bind(did)
                    .bind(collection)
                    .bind(rkey)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// A page of a collection's records that aren't taken down, by rkey:
    /// newest first unless `reverse`, after `cursor` (an rkey). Returns
    /// (rkey, cid) pairs.
    pub async fn list_records(
        &self,
        did: &str,
        collection: &str,
        limit: i64,
        cursor: Option<&str>,
        reverse: bool,
    ) -> CoreResult<Vec<(String, String)>> {
        let (cmp, order) = if reverse { (">", "ASC") } else { ("<", "DESC") };
        let q = |p: [&str; 4]| {
            format!(
                "SELECT rkey, cid FROM record WHERE did = {} AND collection = {} AND takedown_ref IS NULL \
                 AND ({} IS NULL OR rkey {cmp} {}) ORDER BY rkey {order} LIMIT {}",
                p[0], p[1], p[2], p[2], p[3]
            )
        };
        match self {
            Self::Sqlite(p) => {
                sqlx::query_as(&q(["?1", "?2", "?3", "?4"]))
                    .bind(did)
                    .bind(collection)
                    .bind(cursor)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(&q(["$1", "$2", "$3::text", "$4"]))
                    .bind(did)
                    .bind(collection)
                    .bind(cursor)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// A stored block's bytes.
    pub async fn block(&self, did: &str, cid: &str) -> CoreResult<Option<Vec<u8>>> {
        match self {
            Self::Sqlite(p) => {
                sqlx::query_scalar("SELECT content FROM repo_block WHERE did = ?1 AND cid = ?2")
                    .bind(did)
                    .bind(cid)
                    .fetch_optional(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_scalar("SELECT content FROM repo_block WHERE did = $1 AND cid = $2")
                    .bind(did)
                    .bind(cid)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// The collections a repository has records in.
    pub async fn collections(&self, did: &str) -> CoreResult<Vec<String>> {
        match self {
            Self::Sqlite(p) => {
                sqlx::query_scalar(
                    "SELECT DISTINCT collection FROM record WHERE did = ?1 ORDER BY collection",
                )
                .bind(did)
                .fetch_all(p)
                .await
            }
            Self::Postgres(p) => {
                sqlx::query_scalar(
                    "SELECT DISTINCT collection FROM record WHERE did = $1 ORDER BY collection",
                )
                .bind(did)
                .fetch_all(p)
                .await
            }
        }
        .map_err(db_err)
    }

    /// A repository's commit, rev, live block count and record count
    /// (`checkAccountStatus`), if it has a repository.
    pub async fn repo_stats(&self, did: &str) -> CoreResult<Option<(String, String, i64, i64)>> {
        let sql = |p: &str| {
            format!(
                "SELECT r.commit_cid, r.rev, \
                 (SELECT COUNT(*) FROM repo_block b WHERE b.did = r.did AND b.ref_count > 0), \
                 (SELECT COUNT(*) FROM record c WHERE c.did = r.did) \
                 FROM repo_root r WHERE r.did = {p}"
            )
        };
        match self {
            Self::Sqlite(p) => sqlx::query_as(&sql("?1")).bind(did).fetch_optional(p).await,
            Self::Postgres(p) => sqlx::query_as(&sql("$1")).bind(did).fetch_optional(p).await,
        }
        .map_err(db_err)
    }

    /// Every outbox payload for a repository, oldest first, for tests.
    pub async fn outbox_for(&self, did: &str) -> CoreResult<Vec<(String, Vec<u8>)>> {
        match self {
            Self::Sqlite(p) => {
                sqlx::query_as("SELECT kind, payload FROM outbox WHERE did = ?1 ORDER BY id")
                    .bind(did)
                    .fetch_all(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as("SELECT kind, payload FROM outbox WHERE did = $1 ORDER BY id")
                    .bind(did)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }
}

impl Store {
    /// Up to `limit` of an account's record index rows after (collection,
    /// rkey), in order: (collection, rkey, cid).
    pub async fn record_index_page(
        &self,
        did: &str,
        after: Option<(&str, &str)>,
        limit: i64,
    ) -> CoreResult<Vec<(String, String, String)>> {
        let (c, r) = after.unwrap_or(("", ""));
        let q = crate::blob::sql(
            "SELECT collection, rkey, cid FROM record WHERE did = {1} \
             AND (collection > {2} OR (collection = {2} AND rkey > {3})) \
             ORDER BY collection, rkey LIMIT {4}",
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => {
                sqlx::query_as(&q)
                    .bind(did)
                    .bind(c)
                    .bind(r)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_as(&q)
                    .bind(did)
                    .bind(c)
                    .bind(r)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }
}

impl Tx {
    /// Sets every block of an account to no references (a rebuild then
    /// counts the new tree's; the rest are left for GC).
    pub async fn zero_block_counts(&mut self, did: &str) -> CoreResult<u64> {
        let q = crate::blob::sql(
            "UPDATE repo_block SET ref_count = 0, zero_since = {now} WHERE did = {1} AND ref_count > 0",
            matches!(self, Tx::Postgres(_)),
        );
        match self {
            Tx::Sqlite(t) => sqlx::query(&q)
                .bind(did)
                .execute(&mut **t)
                .await
                .map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&q)
                .bind(did)
                .execute(&mut **t)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    }
}
