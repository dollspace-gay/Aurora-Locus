//! Account preferences: JSON values by `$type`, in write order. A write
//! replaces every preference of its namespace the writer may see.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, Tx, db_err};

/// Whether a preference name is in a namespace (`app.bsky` covers
/// `app.bsky.actor.defs#adultContentPref`).
#[must_use]
pub fn in_namespace(namespace: &str, name: &str) -> bool {
    name == namespace
        || name
            .strip_prefix(namespace)
            .is_some_and(|rest| rest.starts_with('.'))
}

impl Store {
    /// Every preference of the account in `namespace`, as (name, JSON), in
    /// write order.
    pub async fn preferences(
        &self,
        did: &str,
        namespace: &str,
    ) -> CoreResult<Vec<(String, String)>> {
        let q = sql(
            "SELECT name, value_json FROM account_pref WHERE did = {1} ORDER BY id",
            self.is_postgres(),
        );
        let rows: Vec<(String, String)> = match self {
            Store::Sqlite(p) => sqlx::query_as(&q).bind(did).fetch_all(p).await,
            Store::Postgres(p) => sqlx::query_as(&q).bind(did).fetch_all(p).await,
        }
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .filter(|(name, _)| in_namespace(namespace, name))
            .collect())
    }
}

impl Tx {
    /// Replaces the account's preferences in `namespace` that the writer
    /// may touch (`writable`) with `values`; the others are kept.
    pub async fn replace_preferences(
        &mut self,
        did: &str,
        namespace: &str,
        writable: impl Fn(&str) -> bool,
        values: &[(String, String)],
    ) -> CoreResult<()> {
        let pg = matches!(self, Tx::Postgres(_));
        let list = sql("SELECT id, name FROM account_pref WHERE did = {1}", pg);
        let rows: Vec<(i64, String)> = match self {
            Tx::Sqlite(t) => sqlx::query_as(&list).bind(did).fetch_all(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(&list).bind(did).fetch_all(&mut **t).await,
        }
        .map_err(db_err)?;
        let del = sql("DELETE FROM account_pref WHERE id = {1}", pg);
        for (id, name) in rows {
            if in_namespace(namespace, &name) && writable(&name) {
                match self {
                    Tx::Sqlite(t) => sqlx::query(&del)
                        .bind(id)
                        .execute(&mut **t)
                        .await
                        .map(|_| ()),
                    Tx::Postgres(t) => sqlx::query(&del)
                        .bind(id)
                        .execute(&mut **t)
                        .await
                        .map(|_| ()),
                }
                .map_err(db_err)?;
            }
        }
        let ins = sql(
            "INSERT INTO account_pref (did, namespace, name, value_json) VALUES ({1}, {2}, {3}, {4})",
            pg,
        );
        for (name, json) in values {
            match self {
                Tx::Sqlite(t) => sqlx::query(&ins)
                    .bind(did)
                    .bind(namespace)
                    .bind(name)
                    .bind(json)
                    .execute(&mut **t)
                    .await
                    .map(|_| ()),
                Tx::Postgres(t) => sqlx::query(&ins)
                    .bind(did)
                    .bind(namespace)
                    .bind(name)
                    .bind(json)
                    .execute(&mut **t)
                    .await
                    .map(|_| ()),
            }
            .map_err(db_err)?;
        }
        Ok(())
    }
}

/// A record written after some rev: (collection, rkey, cid, indexed_at ms,
/// DAG-CBOR bytes).
pub type RecordSince = (String, String, String, i64, Vec<u8>);

impl Store {
    /// Up to `limit` of the account's records written after `rev`, oldest
    /// first (read-after-write).
    pub async fn records_since_rev(
        &self,
        did: &str,
        rev: &str,
        limit: i64,
    ) -> CoreResult<Vec<RecordSince>> {
        let q = sql(
            "SELECT r.collection, r.rkey, r.cid, r.indexed_at, b.content FROM record r \
             JOIN repo_block b ON b.did = r.did AND b.cid = r.cid \
             WHERE r.did = {1} AND r.rev > {2} ORDER BY r.rev ASC LIMIT {3}",
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => {
                sqlx::query_as(&q)
                    .bind(did)
                    .bind(rev)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_as(&q)
                    .bind(did)
                    .bind(rev)
                    .bind(limit)
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)
    }

    /// Whether the account has any record at or before `rev` (a rev from
    /// before all of them, as after a migration, means nothing to merge).
    pub async fn has_record_at_or_before(&self, did: &str, rev: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM record WHERE did = {1} AND rev <= {2} LIMIT 1",
            self.is_postgres(),
        );
        let r: Option<i32> = match self {
            Store::Sqlite(p) => {
                sqlx::query_scalar(&q)
                    .bind(did)
                    .bind(rev)
                    .fetch_optional(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_scalar(&q)
                    .bind(did)
                    .bind(rev)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)?;
        Ok(r.is_some())
    }
}
