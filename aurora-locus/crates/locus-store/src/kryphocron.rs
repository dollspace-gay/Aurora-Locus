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
    /// than `current` or dated past `future`, none, or legacy text), not
    /// flagged, after `after`.
    pub async fn rotation_due_accounts(
        &self,
        current: &str,
        future: &str,
        after: &str,
        limit: i64,
    ) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT DISTINCT did FROM kr_private_record WHERE flagged IS NULL AND did > {2} \
             AND (generation IS NULL OR generation < {1} OR generation > {5} OR legacy_text = {4}) \
             ORDER BY did LIMIT {3}",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_all, current, after, limit, true, future)
    }

    /// One account's private records due for rotation, oldest key first:
    /// the urgent ones (legacy text, no generation) and, unless
    /// `urgent_only`, those under a generation older than `current`.
    pub async fn rotation_due_records(
        &self,
        did: &str,
        current: &str,
        urgent_only: bool,
        limit: i64,
    ) -> CoreResult<Vec<PrivateRecord>> {
        let q = sql(
            &format!(
                "SELECT {PRIVATE_COLS} FROM kr_private_record WHERE did = {{1}} AND flagged IS NULL \
                 AND (generation IS NULL OR legacy_text = {{4}} OR ({{5}} AND generation < {{2}})) \
                 ORDER BY collection, rkey LIMIT {{3}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did, current, limit, true, !urgent_only)
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

// ---------------------------------------------------------------- the audience copy

/// What a refresh remembers of a row before its first page.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AudienceVersion {
    pub rkey: String,
    pub row_version: i64,
    pub origin: String,
    pub pending: String,
    pub prism_cid: Option<String>,
    pub prism_did: String,
    pub updated_at: String,
}

/// An account's refresh state.
#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct AudienceSync {
    pub did: String,
    pub last_listing_started_at: Option<i64>,
    pub last_complete_at: Option<i64>,
    pub next_poll_at: i64,
    pub last_owner_refresh_at: i64,
    pub last_third_party_refresh_at: i64,
    pub last_error: Option<String>,
}

/// Which refresh limiter a refresh counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshLimiter {
    /// The owner's own actions: at most one per interval, retryable when
    /// throttled.
    Owner,
    /// Other accounts' checks: at most one per interval per owner.
    ThirdParty,
}

impl Store {
    /// The database clock, in ms.
    pub async fn db_now(&self) -> CoreResult<i64> {
        let q = sql("SELECT {now}", self.is_postgres());
        scalar!(self, q, fetch_one,)
    }

    /// Every audience row of an account (pending deletes included), as a
    /// refresh snapshots them.
    pub async fn audience_versions(&self, did: &str) -> CoreResult<Vec<AudienceVersion>> {
        let q = sql(
            "SELECT rkey, row_version, origin, pending, prism_cid, prism_did, updated_at \
             FROM kr_audience WHERE did = {1} ORDER BY rkey",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    /// Every audience row of an account, newest key first (pending deletes
    /// left out).
    pub async fn kr_audiences(&self, did: &str) -> CoreResult<Vec<AudienceRow>> {
        let q = sql(
            &format!(
                "SELECT {AUDIENCE_COLS} FROM kr_audience WHERE did = {{1}} AND pending <> 'delete' ORDER BY rkey DESC"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    /// Adopts an audience the copy didn't hold. False if a row appeared
    /// meanwhile.
    pub async fn adopt_audience(&self, r: &AudienceRow) -> CoreResult<bool> {
        let q = sql(
            &format!(
                "INSERT INTO kr_audience ({AUDIENCE_COLS}, adopted_at, synced_from_appview_at) VALUES ({{1}}, {{2}}, {{3}}, \
                 {{4}}, {{5}}, {{6}}, {{7}}, {{8}}, {{9}}, {{10}}, {{11}}, {{12}}, {{13}}, {{14}}, {{now}}, {{now}}) \
                 ON CONFLICT (did, rkey) DO NOTHING"
            ),
            self.is_postgres(),
        );
        let n = exec!(
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
        Ok(n == 1)
    }

    /// Replaces a row's value with the AppView's, only if it hasn't changed
    /// since `version` (a concurrent local write wins). Clears any pending
    /// edit.
    pub async fn replace_audience_if(&self, r: &AudienceRow, version: i64) -> CoreResult<bool> {
        let q = sql(
            "UPDATE kr_audience SET name = {3}, mode = {4}, members_json = {5}, created_at = {6}, updated_at = {7}, \
             prism_cid = {8}, local_cid = {9}, value_cbor = {10}, pending = 'none', pending_at = NULL, \
             prism_did = {11}, origin = 'prism', row_version = row_version + 1, synced_from_appview_at = {now} \
             WHERE did = {1} AND rkey = {2} AND row_version = {12}",
            self.is_postgres(),
        );
        let n = exec!(
            self,
            q,
            &r.did,
            &r.rkey,
            &r.name,
            &r.mode,
            &r.members_json,
            &r.created_at,
            &r.updated_at,
            &r.prism_cid,
            &r.local_cid,
            &r.value_cbor,
            &r.prism_did,
            version
        )?;
        Ok(n == 1)
    }

    /// Deletes a row (only at `version`, if given) and writes its tombstone,
    /// in one transaction. False if the row changed or was gone.
    pub async fn delete_audience(
        &self,
        did: &str,
        rkey: &str,
        version: Option<i64>,
    ) -> CoreResult<bool> {
        let mut tx = self.begin().await?;
        let pg = matches!(tx, Tx::Postgres(_));
        let q = sql(
            "DELETE FROM kr_audience WHERE did = {1} AND rkey = {2} AND ({3} OR row_version = {4})",
            pg,
        );
        let n = tx_exec!(
            &mut tx,
            q,
            did,
            rkey,
            version.is_none(),
            version.unwrap_or(0)
        )?;
        if n == 1 {
            let t = sql(
                "INSERT INTO kr_audience_tombstone (did, rkey, deleted_at) VALUES ({1}, {2}, {now}) \
                 ON CONFLICT (did, rkey) DO UPDATE SET deleted_at = excluded.deleted_at",
                pg,
            );
            tx_exec!(&mut tx, t, did, rkey)?;
        }
        tx.commit().await?;
        Ok(n == 1)
    }

    /// When a tombstone names this audience, if one does.
    pub async fn audience_tombstone(&self, did: &str, rkey: &str) -> CoreResult<Option<i64>> {
        let q = sql(
            "SELECT deleted_at FROM kr_audience_tombstone WHERE did = {1} AND rkey = {2}",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_optional, did, rkey)
    }

    /// Drops tombstones older than `before` (database ms).
    pub async fn prune_audience_tombstones(&self, before: i64) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM kr_audience_tombstone WHERE deleted_at < {1}",
            self.is_postgres(),
        );
        exec!(self, q, before)
    }

    /// The AppView rkey a local audience was mapped to.
    pub async fn audience_alias(&self, did: &str, local_rkey: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT prism_rkey FROM kr_audience_alias WHERE did = {1} AND local_rkey = {2}",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_optional, did, local_rkey)
    }

    /// Every alias of an account: (local rkey, prism rkey).
    pub async fn audience_aliases(&self, did: &str) -> CoreResult<Vec<(String, String)>> {
        let q = sql(
            "SELECT local_rkey, prism_rkey FROM kr_audience_alias WHERE did = {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    pub async fn flag_audience_alias(
        &self,
        did: &str,
        local_rkey: &str,
        why: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE kr_audience_alias SET flagged = {3} WHERE did = {1} AND local_rkey = {2}",
            self.is_postgres(),
        );
        exec!(self, q, did, local_rkey, why)?;
        Ok(())
    }

    pub async fn flag_audience(&self, did: &str, rkey: &str, why: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE kr_audience SET flagged = {3} WHERE did = {1} AND rkey = {2}",
            self.is_postgres(),
        );
        exec!(self, q, did, rkey, why)?;
        Ok(())
    }

    /// Discards an account's copy: audiences (local ones included), aliases
    /// and open attempts.
    pub async fn purge_audience_copy(&self, did: &str) -> CoreResult<()> {
        for table in ["kr_audience", "kr_audience_alias", "kr_audience_attempt"] {
            let q = sql(
                &format!("DELETE FROM {table} WHERE did = {{1}}"),
                self.is_postgres(),
            );
            exec!(self, q, did)?;
        }
        Ok(())
    }

    /// An account's refresh state (defaults when it has none yet).
    pub async fn audience_sync(&self, did: &str) -> CoreResult<AudienceSync> {
        let q = sql(
            "SELECT did, last_listing_started_at, last_complete_at, next_poll_at, last_owner_refresh_at, \
             last_third_party_refresh_at, last_error FROM kr_audience_sync WHERE did = {1}",
            self.is_postgres(),
        );
        let row: Option<AudienceSync> = run!(self, q, fetch_optional, did)?;
        Ok(row.unwrap_or_else(|| AudienceSync {
            did: did.to_owned(),
            ..AudienceSync::default()
        }))
    }

    /// Takes a refresh from an account's limiter: true (and the limiter
    /// stamped now) if the last one was at least `interval_ms` ago.
    pub async fn take_refresh(
        &self,
        did: &str,
        limiter: RefreshLimiter,
        interval_ms: i64,
    ) -> CoreResult<bool> {
        let col = match limiter {
            RefreshLimiter::Owner => "last_owner_refresh_at",
            RefreshLimiter::ThirdParty => "last_third_party_refresh_at",
        };
        let q = sql(
            &format!(
                "INSERT INTO kr_audience_sync (did, {col}) VALUES ({{1}}, {{now}}) \
                 ON CONFLICT (did) DO UPDATE SET {col} = excluded.{col} \
                 WHERE kr_audience_sync.{col} <= {{now}} - {{2}} RETURNING 1"
            ),
            self.is_postgres(),
        );
        let r: Option<i32> = scalar!(self, q, fetch_optional, did, interval_ms)?;
        Ok(r.is_some())
    }

    /// Records a listing's start, completion or failure.
    pub async fn note_listing(
        &self,
        did: &str,
        started_at: i64,
        complete: bool,
        error: Option<&str>,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_audience_sync (did, last_listing_started_at, last_complete_at, last_error) \
             VALUES ({1}, {2}, CASE WHEN {3} THEN {now} END, {4}) ON CONFLICT (did) DO UPDATE SET \
             last_listing_started_at = excluded.last_listing_started_at, \
             last_complete_at = COALESCE(excluded.last_complete_at, kr_audience_sync.last_complete_at), \
             last_error = excluded.last_error",
            self.is_postgres(),
        );
        exec!(self, q, did, started_at, complete, error)?;
        Ok(())
    }

    /// When the background poll should next look at an account.
    pub async fn set_next_poll(&self, did: &str, at: i64) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_audience_sync (did, next_poll_at) VALUES ({1}, {2}) \
             ON CONFLICT (did) DO UPDATE SET next_poll_at = excluded.next_poll_at",
            self.is_postgres(),
        );
        exec!(self, q, did, at)?;
        Ok(())
    }

    /// Accounts due a background poll, soonest first.
    pub async fn audience_polls_due(&self, now: i64, limit: i64) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT did FROM kr_audience_sync WHERE next_poll_at <= {1} ORDER BY next_poll_at LIMIT {2}",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_all, now, limit)
    }
}

impl Store {
    /// After an outage: accounts with local audiences or pending edits are
    /// polled first.
    pub async fn poll_queued_audiences_now(&self) -> CoreResult<u64> {
        let q = sql(
            "INSERT INTO kr_audience_sync (did, next_poll_at) SELECT DISTINCT did, 0 FROM kr_audience \
             WHERE origin = 'local' OR pending <> 'none' \
             ON CONFLICT (did) DO UPDATE SET next_poll_at = 0",
            self.is_postgres(),
        );
        exec!(self, q,)
    }
}

impl Store {
    /// Maps a local audience rkey to the AppView's (no-op if mapped).
    pub async fn put_audience_alias(
        &self,
        did: &str,
        local_rkey: &str,
        prism_rkey: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_audience_alias (did, local_rkey, prism_rkey, mapped_at) VALUES ({1}, {2}, {3}, {now}) \
             ON CONFLICT (did, local_rkey) DO NOTHING",
            self.is_postgres(),
        );
        exec!(self, q, did, local_rkey, prism_rkey)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- audience writes

/// An audience create sent to the AppView.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AudienceAttempt {
    pub id: String,
    pub did: String,
    pub value_json: String,
    pub created_at_value: String,
    pub updated_at_value: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub outcome: String,
    pub local_rkey: Option<String>,
    pub local_row_version: Option<i64>,
}

/// A row a refresh adopted and no attempt has claimed yet.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AdoptedRow {
    pub rkey: String,
    pub name: Option<String>,
    pub mode: String,
    pub members_json: String,
    pub created_at: String,
    pub updated_at: String,
    pub adopted_at: i64,
}

impl Store {
    /// Writes an attempt row (`sending`) before the create is sent; its
    /// `created_at` is the database clock. Returns it.
    pub async fn create_audience_attempt(
        &self,
        id: &str,
        did: &str,
        value_json: &str,
        created_at_value: &str,
        updated_at_value: &str,
        local: Option<(&str, i64)>,
    ) -> CoreResult<i64> {
        let q = sql(
            "INSERT INTO kr_audience_attempt (id, did, value_json, created_at_value, updated_at_value, \
             created_at, expires_at, outcome, local_rkey, local_row_version) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {now}, {now} + 3600000, 'sending', {6}, {7}) RETURNING created_at",
            self.is_postgres(),
        );
        scalar!(
            self,
            q,
            fetch_one,
            id,
            did,
            value_json,
            created_at_value,
            updated_at_value,
            local.map(|(r, _)| r),
            local.map(|(_, v)| v)
        )
    }

    pub async fn set_attempt_outcome(&self, id: &str, outcome: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE kr_audience_attempt SET outcome = {2} WHERE id = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id, outcome)?;
        Ok(())
    }

    pub async fn delete_attempt(&self, id: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM kr_audience_attempt WHERE id = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id)?;
        Ok(())
    }

    pub async fn audience_attempt(&self, id: &str) -> CoreResult<Option<AudienceAttempt>> {
        let q = sql(
            "SELECT id, did, value_json, created_at_value, updated_at_value, created_at, expires_at, outcome, \
             local_rkey, local_row_version FROM kr_audience_attempt WHERE id = {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, id)
    }

    /// Open attempts (`sending`, `unknown`) of an account; ones past their
    /// hour become `expired` first.
    pub async fn open_attempts(&self, did: &str) -> CoreResult<Vec<AudienceAttempt>> {
        let expire = sql(
            "UPDATE kr_audience_attempt SET outcome = 'expired' WHERE did = {1} \
             AND outcome IN ('sending', 'unknown') AND expires_at <= {now}",
            self.is_postgres(),
        );
        exec!(self, expire, did)?;
        let q = sql(
            "SELECT id, did, value_json, created_at_value, updated_at_value, created_at, expires_at, outcome, \
             local_rkey, local_row_version FROM kr_audience_attempt WHERE did = {1} \
             AND outcome IN ('sending', 'unknown') ORDER BY created_at",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    /// Rows adopted at or after `since` that no attempt has claimed.
    pub async fn unlinked_adopted(&self, did: &str, since: i64) -> CoreResult<Vec<AdoptedRow>> {
        let q = sql(
            "SELECT rkey, name, mode, members_json, created_at, updated_at, adopted_at FROM kr_audience \
             WHERE did = {1} AND adopted_at IS NOT NULL AND adopted_at >= {2} AND adopted_by_attempt IS NULL",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did, since)
    }

    /// Links an adopted row to the attempt that created it, and marks the
    /// attempt adopted.
    pub async fn link_attempt(&self, did: &str, rkey: &str, attempt: &str) -> CoreResult<bool> {
        let q = sql(
            "UPDATE kr_audience SET adopted_by_attempt = {3} WHERE did = {1} AND rkey = {2} \
             AND adopted_by_attempt IS NULL",
            self.is_postgres(),
        );
        let n = exec!(self, q, did, rkey, attempt)?;
        if n == 1 {
            self.set_attempt_outcome(attempt, "adopted").await?;
        }
        Ok(n == 1)
    }

    /// The row an attempt was linked to.
    pub async fn attempt_row(&self, did: &str, attempt: &str) -> CoreResult<Option<AudienceRow>> {
        let q = sql(
            &format!(
                "SELECT {AUDIENCE_COLS} FROM kr_audience WHERE did = {{1}} AND adopted_by_attempt = {{2}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did, attempt)
    }

    /// Sets a row's value from a local write: a write-through prism accepted
    /// (`pending` none, the new version token), an edit queued during an
    /// outage (`pending` update), or a local audience's edit. Conditional on
    /// `version`. Clears a flag (an edit clears it).
    pub async fn write_audience_value(&self, r: &AudienceRow, version: i64) -> CoreResult<bool> {
        let q = sql(
            "UPDATE kr_audience SET name = {3}, mode = {4}, members_json = {5}, created_at = {6}, updated_at = {7}, \
             prism_cid = {8}, local_cid = {9}, value_cbor = {10}, pending = {11}, \
             pending_at = CASE WHEN {11} = 'none' THEN NULL ELSE {now} END, flagged = NULL, \
             row_version = row_version + 1 WHERE did = {1} AND rkey = {2} AND row_version = {12}",
            self.is_postgres(),
        );
        let n = exec!(
            self,
            q,
            &r.did,
            &r.rkey,
            &r.name,
            &r.mode,
            &r.members_json,
            &r.created_at,
            &r.updated_at,
            &r.prism_cid,
            &r.local_cid,
            &r.value_cbor,
            &r.pending,
            version
        )?;
        Ok(n == 1)
    }

    /// Queues a delete of a prism audience (an outage, the tier off).
    pub async fn queue_audience_delete(&self, did: &str, rkey: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE kr_audience SET pending = 'delete', pending_at = {now}, row_version = row_version + 1 \
             WHERE did = {1} AND rkey = {2}",
            self.is_postgres(),
        );
        exec!(self, q, did, rkey)?;
        Ok(())
    }

    /// How many audiences an account holds (local ones included).
    pub async fn count_audiences(&self, did: &str) -> CoreResult<i64> {
        let q = sql(
            "SELECT COUNT(*) FROM kr_audience WHERE did = {1} AND pending <> 'delete'",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_one, did)
    }

    /// Whether an rkey is used anywhere in an account's copy (a row, an
    /// alias either way, or a tombstone).
    pub async fn audience_rkey_taken(&self, did: &str, rkey: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM kr_audience WHERE did = {1} AND rkey = {2} \
             UNION ALL SELECT 1 FROM kr_audience_alias WHERE did = {1} AND (local_rkey = {2} OR prism_rkey = {2}) \
             UNION ALL SELECT 1 FROM kr_audience_tombstone WHERE did = {1} AND rkey = {2} LIMIT 1",
            self.is_postgres(),
        );
        let r: Option<i32> = scalar!(self, q, fetch_optional, did, rkey)?;
        Ok(r.is_some())
    }
}

impl Store {
    /// The mapping transaction, for a local audience `local_rkey` that prism
    /// now holds at `prism_rkey`:
    /// 1. prism's row: `prism_row` is written unless a refresh already
    ///    adopted it (then that row is kept);
    /// 2. if the local row was edited after its value was sent (its version
    ///    isn't `sent_version`), its current value goes onto prism's row as
    ///    a pending update; if it was deleted meanwhile, prism's row is a
    ///    pending delete;
    /// 3. the alias `local_rkey → prism_rkey`;
    /// 4. the local row goes, with a tombstone.
    ///
    /// Each step is conditional (no alias yet), so a mapping applied twice
    /// is a no-op. Returns whether it mapped.
    pub async fn map_local_audience(
        &self,
        did: &str,
        local_rkey: &str,
        sent_version: Option<i64>,
        prism_rkey: &str,
        prism_row: Option<&AudienceRow>,
    ) -> CoreResult<bool> {
        let mut tx = self.begin().await?;
        let pg = matches!(tx, Tx::Postgres(_));
        let alias = sql(
            "INSERT INTO kr_audience_alias (did, local_rkey, prism_rkey, mapped_at) VALUES ({1}, {2}, {3}, {now}) \
             ON CONFLICT (did, local_rkey) DO NOTHING",
            pg,
        );
        if tx_exec!(&mut tx, alias, did, local_rkey, prism_rkey)? == 0 {
            tx.rollback().await?;
            return Ok(false);
        }
        if let Some(r) = prism_row {
            let q = sql(
                &format!(
                    "INSERT INTO kr_audience ({AUDIENCE_COLS}, adopted_at) VALUES ({{1}}, {{2}}, {{3}}, {{4}}, {{5}}, \
                     {{6}}, {{7}}, {{8}}, {{9}}, {{10}}, {{11}}, {{12}}, {{13}}, {{14}}, {{now}}) \
                     ON CONFLICT (did, rkey) DO NOTHING"
                ),
                pg,
            );
            tx_exec!(
                &mut tx,
                q,
                &r.did,
                prism_rkey,
                &r.name,
                &r.mode,
                &r.members_json,
                &r.created_at,
                &r.updated_at,
                "prism",
                &r.prism_did,
                &r.prism_cid,
                &r.local_cid,
                &r.value_cbor,
                "none",
                1_i64
            )?;
        }
        let local_q = sql(
            &format!("SELECT {AUDIENCE_COLS} FROM kr_audience WHERE did = {{1}} AND rkey = {{2}}"),
            pg,
        );
        let local: Option<AudienceRow> = match &mut tx {
            Tx::Sqlite(t) => {
                sqlx::query_as(&local_q)
                    .bind(did)
                    .bind(local_rkey)
                    .fetch_optional(&mut **t)
                    .await
            }
            Tx::Postgres(t) => {
                sqlx::query_as(&local_q)
                    .bind(did)
                    .bind(local_rkey)
                    .fetch_optional(&mut **t)
                    .await
            }
        }
        .map_err(db_err)?;
        match &local {
            Some(l) if sent_version.is_some_and(|v| v != l.row_version) => {
                let q = sql(
                    "UPDATE kr_audience SET name = {3}, mode = {4}, members_json = {5}, updated_at = {6}, \
                     local_cid = {7}, value_cbor = {8}, pending = 'update', pending_at = {now}, \
                     row_version = row_version + 1 WHERE did = {1} AND rkey = {2}",
                    pg,
                );
                tx_exec!(
                    &mut tx,
                    q,
                    did,
                    prism_rkey,
                    &l.name,
                    &l.mode,
                    &l.members_json,
                    &l.updated_at,
                    &l.local_cid,
                    &l.value_cbor
                )?;
            }
            Some(_) => {}
            None => {
                let q = sql(
                    "UPDATE kr_audience SET pending = 'delete', pending_at = {now}, row_version = row_version + 1 \
                     WHERE did = {1} AND rkey = {2}",
                    pg,
                );
                tx_exec!(&mut tx, q, did, prism_rkey)?;
            }
        }
        if local.is_some() {
            let del = sql("DELETE FROM kr_audience WHERE did = {1} AND rkey = {2}", pg);
            tx_exec!(&mut tx, del, did, local_rkey)?;
            let t = sql(
                "INSERT INTO kr_audience_tombstone (did, rkey, deleted_at) VALUES ({1}, {2}, {now}) \
                 ON CONFLICT (did, rkey) DO UPDATE SET deleted_at = excluded.deleted_at",
                pg,
            );
            tx_exec!(&mut tx, t, did, local_rkey)?;
        }
        tx.commit().await?;
        Ok(true)
    }
}

// ---------------------------------------------------------------- sync and re-pointing

/// A private record naming a mapped local audience URI.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RepointRow {
    /// The repo holding the record.
    pub did: String,
    pub collection: String,
    pub rkey: String,
    /// The audience owner.
    pub audience_did: String,
    pub local_rkey: String,
    pub prism_rkey: String,
}

impl Store {
    /// An account's prism audiences with a queued edit or delete.
    pub async fn pending_audiences(&self, did: &str) -> CoreResult<Vec<AudienceRow>> {
        let q = sql(
            &format!(
                "SELECT {AUDIENCE_COLS} FROM kr_audience WHERE did = {{1}} AND origin = 'prism' \
                 AND pending <> 'none' ORDER BY pending_at"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    /// An account's local audiences not flagged, oldest first.
    pub async fn local_audiences(&self, did: &str) -> CoreResult<Vec<AudienceRow>> {
        let q = sql(
            &format!(
                "SELECT {AUDIENCE_COLS} FROM kr_audience WHERE did = {{1}} AND origin = 'local' \
                 AND flagged IS NULL ORDER BY rkey"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    /// Clears the `limit` flag of an account's local audiences (a complete
    /// listing showed room under prism's cap).
    pub async fn clear_limit_flags(&self, did: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE kr_audience SET flagged = NULL WHERE did = {1} AND origin = 'local' AND flagged = 'limit'",
            self.is_postgres(),
        );
        exec!(self, q, did)?;
        Ok(())
    }

    /// The newest attempt that tried to create a local audience in prism.
    pub async fn latest_local_attempt(
        &self,
        did: &str,
        local_rkey: &str,
    ) -> CoreResult<Option<AudienceAttempt>> {
        let q = sql(
            "SELECT id, did, value_json, created_at_value, updated_at_value, created_at, expires_at, outcome, \
             local_rkey, local_row_version FROM kr_audience_attempt WHERE did = {1} AND local_rkey = {2} \
             ORDER BY created_at DESC LIMIT 1",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did, local_rkey)
    }

    /// Makes sure the background poll knows about an account.
    pub async fn ensure_audience_sync(&self, did: &str) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO kr_audience_sync (did) VALUES ({1}) ON CONFLICT (did) DO NOTHING",
            self.is_postgres(),
        );
        exec!(self, q, did)?;
        Ok(())
    }

    /// How many accounts the background poll covers.
    pub async fn count_audience_syncs(&self) -> CoreResult<i64> {
        let q = String::from("SELECT COUNT(*) FROM kr_audience_sync");
        scalar!(self, q, fetch_one,)
    }

    /// Private records (in repos here) naming a mapped local audience URI,
    /// by repo.
    pub async fn repoint_candidates(&self, limit: i64) -> CoreResult<Vec<RepointRow>> {
        let q = sql(
            "SELECT p.did, p.collection, p.rkey, p.audience_did, a.local_rkey, a.prism_rkey \
             FROM kr_private_record p JOIN kr_audience_alias a \
             ON a.did = p.audience_did AND a.local_rkey = p.audience_rkey \
             WHERE a.flagged IS NULL ORDER BY p.did, p.rkey LIMIT {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, limit)
    }
}

impl Store {
    /// Whether `did` has a record in `collection` whose `path` links to
    /// `target` (a block, a follow).
    pub async fn has_backlink(
        &self,
        did: &str,
        collection: &str,
        path: &str,
        target: &str,
    ) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM backlink WHERE did = {1} AND collection = {2} AND path = {3} AND link_to = {4} LIMIT 1",
            self.is_postgres(),
        );
        let r: Option<i32> = scalar!(self, q, fetch_optional, did, collection, path, target)?;
        Ok(r.is_some())
    }
}
