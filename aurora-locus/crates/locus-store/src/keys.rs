//! Install-wide key state: the generation nodes reload on, key backups,
//! PLC rotation key changes and the PLC key scan; plus paged reads of
//! every stored secret, for backups.

use locus_core::keys::MasterKey;
use locus_core::{CoreError, CoreResult};
use zeroize::Zeroizing;

use crate::blob::sql;
use crate::identity::{PlcKeyRole, plc_key_secret_name};
use crate::secrets::{secret_aad, signing_key_aad};
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

macro_rules! tx_run {
    ($tx:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
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

/// The install's key state.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KeyState {
    pub generation: i64,
    pub last_backup_at: Option<i64>,
    pub last_backup_by: Option<String>,
    pub scan_cursor: Option<String>,
    pub scan_started_at: Option<i64>,
    pub scan_completed_at: Option<i64>,
}

/// A held PLC rotation key with its timestamps.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PlcKeyRow {
    pub did_key: String,
    pub role: String,
    pub created_at: i64,
    pub current_since: Option<i64>,
    pub secondary_since: Option<i64>,
}

/// How the accounts' documents list the held keys, from `plc_key_listing`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListingCounts {
    /// did:plc accounts with no listing row yet.
    pub unchecked: i64,
    /// Listing the current key.
    pub current: i64,
    /// Listing held keys, but not the current one.
    pub secondary_only: i64,
    /// Listing no held key at all.
    pub none: i64,
}

fn pg(store: &Store) -> bool {
    matches!(store, Store::Postgres(_))
}

fn tx_pg(tx: &Tx) -> bool {
    matches!(tx, Tx::Postgres(_))
}

impl Store {
    pub async fn key_state(&self) -> CoreResult<KeyState> {
        let q = "SELECT generation, last_backup_at, last_backup_by, scan_cursor, scan_started_at, \
                 scan_completed_at FROM key_state WHERE id = 1";
        run!(self, q, fetch_one,)
    }

    /// The database clock, in ms.
    pub async fn db_now_ms(&self) -> CoreResult<i64> {
        let q = sql("SELECT {now}", pg(self));
        let (n,): (i64,) = run!(self, q, fetch_one,)?;
        Ok(n)
    }

    /// Every held PLC rotation key, oldest first.
    pub async fn plc_key_rows(&self) -> CoreResult<Vec<PlcKeyRow>> {
        let q = "SELECT did_key, role, created_at, current_since, secondary_since \
                 FROM plc_rotation_key ORDER BY created_at";
        run!(self, q, fetch_all,)
    }

    /// Up to `limit` named secrets after `after`, decrypted.
    pub async fn secrets_page(
        &self,
        after: &str,
        limit: i64,
        key: &MasterKey,
    ) -> CoreResult<Vec<(String, Zeroizing<Vec<u8>>)>> {
        let q = sql(
            "SELECT name, sealed FROM secret WHERE name > {1} ORDER BY name LIMIT {2}",
            pg(self),
        );
        let rows: Vec<(String, Vec<u8>)> = run!(self, q, fetch_all, after, limit)?;
        rows.into_iter()
            .map(|(name, sealed)| {
                let v = key.open(&secret_aad(&name), &sealed)?;
                Ok((name, v))
            })
            .collect()
    }

    /// Up to `limit` account signing keys after `after` (by DID),
    /// decrypted: (did, key type, private key).
    pub async fn signing_keys_page(
        &self,
        after: &str,
        limit: i64,
        key: &MasterKey,
    ) -> CoreResult<Vec<(String, String, Zeroizing<Vec<u8>>)>> {
        let q = sql(
            "SELECT did, key_type, sealed FROM signing_key WHERE did > {1} ORDER BY did LIMIT {2}",
            pg(self),
        );
        let rows: Vec<(String, String, Vec<u8>)> = run!(self, q, fetch_all, after, limit)?;
        rows.into_iter()
            .map(|(did, key_type, sealed)| {
                let v = key.open(&signing_key_aad(&did), &sealed)?;
                Ok((did, key_type, v))
            })
            .collect()
    }

    /// Records a key backup started at `started_at` (database clock) by
    /// `by`. A pending PLC rotation key stored before the backup started is
    /// in it, so it becomes current. Returns the key that did.
    pub async fn record_key_backup(&self, started_at: i64, by: &str) -> CoreResult<Option<String>> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE key_state SET last_backup_at = {1}, last_backup_by = {2} WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, started_at, by)?;
        let q = sql(
            "SELECT did_key FROM plc_rotation_key WHERE role = 'pending' AND created_at < {1}",
            tx_pg(&tx),
        );
        let pending: Option<(String,)> = tx_run!(&mut tx, q, fetch_optional, started_at)?;
        let promoted = match pending {
            Some(_) => tx.promote_pending_plc_key().await?,
            None => None,
        };
        tx.commit().await?;
        Ok(promoted)
    }

    /// The hosted did:plc accounts whose documents must be fetched before
    /// `did_key` is removed: their listing names it, is missing, or was
    /// checked before `since`.
    pub async fn plc_guard_candidates(&self, did_key: &str, since: i64) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT a.did FROM account a LEFT JOIN plc_key_listing l ON l.did = a.did \
             WHERE a.did_method = 'plc' AND a.status <> 'deleted' \
             AND (l.did IS NULL OR l.listed_keys LIKE {1} OR l.checked_at < {2}) ORDER BY a.did",
            pg(self),
        );
        let pattern = format!("%\"{did_key}\"%");
        let rows: Vec<(String,)> = run!(self, q, fetch_all, pattern, since)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// The next `limit` hosted did:plc accounts after `after`, for the scan.
    pub async fn plc_accounts_after(&self, after: &str, limit: i64) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT did FROM account WHERE did_method = 'plc' AND status <> 'deleted' AND did > {1} \
             ORDER BY did LIMIT {2}",
            pg(self),
        );
        let rows: Vec<(String,)> = run!(self, q, fetch_all, after, limit)?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    /// Records the scan's progress: a cursor (None when a pass finished)
    /// and, on a start or a finish, the time.
    pub async fn set_plc_scan(
        &self,
        cursor: Option<&str>,
        started: bool,
        completed: bool,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let q = sql(
            "UPDATE key_state SET scan_cursor = {1}, \
             scan_started_at = CASE WHEN {2} THEN {now} ELSE scan_started_at END, \
             scan_completed_at = CASE WHEN {3} THEN {now} ELSE scan_completed_at END WHERE id = 1",
            tx_pg(&tx),
        );
        tx_exec!(&mut tx, q, cursor, started, completed)?;
        tx.commit().await
    }

    /// How the hosted did:plc accounts' documents list the held keys.
    pub async fn plc_listing_counts(&self, current: &str) -> CoreResult<ListingCounts> {
        let q = sql(
            "SELECT \
             COALESCE(SUM(CASE WHEN l.did IS NULL THEN 1 ELSE 0 END), 0), \
             COALESCE(SUM(CASE WHEN l.listed_keys LIKE {1} THEN 1 ELSE 0 END), 0), \
             COALESCE(SUM(CASE WHEN l.did IS NOT NULL AND l.listed_keys NOT LIKE {1} AND l.listed_keys <> '[]' THEN 1 ELSE 0 END), 0), \
             COALESCE(SUM(CASE WHEN l.listed_keys = '[]' THEN 1 ELSE 0 END), 0) \
             FROM account a LEFT JOIN plc_key_listing l ON l.did = a.did \
             WHERE a.did_method = 'plc' AND a.status <> 'deleted'",
            pg(self),
        );
        let pattern = format!("%\"{current}\"%");
        let (unchecked, cur, secondary_only, none): (i64, i64, i64, i64) =
            run!(self, q, fetch_one, pattern)?;
        Ok(ListingCounts {
            unchecked,
            current: cur,
            secondary_only,
            none,
        })
    }
}

impl Tx {
    /// Tells every node to reload the keys it holds in memory.
    pub async fn bump_key_generation(&mut self) -> CoreResult<()> {
        let q = "UPDATE key_state SET generation = generation + 1 WHERE id = 1";
        tx_exec!(self, q,)?;
        Ok(())
    }

    /// The pending PLC rotation key becomes current; the current one stays
    /// as a secondary signer. Returns the new current key.
    pub async fn promote_pending_plc_key(&mut self) -> CoreResult<Option<String>> {
        let q = "SELECT did_key FROM plc_rotation_key WHERE role = 'pending'";
        let pending: Option<(String,)> = tx_run!(self, q, fetch_optional,)?;
        let Some((did_key,)) = pending else {
            return Ok(None);
        };
        let q = sql(
            "UPDATE plc_rotation_key SET role = 'secondary', secondary_since = {now} WHERE role = 'current'",
            tx_pg(self),
        );
        tx_exec!(self, q,)?;
        let q = sql(
            "UPDATE plc_rotation_key SET role = 'current', current_since = {now}, secondary_since = NULL \
             WHERE did_key = {1}",
            tx_pg(self),
        );
        tx_exec!(self, q, &did_key)?;
        self.bump_key_generation().await?;
        Ok(Some(did_key))
    }

    /// Stores `did_key` as the pending PLC rotation key (`private` is its
    /// private half), replacing any other pending key; a held secondary key
    /// turns pending in place. Refused for the current key. Returns the
    /// pending key it replaced. The caller runs the listing guard for that
    /// one first.
    pub async fn store_pending_plc_key(
        &mut self,
        did_key: &str,
        private: &[u8],
        key: &MasterKey,
    ) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT role FROM plc_rotation_key WHERE did_key = {1}",
            tx_pg(self),
        );
        let existing: Option<(String,)> = tx_run!(self, q, fetch_optional, did_key)?;
        if let Some((role,)) = &existing {
            match PlcKeyRole::parse(role)? {
                PlcKeyRole::Current => {
                    return Err(CoreError::Invalid(
                        "That key is already the current PLC rotation key".into(),
                    ));
                }
                PlcKeyRole::Pending => return Ok(None),
                PlcKeyRole::Secondary => {}
            }
        }
        let q = "SELECT did_key FROM plc_rotation_key WHERE role = 'pending'";
        let replaced: Option<(String,)> = tx_run!(self, q, fetch_optional,)?;
        if let Some((old,)) = &replaced {
            self.remove_plc_key(old).await?;
        }
        if existing.is_some() {
            let q = sql(
                "UPDATE plc_rotation_key SET role = 'pending', secondary_since = NULL, created_at = {now} \
                 WHERE did_key = {1}",
                tx_pg(self),
            );
            tx_exec!(self, q, did_key)?;
        } else {
            self.put_secret(&plc_key_secret_name(did_key), private, key)
                .await?;
            self.insert_plc_rotation_key(did_key, PlcKeyRole::Pending)
                .await?;
        }
        self.bump_key_generation().await?;
        Ok(replaced.map(|r| r.0))
    }

    /// Removes a pending or secondary PLC rotation key and its private
    /// half. The current key can't be removed.
    pub async fn remove_plc_key(&mut self, did_key: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM plc_rotation_key WHERE did_key = {1} AND role <> 'current'",
            tx_pg(self),
        );
        if tx_exec!(self, q, did_key)? != 1 {
            return Err(CoreError::Invalid(
                "That isn't a pending or secondary PLC rotation key".into(),
            ));
        }
        let q = sql("DELETE FROM secret WHERE name = {1}", tx_pg(self));
        tx_exec!(self, q, plc_key_secret_name(did_key))?;
        let q = sql(
            "DELETE FROM plc_key_listing WHERE listed_keys = {1}",
            tx_pg(self),
        );
        // A listing that named only this key no longer names a held one.
        let only = serde_json::to_string(&[did_key]).unwrap_or_default();
        tx_exec!(self, q, only)?;
        self.bump_key_generation().await?;
        Ok(())
    }
}
