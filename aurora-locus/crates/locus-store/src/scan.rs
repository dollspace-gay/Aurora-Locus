//! Blob scanning state and the review queue of matched blobs.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, Tx, db_err};

/// A matched blob waiting for (or past) an operator's review.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ReviewRow {
    pub id: i64,
    pub did: String,
    pub cid: String,
    pub created_at: i64,
    /// `upload` or `rescan`.
    pub source: String,
    /// What the scanner said (`reject`, `quarantine`).
    pub result: String,
    pub reason: Option<String>,
    /// `open`, `cleared` or `reported`.
    pub status: String,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
    pub note: Option<String>,
}

/// A blob still to be scanned.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Unscanned {
    pub did: String,
    pub cid: String,
    pub mime_type: String,
    pub storage_key: String,
}

const REVIEW_COLS: &str =
    "id, did, cid, created_at, source, result, reason, status, resolved_at, resolved_by, note";

impl Tx {
    /// Records what the scanner said about a blob.
    pub async fn set_scan_status(&mut self, did: &str, cid: &str, status: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE blob SET scan_status = {3} WHERE did = {1} AND cid = {2}",
            matches!(self, Tx::Postgres(_)),
        );
        match self {
            Tx::Sqlite(t) => sqlx::query(&q)
                .bind(did)
                .bind(cid)
                .bind(status)
                .execute(&mut **t)
                .await
                .map(|_| ()),
            Tx::Postgres(t) => sqlx::query(&q)
                .bind(did)
                .bind(cid)
                .bind(status)
                .execute(&mut **t)
                .await
                .map(|_| ()),
        }
        .map_err(db_err)
    }

    /// A match: the blob is quarantined and held (never collected) until an
    /// operator resolves its review.
    pub async fn quarantine_match(&mut self, did: &str, cid: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE blob SET state = 'quarantined', review_hold = {3}, scan_status = 'matched' \
             WHERE did = {1} AND cid = {2}",
            matches!(self, Tx::Postgres(_)),
        );
        match self {
            Tx::Sqlite(t) => sqlx::query(&q)
                .bind(did)
                .bind(cid)
                .bind(true)
                .execute(&mut **t)
                .await
                .map(|_| ()),
            Tx::Postgres(t) => sqlx::query(&q)
                .bind(did)
                .bind(cid)
                .bind(true)
                .execute(&mut **t)
                .await
                .map(|_| ()),
        }
        .map_err(db_err)
    }
}

impl Store {
    pub async fn set_scan_status(&self, did: &str, cid: &str, status: &str) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.set_scan_status(did, cid, status).await?;
        tx.commit().await
    }

    pub async fn quarantine_match(&self, did: &str, cid: &str) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.quarantine_match(did, cid).await?;
        tx.commit().await
    }

    /// Blobs stored without a scan (the scanner was down), oldest first.
    pub async fn unscanned_blobs(&self, limit: i64) -> CoreResult<Vec<Unscanned>> {
        let q = sql(
            "SELECT did, cid, mime_type, storage_key FROM blob WHERE scan_status = 'unscanned' \
             AND state IN ('temp', 'permanent') ORDER BY created_at LIMIT {1}",
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => sqlx::query_as(&q).bind(limit).fetch_all(p).await,
            Store::Postgres(p) => sqlx::query_as(&q).bind(limit).fetch_all(p).await,
        }
        .map_err(db_err)
    }

    /// Adds a matched blob to the review queue. Returns its id.
    pub async fn add_review(
        &self,
        did: &str,
        cid: &str,
        source: &str,
        result: &str,
        reason: Option<&str>,
    ) -> CoreResult<i64> {
        let q = sql(
            "INSERT INTO blob_review (did, cid, created_at, source, result, reason) \
             VALUES ({1}, {2}, {now}, {3}, {4}, {5}) RETURNING id",
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => {
                sqlx::query_scalar(&q)
                    .bind(did)
                    .bind(cid)
                    .bind(source)
                    .bind(result)
                    .bind(reason)
                    .fetch_one(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_scalar(&q)
                    .bind(did)
                    .bind(cid)
                    .bind(source)
                    .bind(result)
                    .bind(reason)
                    .fetch_one(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// Reviews by status, oldest first.
    pub async fn reviews(&self, status: &str, limit: i64) -> CoreResult<Vec<ReviewRow>> {
        let q = sql(
            &format!(
                "SELECT {REVIEW_COLS} FROM blob_review WHERE status = {{1}} ORDER BY created_at LIMIT {{2}}"
            ),
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => {
                sqlx::query_as(&q)
                    .bind(status)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_as(&q)
                    .bind(status)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// Resolves a review (`cleared` or `reported`).
    pub async fn resolve_review(
        &self,
        id: i64,
        status: &str,
        by: &str,
        note: Option<&str>,
    ) -> CoreResult<bool> {
        let q = sql(
            "UPDATE blob_review SET status = {2}, resolved_at = {now}, resolved_by = {3}, note = {4} \
             WHERE id = {1} AND status = 'open'",
            self.is_postgres(),
        );
        let n = match self {
            Store::Sqlite(p) => sqlx::query(&q)
                .bind(id)
                .bind(status)
                .bind(by)
                .bind(note)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
            Store::Postgres(p) => sqlx::query(&q)
                .bind(id)
                .bind(status)
                .bind(by)
                .bind(note)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)?;
        Ok(n == 1)
    }
}

impl Store {
    /// A blob's scan status (`none`, `allowed`, `unscanned`, `matched`).
    pub async fn scan_status(&self, did: &str, cid: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT scan_status FROM blob WHERE did = {1} AND cid = {2}",
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => {
                sqlx::query_scalar(&q)
                    .bind(did)
                    .bind(cid)
                    .fetch_optional(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_scalar(&q)
                    .bind(did)
                    .bind(cid)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)
    }
}
