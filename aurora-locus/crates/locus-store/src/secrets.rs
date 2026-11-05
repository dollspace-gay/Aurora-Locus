//! The encrypted key store: named secrets and per-account signing keys,
//! sealed with the master key.
//!
//! The install has one master key. `master_key_check` holds its id and a MAC
//! under it. A node checks the row at boot ([`Store::check_master_key`]) and
//! every transaction that writes a secret checks it again
//! ([`Tx::ensure_master_key`]), inserting it if absent (the first writer
//! wins), so a node with a different key can never write a secret.

use locus_core::keys::MasterKey;
use locus_core::{CoreError, CoreResult};
use zeroize::Zeroizing;

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err, is_missing_table};

/// Outcome of the boot-time key check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCheck {
    /// No secret has been written yet.
    Empty,
    /// The stored check matches this key.
    Matches,
}

fn secret_aad(name: &str) -> Vec<u8> {
    format!("secret:{name}").into_bytes()
}

fn signing_key_aad(did: &str) -> Vec<u8> {
    format!("signing_key:{did}").into_bytes()
}

impl Store {
    /// Whether the key store is empty (no `master_key_check` row, or the
    /// table doesn't exist yet on a database that hasn't been migrated).
    pub async fn key_store_empty(&self) -> CoreResult<bool> {
        Ok(self.master_key_row().await?.is_none())
    }

    async fn master_key_row(&self) -> CoreResult<Option<(String, Vec<u8>)>> {
        let sql = "SELECT key_id, mac FROM master_key_check WHERE id = 1";
        let res = match self {
            Self::Sqlite(p) => sqlx::query_as(sql).fetch_optional(p).await,
            Self::Postgres(p) => sqlx::query_as(sql).fetch_optional(p).await,
        };
        match res {
            Ok(row) => Ok(row),
            Err(sqlx::Error::Database(e)) if is_missing_table(e.as_ref()) => Ok(None),
            Err(e) => Err(db_err(e)),
        }
    }

    /// The boot check: refuses a key that doesn't match the install's.
    pub async fn check_master_key(&self, key: &MasterKey) -> CoreResult<KeyCheck> {
        match self.master_key_row().await? {
            None => Ok(KeyCheck::Empty),
            Some((key_id, mac)) if key.verifies(&key_id, &mac) => Ok(KeyCheck::Matches),
            Some((key_id, _)) => Err(CoreError::Config(format!(
                "this node's master key (id {}, from {}) is not the install's key (id {key_id}); \
                 every node must use the same master key",
                key.key_id(),
                key.source().as_str()
            ))),
        }
    }

    /// Reads and decrypts a named secret.
    pub async fn get_secret(
        &self,
        name: &str,
        key: &MasterKey,
    ) -> CoreResult<Option<Zeroizing<Vec<u8>>>> {
        let sealed: Option<Vec<u8>> = match self {
            Self::Sqlite(p) => {
                sqlx::query_scalar("SELECT sealed FROM secret WHERE name = ?1")
                    .bind(name)
                    .fetch_optional(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_scalar("SELECT sealed FROM secret WHERE name = $1")
                    .bind(name)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)?;
        sealed.map(|s| key.open(&secret_aad(name), &s)).transpose()
    }

    /// Removes a named secret; true if there was one.
    pub async fn delete_secret(&self, name: &str) -> CoreResult<bool> {
        let n = match self {
            Self::Sqlite(p) => sqlx::query("DELETE FROM secret WHERE name = ?1")
                .bind(name)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
            Self::Postgres(p) => sqlx::query("DELETE FROM secret WHERE name = $1")
                .bind(name)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)?;
        Ok(n > 0)
    }

    /// Reads and decrypts an account's signing key: (key type, private key).
    pub async fn get_signing_key(
        &self,
        did: &str,
        key: &MasterKey,
    ) -> CoreResult<Option<(String, Zeroizing<Vec<u8>>)>> {
        let row: Option<(String, Vec<u8>)> = match self {
            Self::Sqlite(p) => {
                sqlx::query_as("SELECT key_type, sealed FROM signing_key WHERE did = ?1")
                    .bind(did)
                    .fetch_optional(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as("SELECT key_type, sealed FROM signing_key WHERE did = $1")
                    .bind(did)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)?;
        match row {
            None => Ok(None),
            Some((key_type, sealed)) => {
                Ok(Some((key_type, key.open(&signing_key_aad(did), &sealed)?)))
            }
        }
    }
}

impl Tx {
    /// The per-write key check: inserts the install's key check if absent
    /// (first writer wins), then requires it to match `key`. A mismatch
    /// fails, so the transaction writes no secret.
    pub async fn ensure_master_key(&mut self, key: &MasterKey) -> CoreResult<()> {
        let key_id = key.key_id();
        let mac = key.check_mac();
        let row: (String, Vec<u8>) = match self {
            Tx::Sqlite(t) => {
                sqlx::query(&format!(
                    "INSERT INTO master_key_check (id, key_id, source_kind, mac, created_at) \
                     VALUES (1, ?1, ?2, ?3, {SQLITE_NOW_MS}) ON CONFLICT (id) DO NOTHING"
                ))
                .bind(&key_id)
                .bind(key.source().as_str())
                .bind(&mac)
                .execute(&mut **t)
                .await
                .map_err(db_err)?;
                sqlx::query_as("SELECT key_id, mac FROM master_key_check WHERE id = 1")
                    .fetch_one(&mut **t)
                    .await
                    .map_err(db_err)?
            }
            Tx::Postgres(t) => {
                sqlx::query(&format!(
                    "INSERT INTO master_key_check (id, key_id, source_kind, mac, created_at) \
                     VALUES (1, $1, $2, $3, {PG_NOW_MS}) ON CONFLICT (id) DO NOTHING"
                ))
                .bind(&key_id)
                .bind(key.source().as_str())
                .bind(&mac)
                .execute(&mut **t)
                .await
                .map_err(db_err)?;
                sqlx::query_as("SELECT key_id, mac FROM master_key_check WHERE id = 1 FOR SHARE")
                    .fetch_one(&mut **t)
                    .await
                    .map_err(db_err)?
            }
        };
        if key.verifies(&row.0, &row.1) {
            Ok(())
        } else {
            Err(CoreError::Config(format!(
                "refusing to write a secret: this node's master key (id {key_id}) is not the install's (id {})",
                row.0
            )))
        }
    }

    /// Seals and stores a named secret.
    pub async fn put_secret(
        &mut self,
        name: &str,
        plaintext: &[u8],
        key: &MasterKey,
    ) -> CoreResult<()> {
        self.ensure_master_key(key).await?;
        let sealed = key.seal(&secret_aad(name), plaintext)?;
        match self {
            Tx::Sqlite(t) => sqlx::query(&format!(
                "INSERT INTO secret (name, sealed, updated_at) VALUES (?1, ?2, {SQLITE_NOW_MS}) \
                 ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, updated_at = excluded.updated_at"
            ))
            .bind(name)
            .bind(&sealed)
            .execute(&mut **t)
            .await
            .map(|_| ()),
            Tx::Postgres(t) => sqlx::query(&format!(
                "INSERT INTO secret (name, sealed, updated_at) VALUES ($1, $2, {PG_NOW_MS}) \
                 ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, updated_at = excluded.updated_at"
            ))
            .bind(name)
            .bind(&sealed)
            .execute(&mut **t)
            .await
            .map(|_| ()),
        }
        .map_err(db_err)
    }

    /// Seals and stores a named secret only if none exists yet (first writer
    /// wins). Returns whether this call stored it.
    pub async fn put_secret_if_absent(
        &mut self,
        name: &str,
        plaintext: &[u8],
        key: &MasterKey,
    ) -> CoreResult<bool> {
        self.ensure_master_key(key).await?;
        let sealed = key.seal(&secret_aad(name), plaintext)?;
        match self {
            Tx::Sqlite(t) => sqlx::query(&format!(
                "INSERT INTO secret (name, sealed, updated_at) VALUES (?1, ?2, {SQLITE_NOW_MS}) \
                 ON CONFLICT (name) DO NOTHING"
            ))
            .bind(name)
            .bind(&sealed)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected() == 1),
            Tx::Postgres(t) => sqlx::query(&format!(
                "INSERT INTO secret (name, sealed, updated_at) VALUES ($1, $2, {PG_NOW_MS}) \
                 ON CONFLICT (name) DO NOTHING"
            ))
            .bind(name)
            .bind(&sealed)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected() == 1),
        }
        .map_err(db_err)
    }

    /// Seals and stores an account's signing key (replacing any).
    pub async fn put_signing_key(
        &mut self,
        did: &str,
        key_type: &str,
        private_key: &[u8],
        key: &MasterKey,
    ) -> CoreResult<()> {
        self.ensure_master_key(key).await?;
        let sealed = key.seal(&signing_key_aad(did), private_key)?;
        match self {
            Tx::Sqlite(t) => sqlx::query(&format!(
                "INSERT INTO signing_key (did, key_type, sealed, created_at) VALUES (?1, ?2, ?3, {SQLITE_NOW_MS}) \
                 ON CONFLICT (did) DO UPDATE SET key_type = excluded.key_type, sealed = excluded.sealed"
            ))
            .bind(did)
            .bind(key_type)
            .bind(&sealed)
            .execute(&mut **t)
            .await
            .map(|_| ()),
            Tx::Postgres(t) => sqlx::query(&format!(
                "INSERT INTO signing_key (did, key_type, sealed, created_at) VALUES ($1, $2, $3, {PG_NOW_MS}) \
                 ON CONFLICT (did) DO UPDATE SET key_type = excluded.key_type, sealed = excluded.sealed"
            ))
            .bind(did)
            .bind(key_type)
            .bind(&sealed)
            .execute(&mut **t)
            .await
            .map(|_| ()),
        }
        .map_err(db_err)
    }
}
