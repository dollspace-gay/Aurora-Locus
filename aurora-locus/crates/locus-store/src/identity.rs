//! PLC rotation keys and key listings, the DID and handle caches, and hosted
//! did:web documents.
//!
//! Cache ages are measured on the database clock, so every node agrees on
//! whether an entry is fresh.

use locus_core::{CoreError, CoreResult};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// A PLC rotation key's role. Every row is a held key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlcKeyRole {
    Pending,
    Current,
    Secondary,
}

impl PlcKeyRole {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Current => "current",
            Self::Secondary => "secondary",
        }
    }

    pub fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "current" => Ok(Self::Current),
            "secondary" => Ok(Self::Secondary),
            other => Err(CoreError::Storage(format!(
                "unknown PLC key role {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlcRotationKey {
    pub did_key: String,
    pub role: PlcKeyRole,
    pub created_at: i64,
}

/// The key-store name of a PLC rotation key's private half.
#[must_use]
pub fn plc_key_secret_name(did_key: &str) -> String {
    format!("plc_rotation_key/{did_key}")
}

/// A cached value and its age in ms, by the database clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached<T> {
    pub value: T,
    pub age_ms: i64,
}

/// A hosted did:web row. `doc` is None once the account is gone; the row
/// stays so the host stays reserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedDidWeb {
    pub did: String,
    pub host: String,
    pub doc: Option<String>,
}

/// An identity change the repair job converges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairKind {
    /// A handle change; the value is the new handle.
    Handle,
    /// A tombstone; the value is the CID of the DID's genesis operation.
    Tombstone,
}

impl RepairKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Handle => "handle",
            Self::Tombstone => "tombstone",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityRepair {
    pub did: String,
    pub kind: RepairKind,
    pub value: String,
    pub attempts: i64,
    /// Age in ms, by the database clock.
    pub age_ms: i64,
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

impl Store {
    /// Every held PLC rotation key.
    pub async fn plc_rotation_keys(&self) -> CoreResult<Vec<PlcRotationKey>> {
        let sql = "SELECT did_key, role, created_at FROM plc_rotation_key ORDER BY created_at";
        let rows: Vec<(String, String, i64)> = on_pool!(self, sql, sql, |q, p| sqlx::query_as(q)
            .fetch_all(p)
            .await
            .map_err(db_err))?;
        rows.into_iter()
            .map(|(did_key, role, created_at)| {
                Ok(PlcRotationKey {
                    did_key,
                    role: PlcKeyRole::parse(&role)?,
                    created_at,
                })
            })
            .collect()
    }

    /// The held keys `did`'s PLC document listed when last checked.
    pub async fn plc_key_listing(&self, did: &str) -> CoreResult<Option<Vec<String>>> {
        let raw: Option<String> = on_pool!(
            self,
            "SELECT listed_keys FROM plc_key_listing WHERE did = ?1",
            "SELECT listed_keys FROM plc_key_listing WHERE did = $1",
            |q, p| sqlx::query_scalar(q)
                .bind(did)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        raw.map(|r| serde_json::from_str(&r).map_err(|e| CoreError::Storage(e.to_string())))
            .transpose()
    }

    /// Records which held keys `did`'s PLC document lists.
    pub async fn set_plc_key_listing(&self, did: &str, listed: &[String]) -> CoreResult<()> {
        let json = serde_json::to_string(listed).map_err(|e| CoreError::Storage(e.to_string()))?;
        let upsert = |now: &str, a: &str, b: &str| {
            format!(
                "INSERT INTO plc_key_listing (did, listed_keys, checked_at) VALUES ({a}, {b}, {now}) \
                 ON CONFLICT (did) DO UPDATE SET listed_keys = excluded.listed_keys, \
                 checked_at = excluded.checked_at"
            )
        };
        on_pool!(
            self,
            upsert(SQLITE_NOW_MS, "?1", "?2"),
            upsert(PG_NOW_MS, "$1", "$2"),
            |q, p| sqlx::query(q)
                .bind(did)
                .bind(&json)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// A cached DID document (JSON text) and its age.
    pub async fn cached_did_doc(&self, did: &str) -> CoreResult<Option<Cached<String>>> {
        let row: Option<(String, i64)> = on_pool!(
            self,
            format!("SELECT doc, {SQLITE_NOW_MS} - updated_at FROM did_cache WHERE did = ?1"),
            format!("SELECT doc, {PG_NOW_MS} - updated_at FROM did_cache WHERE did = $1"),
            |q, p| sqlx::query_as(q)
                .bind(did)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(|(value, age_ms)| Cached { value, age_ms }))
    }

    pub async fn cache_did_doc(&self, did: &str, doc: &str) -> CoreResult<()> {
        let upsert = |now: &str, a: &str, b: &str| {
            format!(
                "INSERT INTO did_cache (did, doc, updated_at) VALUES ({a}, {b}, {now}) \
                 ON CONFLICT (did) DO UPDATE SET doc = excluded.doc, updated_at = excluded.updated_at"
            )
        };
        on_pool!(
            self,
            upsert(SQLITE_NOW_MS, "?1", "?2"),
            upsert(PG_NOW_MS, "$1", "$2"),
            |q, p| sqlx::query(q)
                .bind(did)
                .bind(doc)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    pub async fn clear_did_doc(&self, did: &str) -> CoreResult<()> {
        on_pool!(
            self,
            "DELETE FROM did_cache WHERE did = ?1",
            "DELETE FROM did_cache WHERE did = $1",
            |q, p| sqlx::query(q)
                .bind(did)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// A cached handle lookup: Some(None) is a cached "doesn't resolve".
    pub async fn cached_handle(&self, handle: &str) -> CoreResult<Option<Cached<Option<String>>>> {
        let row: Option<(Option<String>, i64)> = on_pool!(
            self,
            format!("SELECT did, {SQLITE_NOW_MS} - updated_at FROM handle_cache WHERE handle = ?1"),
            format!("SELECT did, {PG_NOW_MS} - updated_at FROM handle_cache WHERE handle = $1"),
            |q, p| sqlx::query_as(q)
                .bind(handle)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(|(value, age_ms)| Cached { value, age_ms }))
    }

    pub async fn cache_handle(&self, handle: &str, did: Option<&str>) -> CoreResult<()> {
        let upsert = |now: &str, a: &str, b: &str| {
            format!(
                "INSERT INTO handle_cache (handle, did, updated_at) VALUES ({a}, {b}, {now}) \
                 ON CONFLICT (handle) DO UPDATE SET did = excluded.did, updated_at = excluded.updated_at"
            )
        };
        on_pool!(
            self,
            upsert(SQLITE_NOW_MS, "?1", "?2"),
            upsert(PG_NOW_MS, "$1", "$2"),
            |q, p| sqlx::query(q)
                .bind(handle)
                .bind(did)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    pub async fn clear_handle_cache(&self, handle: &str) -> CoreResult<()> {
        on_pool!(
            self,
            "DELETE FROM handle_cache WHERE handle = ?1",
            "DELETE FROM handle_cache WHERE handle = $1",
            |q, p| sqlx::query(q)
                .bind(handle)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Records an identity change to converge (replacing one of the same kind).
    pub async fn queue_identity_repair(
        &self,
        did: &str,
        kind: RepairKind,
        value: &str,
    ) -> CoreResult<()> {
        let upsert = |now: &str, a: &str, b: &str, c: &str| {
            format!(
                "INSERT INTO identity_repair (did, kind, value, created_at, next_at) \
                 VALUES ({a}, {b}, {c}, {now}, {now}) \
                 ON CONFLICT (did, kind) DO UPDATE SET value = excluded.value, \
                 created_at = excluded.created_at, attempts = 0, next_at = excluded.next_at, last_error = NULL"
            )
        };
        on_pool!(
            self,
            upsert(SQLITE_NOW_MS, "?1", "?2", "?3"),
            upsert(PG_NOW_MS, "$1", "$2", "$3"),
            |q, p| sqlx::query(q)
                .bind(did)
                .bind(kind.as_str())
                .bind(value)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Removes a repair entry, but only if it still holds `value` (a newer
    /// change of the same kind replaces it).
    pub async fn finish_identity_repair(
        &self,
        did: &str,
        kind: RepairKind,
        value: &str,
    ) -> CoreResult<()> {
        on_pool!(
            self,
            "DELETE FROM identity_repair WHERE did = ?1 AND kind = ?2 AND value = ?3",
            "DELETE FROM identity_repair WHERE did = $1 AND kind = $2 AND value = $3",
            |q, p| sqlx::query(q)
                .bind(did)
                .bind(kind.as_str())
                .bind(value)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Records a failed attempt and when to try again.
    pub async fn retry_identity_repair(
        &self,
        did: &str,
        kind: RepairKind,
        error: &str,
        backoff_ms: i64,
    ) -> CoreResult<()> {
        let upd = |now: &str, a: &str, b: &str, c: &str, d: &str| {
            format!(
                "UPDATE identity_repair SET attempts = attempts + 1, last_error = {c}, \
                 next_at = {now} + {d} WHERE did = {a} AND kind = {b}"
            )
        };
        on_pool!(
            self,
            upd(SQLITE_NOW_MS, "?1", "?2", "?3", "?4"),
            upd(PG_NOW_MS, "$1", "$2", "$3", "$4"),
            |q, p| sqlx::query(q)
                .bind(did)
                .bind(kind.as_str())
                .bind(error)
                .bind(backoff_ms)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Repairs that are due, oldest first.
    pub async fn due_identity_repairs(&self, limit: i64) -> CoreResult<Vec<IdentityRepair>> {
        let rows: Vec<(String, String, String, i64, i64)> = on_pool!(
            self,
            format!(
                "SELECT did, kind, value, attempts, {SQLITE_NOW_MS} - created_at FROM identity_repair \
                 WHERE next_at <= {SQLITE_NOW_MS} ORDER BY next_at LIMIT ?1"
            ),
            format!(
                "SELECT did, kind, value, attempts::bigint, {PG_NOW_MS} - created_at FROM identity_repair \
                 WHERE next_at <= {PG_NOW_MS} ORDER BY next_at LIMIT $1"
            ),
            |q, p| sqlx::query_as(q)
                .bind(limit)
                .fetch_all(p)
                .await
                .map_err(db_err)
        )?;
        rows.into_iter()
            .map(|(did, kind, value, attempts, age_ms)| {
                let kind = match kind.as_str() {
                    "handle" => RepairKind::Handle,
                    "tombstone" => RepairKind::Tombstone,
                    other => {
                        return Err(CoreError::Storage(format!("unknown repair kind {other:?}")));
                    }
                };
                Ok(IdentityRepair {
                    did,
                    kind,
                    value,
                    attempts,
                    age_ms,
                })
            })
            .collect()
    }

    /// The hosted did:web row for a host, past or present.
    pub async fn hosted_did_web_by_host(&self, host: &str) -> CoreResult<Option<HostedDidWeb>> {
        let row: Option<(String, String, Option<String>)> = on_pool!(
            self,
            "SELECT did, host, doc FROM hosted_did_web WHERE host = ?1",
            "SELECT did, host, doc FROM hosted_did_web WHERE host = $1",
            |q, p| sqlx::query_as(q)
                .bind(host)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(|(did, host, doc)| HostedDidWeb { did, host, doc }))
    }

    pub async fn hosted_did_web(&self, did: &str) -> CoreResult<Option<HostedDidWeb>> {
        let row: Option<(String, String, Option<String>)> = on_pool!(
            self,
            "SELECT did, host, doc FROM hosted_did_web WHERE did = ?1",
            "SELECT did, host, doc FROM hosted_did_web WHERE did = $1",
            |q, p| sqlx::query_as(q)
                .bind(did)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(|(did, host, doc)| HostedDidWeb { did, host, doc }))
    }
}

impl Tx {
    /// Adds a held PLC rotation key. A second `current` (or `pending`) row
    /// is refused by the unique index, as [`CoreError::Conflict`].
    pub async fn insert_plc_rotation_key(
        &mut self,
        did_key: &str,
        role: PlcKeyRole,
    ) -> CoreResult<()> {
        let ins = |now: &str, a: &str, b: &str| {
            let current_since = if role == PlcKeyRole::Current {
                now
            } else {
                "NULL"
            };
            format!(
                "INSERT INTO plc_rotation_key (did_key, role, created_at, current_since) \
                 VALUES ({a}, {b}, {now}, {current_since})"
            )
        };
        on_tx!(
            self,
            ins(SQLITE_NOW_MS, "?1", "?2"),
            ins(PG_NOW_MS, "$1", "$2"),
            |q, ex| sqlx::query(q)
                .bind(did_key)
                .bind(role.as_str())
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(|e| if crate::accounts::is_unique_violation(&e) {
                    CoreError::Conflict(format!(
                        "a {} PLC rotation key already exists",
                        role.as_str()
                    ))
                } else {
                    db_err(e)
                })
        )
    }

    /// Hosts a did:web document. The host must not be tied to another DID.
    pub async fn insert_hosted_did_web(
        &mut self,
        did: &str,
        host: &str,
        doc: &str,
    ) -> CoreResult<()> {
        let ins = |now: &str, a: &str, b: &str, c: &str| {
            format!(
                "INSERT INTO hosted_did_web (did, host, doc, updated_at) VALUES ({a}, {b}, {c}, {now})"
            )
        };
        on_tx!(
            self,
            ins(SQLITE_NOW_MS, "?1", "?2", "?3"),
            ins(PG_NOW_MS, "$1", "$2", "$3"),
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(host)
                .bind(doc)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(|e| if crate::accounts::is_unique_violation(&e) {
                    CoreError::Conflict("Handle already taken".into())
                } else {
                    db_err(e)
                })
        )
    }

    /// Replaces a hosted did:web document (None once the account is gone).
    pub async fn set_hosted_did_web_doc(
        &mut self,
        did: &str,
        doc: Option<&str>,
    ) -> CoreResult<bool> {
        let upd = |now: &str, a: &str, b: &str| {
            format!("UPDATE hosted_did_web SET doc = {b}, updated_at = {now} WHERE did = {a}")
        };
        on_tx!(
            self,
            upd(SQLITE_NOW_MS, "?1", "?2"),
            upd(PG_NOW_MS, "$1", "$2"),
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(doc)
                .execute(ex)
                .await
                .map(|r| r.rows_affected() == 1)
                .map_err(db_err)
        )
    }

    /// The hosted did:web document, locked for update on Postgres.
    pub async fn hosted_did_web_for_update(
        &mut self,
        did: &str,
    ) -> CoreResult<Option<HostedDidWeb>> {
        let row: Option<(String, String, Option<String>)> = on_tx!(
            self,
            "SELECT did, host, doc FROM hosted_did_web WHERE did = ?1",
            "SELECT did, host, doc FROM hosted_did_web WHERE did = $1 FOR UPDATE",
            |q, ex| sqlx::query_as(q)
                .bind(did)
                .fetch_optional(ex)
                .await
                .map_err(db_err)
        )?;
        Ok(row.map(|(did, host, doc)| HostedDidWeb { did, host, doc }))
    }
}
