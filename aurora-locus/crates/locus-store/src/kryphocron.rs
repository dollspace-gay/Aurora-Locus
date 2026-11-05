//! Kryphocron storage: private-tier permission copied from the AppView,
//! the private audience store, the private-record index, rotation
//! generations and audit events.

use locus_core::CoreResult;

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

macro_rules! scalar {
    ($store:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query_scalar(&$q)$(.bind($arg))*.$method(p).await,
            Store::Postgres(p) => sqlx::query_scalar(&$q)$(.bind($arg))*.$method(p).await,
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

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

// ---------------------------------------------------------------- permission

/// Where a permission row came from.
pub const SOURCE_MEMBERSHIP: &str = "membership";
pub const SOURCE_PROBE: &str = "probe";

/// An answer about an account's private-tier permission, copied from the
/// AppView.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PermissionRow {
    pub did: String,
    pub source: String,
    pub prism_did: String,
    pub status: String,
    pub private_indexed: bool,
    pub private_read: bool,
    pub expires_at: i64,
    pub checked_at: i64,
}

const PERMISSION_COLS: &str =
    "did, source, prism_did, status, private_indexed, private_read, expires_at, checked_at";

impl Store {
    pub async fn kr_permission(
        &self,
        did: &str,
        source: &str,
    ) -> CoreResult<Option<PermissionRow>> {
        let q = sql(
            &format!(
                "SELECT {PERMISSION_COLS} FROM kr_permission WHERE did = {{1}} AND source = {{2}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did, source)
    }

    /// Writes (or replaces) a permission row.
    pub async fn put_kr_permission(&self, r: &PermissionRow) -> CoreResult<()> {
        let q = sql(
            &format!(
                "INSERT INTO kr_permission ({PERMISSION_COLS}) VALUES ({{1}}, {{2}}, {{3}}, {{4}}, {{5}}, {{6}}, {{7}}, {{8}}) \
                 ON CONFLICT (did, source) DO UPDATE SET prism_did = excluded.prism_did, status = excluded.status, \
                 private_indexed = excluded.private_indexed, private_read = excluded.private_read, \
                 expires_at = excluded.expires_at, checked_at = excluded.checked_at"
            ),
            self.is_postgres(),
        );
        exec!(
            self,
            q,
            &r.did,
            &r.source,
            &r.prism_did,
            &r.status,
            r.private_indexed,
            r.private_read,
            r.expires_at,
            r.checked_at
        )?;
        Ok(())
    }

    /// Drops both permission rows of an account.
    pub async fn delete_kr_permissions(&self, did: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM kr_permission WHERE did = {1}",
            self.is_postgres(),
        );
        exec!(self, q, did)?;
        Ok(())
    }

    /// Drops every permission row copied from another AppView.
    pub async fn delete_kr_permissions_not_from(&self, prism_did: &str) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM kr_permission WHERE prism_did <> {1}",
            self.is_postgres(),
        );
        exec!(self, q, prism_did)
    }

    /// The operator's local block on private-tier writes.
    pub async fn private_tier_blocked(&self, did: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT private_tier_blocked FROM account WHERE did = {1}",
            self.is_postgres(),
        );
        let v: Option<bool> = scalar!(self, q, fetch_optional, did)?;
        Ok(v.unwrap_or(false))
    }

    pub async fn set_private_tier_blocked(&self, did: &str, blocked: bool) -> CoreResult<()> {
        let q = sql(
            "UPDATE account SET private_tier_blocked = {2} WHERE did = {1}",
            self.is_postgres(),
        );
        exec!(self, q, did, blocked)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- audiences

/// An audience in the private audience store.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AudienceRow {
    pub did: String,
    pub rkey: String,
    pub name: Option<String>,
    pub mode: String,
    pub members_json: String,
    pub created_at: String,
    pub updated_at: String,
    pub origin: String,
    pub prism_did: String,
    pub prism_cid: Option<String>,
    pub local_cid: String,
    pub value_cbor: Vec<u8>,
    pub pending: String,
    pub row_version: i64,
}

const AUDIENCE_COLS: &str = "did, rkey, name, mode, members_json, created_at, updated_at, origin, \
     prism_did, prism_cid, local_cid, value_cbor, pending, row_version";

impl Store {
    /// The audience at `rkey`, unless it's a pending delete (which counts as
    /// missing).
    pub async fn kr_audience(&self, did: &str, rkey: &str) -> CoreResult<Option<AudienceRow>> {
        let q = sql(
            &format!(
                "SELECT {AUDIENCE_COLS} FROM kr_audience WHERE did = {{1}} AND rkey = {{2}} AND pending <> 'delete'"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did, rkey)
    }

    /// Writes (or replaces) an audience row.
    pub async fn put_kr_audience(&self, r: &AudienceRow) -> CoreResult<()> {
        let q = sql(
            &format!(
                "INSERT INTO kr_audience ({AUDIENCE_COLS}) VALUES ({{1}}, {{2}}, {{3}}, {{4}}, {{5}}, {{6}}, {{7}}, \
                 {{8}}, {{9}}, {{10}}, {{11}}, {{12}}, {{13}}, {{14}}) ON CONFLICT (did, rkey) DO UPDATE SET \
                 name = excluded.name, mode = excluded.mode, members_json = excluded.members_json, \
                 created_at = excluded.created_at, updated_at = excluded.updated_at, origin = excluded.origin, \
                 prism_did = excluded.prism_did, prism_cid = excluded.prism_cid, local_cid = excluded.local_cid, \
                 value_cbor = excluded.value_cbor, pending = excluded.pending, \
                 row_version = kr_audience.row_version + 1"
            ),
            self.is_postgres(),
        );
        exec!(
            self,
            q,
            &r.did,
            &r.rkey,
            &r.name,
            &r.mode,
            &r.members_json,
            &r.created_at,
            &r.updated_at,
            &r.origin,
            &r.prism_did,
            &r.prism_cid,
            &r.local_cid,
            &r.value_cbor,
            &r.pending,
            r.row_version
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------- private records

/// A private record's index entry.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PrivateRecord {
    pub did: String,
    pub collection: String,
    pub rkey: String,
    /// `encodedContentGeneration`, if the record has one.
    pub generation: Option<String>,
    /// The record carries `text` (written before encoding, or imported).
    pub legacy_text: bool,
    pub flagged: Option<String>,
    pub audience_did: Option<String>,
    pub audience_rkey: Option<String>,
}

impl Tx {
    /// Sets (or, with `None`, removes) a record's private index entry. A
    /// rewrite clears any flag.
    pub async fn replace_private_record(
        &mut self,
        did: &str,
        collection: &str,
        rkey: &str,
        entry: Option<&PrivateRecord>,
    ) -> CoreResult<()> {
        let pg = matches!(self, Tx::Postgres(_));
        let Some(e) = entry else {
            let q = sql(
                "DELETE FROM kr_private_record WHERE did = {1} AND collection = {2} AND rkey = {3}",
                pg,
            );
            tx_exec!(self, q, did, collection, rkey)?;
            return Ok(());
        };
        let q = sql(
            "INSERT INTO kr_private_record (did, collection, rkey, generation, legacy_text, flagged, audience_did, audience_rkey) \
             VALUES ({1}, {2}, {3}, {4}, {5}, NULL, {6}, {7}) ON CONFLICT (did, collection, rkey) DO UPDATE SET \
             generation = excluded.generation, legacy_text = excluded.legacy_text, flagged = NULL, \
             audience_did = excluded.audience_did, audience_rkey = excluded.audience_rkey",
            pg,
        );
        tx_exec!(
            self,
            q,
            did,
            collection,
            rkey,
            &e.generation,
            e.legacy_text,
            &e.audience_did,
            &e.audience_rkey
        )?;
        Ok(())
    }
}

const PRIVATE_COLS: &str =
    "did, collection, rkey, generation, legacy_text, flagged, audience_did, audience_rkey";

impl Store {
    pub async fn private_record(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
    ) -> CoreResult<Option<PrivateRecord>> {
        let q = sql(
            &format!(
                "SELECT {PRIVATE_COLS} FROM kr_private_record WHERE did = {{1}} AND collection = {{2}} AND rkey = {{3}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did, collection, rkey)
    }

    /// Accounts with private records due for rotation (a generation older
    /// than `current`, none, or legacy text), not flagged, after `after`.
    pub async fn rotation_due_accounts(
        &self,
        current: &str,
        after: &str,
        limit: i64,
    ) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT DISTINCT did FROM kr_private_record WHERE flagged IS NULL AND did > {2} \
             AND (generation IS NULL OR generation < {1} OR legacy_text = {4}) ORDER BY did LIMIT {3}",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_all, current, after, limit, true)
    }

    /// One account's private records due for rotation, oldest key first.
    pub async fn rotation_due_records(
        &self,
        did: &str,
        current: &str,
        limit: i64,
    ) -> CoreResult<Vec<PrivateRecord>> {
        let q = sql(
            &format!(
                "SELECT {PRIVATE_COLS} FROM kr_private_record WHERE did = {{1}} AND flagged IS NULL \
                 AND (generation IS NULL OR generation < {{2}} OR legacy_text = {{4}}) \
                 ORDER BY collection, rkey LIMIT {{3}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did, current, limit, true)
    }

    /// Records with a generation dated past `future_limit` (a mark the
    /// encoder dated in the future), due for rotation whatever the current
    /// generation is.
    pub async fn rotation_future_dated(
        &self,
        did: &str,
        future_limit: &str,
        limit: i64,
    ) -> CoreResult<Vec<PrivateRecord>> {
        let q = sql(
            &format!(
                "SELECT {PRIVATE_COLS} FROM kr_private_record WHERE did = {{1}} AND flagged IS NULL \
                 AND generation > {{2}} ORDER BY collection, rkey LIMIT {{3}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did, future_limit, limit)
    }

    /// Flags a record rotation can't rewrite (it's skipped until changed).
    pub async fn flag_private_record(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
        why: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE kr_private_record SET flagged = {4} WHERE did = {1} AND collection = {2} AND rkey = {3}",
            self.is_postgres(),
        );
        exec!(self, q, did, collection, rkey, why)?;
        Ok(())
    }

    /// Flagged private records, for the console.
    pub async fn flagged_private_records(&self, limit: i64) -> CoreResult<Vec<PrivateRecord>> {
        let q = sql(
            &format!(
                "SELECT {PRIVATE_COLS} FROM kr_private_record WHERE flagged IS NOT NULL \
                 ORDER BY did, collection, rkey LIMIT {{1}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, limit)
    }
}

// ---------------------------------------------------------------- rotation

impl Store {
    /// The current (newest) rotation generation and when it was made.
    pub async fn current_generation(&self) -> CoreResult<Option<(String, i64)>> {
        let q = String::from(
            "SELECT generation, created_at FROM kr_rotation_state ORDER BY generation DESC LIMIT 1",
        );
        run!(self, q, fetch_optional,)
    }

    /// Adds a generation (no-op if it exists).
    pub async fn add_generation(&self, generation: &str, created_at: i64) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_rotation_state (generation, created_at) VALUES ({1}, {2}) \
             ON CONFLICT (generation) DO NOTHING",
            self.is_postgres(),
        );
        exec!(self, q, generation, created_at)?;
        Ok(())
    }
}

impl Tx {
    /// Adds a generation inside a (fenced) transaction.
    pub async fn add_generation(&mut self, generation: &str, created_at: i64) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_rotation_state (generation, created_at) VALUES ({1}, {2}) \
             ON CONFLICT (generation) DO NOTHING",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(self, q, generation, created_at)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- audit

/// One kryphocron audit event.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditRow {
    pub at: i64,
    pub sink: String,
    pub event: String,
    pub correlation_id: Option<String>,
    pub detail_json: String,
}

impl Store {
    /// Appends audit events.
    pub async fn append_kr_audit(&self, rows: &[AuditRow]) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_audit (at, sink, event, correlation_id, detail_json) VALUES ({1}, {2}, {3}, {4}, {5})",
            self.is_postgres(),
        );
        for r in rows {
            exec!(
                self,
                q,
                r.at,
                &r.sink,
                &r.event,
                &r.correlation_id,
                &r.detail_json
            )?;
        }
        Ok(())
    }

    /// The newest audit events, newest first.
    pub async fn kr_audit(&self, limit: i64) -> CoreResult<Vec<AuditRow>> {
        let q = sql(
            "SELECT at, sink, event, correlation_id, detail_json FROM kr_audit ORDER BY id DESC LIMIT {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, limit)
    }

    /// Drops audit events older than `before` (ms). Returns how many went.
    pub async fn prune_kr_audit(&self, before: i64, limit: i64) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM kr_audit WHERE id IN (SELECT id FROM kr_audit WHERE at < {1} LIMIT {2})",
            self.is_postgres(),
        );
        exec!(self, q, before, limit)
    }
}
