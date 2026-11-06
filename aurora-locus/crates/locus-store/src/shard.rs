//! Reserved signing keys (`reserveSigningKey`, every role) and the shard
//! role's store of entryway token verify keys.

use locus_core::CoreResult;
use locus_core::keys::MasterKey;
use zeroize::Zeroizing;

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

fn reserved_aad(id: &str) -> Vec<u8> {
    format!("reserved_key:{id}").into_bytes()
}

/// An entryway token verify key a shard accepts.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct VerifyKey {
    pub did_key: String,
    pub added_at: i64,
    pub expires_at: Option<i64>,
}

impl Store {
    /// Reserves a signing key under `id` (a DID, or the key's did:key),
    /// replacing an earlier reservation under the same id.
    pub async fn reserve_key(
        &self,
        id: &str,
        did_key: &str,
        private_key: &[u8],
        key: &MasterKey,
        ttl_ms: i64,
    ) -> CoreResult<()> {
        let sealed = key.seal(&reserved_aad(id), private_key)?;
        let q = sql(
            "INSERT INTO reserved_key (id, did_key, sealed, created_at, expires_at) \
             VALUES ({1}, {2}, {3}, {now}, {now} + {4}) \
             ON CONFLICT (id) DO UPDATE SET did_key = excluded.did_key, sealed = excluded.sealed, \
             created_at = excluded.created_at, expires_at = excluded.expires_at",
            self.is_postgres(),
        );
        exec!(self, q, id, did_key, &sealed, ttl_ms)?;
        Ok(())
    }

    /// The unexpired reserved key under `id`: (did:key, private key).
    pub async fn reserved_key(
        &self,
        id: &str,
        key: &MasterKey,
    ) -> CoreResult<Option<(String, Zeroizing<Vec<u8>>)>> {
        let q = sql(
            "SELECT did_key, sealed FROM reserved_key WHERE id = {1} AND expires_at > {now}",
            self.is_postgres(),
        );
        let row: Option<(String, Vec<u8>)> = run!(self, q, fetch_optional, id)?;
        match row {
            Some((did_key, sealed)) => Ok(Some((did_key, key.open(&reserved_aad(id), &sealed)?))),
            None => Ok(None),
        }
    }

    /// Removes expired reservations. Returns how many went.
    pub async fn prune_reserved_keys(&self) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM reserved_key WHERE expires_at <= {now}",
            self.is_postgres(),
        );
        exec!(self, q,)
    }

    /// The verify keys accepted now.
    pub async fn shard_verify_keys(&self) -> CoreResult<Vec<VerifyKey>> {
        let q = sql(
            "SELECT did_key, added_at, expires_at FROM shard_verify_key \
             WHERE expires_at IS NULL OR expires_at > {now} ORDER BY added_at DESC, did_key",
            self.is_postgres(),
        );
        run!(self, q, fetch_all,)
    }

    /// Whether the shard has any verify key row (expired or not).
    pub async fn has_shard_verify_keys(&self) -> CoreResult<bool> {
        let q = "SELECT did_key FROM shard_verify_key LIMIT 1";
        let r: Option<(String,)> = run!(self, q, fetch_optional,)?;
        Ok(r.is_some())
    }
}

impl Tx {
    /// Drops the reservations of a used key (under its DID and its did:key).
    pub async fn clear_reserved_key(&mut self, did: &str, did_key: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM reserved_key WHERE id = {1} OR id = {2} OR did_key = {2}",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(self, q, did, did_key)?;
        Ok(())
    }

    /// Makes `did_key` the accepted verify key. The others stay accepted
    /// for `keep_previous_ms` more (an end already sooner is kept), or end
    /// now when it's None.
    pub async fn set_shard_verify_key(
        &mut self,
        did_key: &str,
        keep_previous_ms: Option<i64>,
    ) -> CoreResult<()> {
        let pg = matches!(self, Tx::Postgres(_));
        match keep_previous_ms {
            Some(ms) => {
                let q = sql(
                    "UPDATE shard_verify_key SET expires_at = {now} + {1} \
                     WHERE did_key <> {2} AND (expires_at IS NULL OR expires_at > {now} + {1})",
                    pg,
                );
                tx_exec!(self, q, ms, did_key)?;
            }
            None => {
                let q = sql("DELETE FROM shard_verify_key WHERE did_key <> {1}", pg);
                tx_exec!(self, q, did_key)?;
            }
        }
        let q = sql(
            "INSERT INTO shard_verify_key (did_key, added_at, expires_at) VALUES ({1}, {now}, NULL) \
             ON CONFLICT (did_key) DO UPDATE SET expires_at = NULL",
            pg,
        );
        tx_exec!(self, q, did_key)?;
        Ok(())
    }

    /// Replaces the accepted set with exactly `keys`, none expiring (a
    /// locus entryway's `setVerifyKeys`, a shard restore).
    pub async fn replace_shard_verify_keys(&mut self, keys: &[String]) -> CoreResult<()> {
        let pg = matches!(self, Tx::Postgres(_));
        tx_exec!(self, "DELETE FROM shard_verify_key",)?;
        let q = sql(
            "INSERT INTO shard_verify_key (did_key, added_at, expires_at) VALUES ({1}, {now}, NULL)",
            pg,
        );
        for k in keys {
            tx_exec!(self, q, k)?;
        }
        Ok(())
    }
}

impl Store {
    /// Revokes one session of an account (its `sid`), for `keep_ms`.
    pub async fn revoke_shard_session(&self, did: &str, sid: &str, keep_ms: i64) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO shard_revocation (did, sid, not_before, expires_at) \
             VALUES ({1}, {2}, NULL, {now} + {3}) \
             ON CONFLICT (did, sid) DO UPDATE SET expires_at = excluded.expires_at",
            self.is_postgres(),
        );
        exec!(self, q, did, sid, keep_ms)?;
        Ok(())
    }

    /// Revokes every token of an account issued before `not_before_ms`
    /// (a later value wins), for `keep_ms`.
    pub async fn revoke_shard_account(
        &self,
        did: &str,
        not_before_ms: i64,
        keep_ms: i64,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO shard_revocation (did, sid, not_before, expires_at) \
             VALUES ({1}, '', {2}, {now} + {3}) \
             ON CONFLICT (did, sid) DO UPDATE SET \
             not_before = CASE WHEN shard_revocation.not_before > excluded.not_before \
                 THEN shard_revocation.not_before ELSE excluded.not_before END, \
             expires_at = excluded.expires_at",
            self.is_postgres(),
        );
        exec!(self, q, did, not_before_ms, keep_ms)?;
        Ok(())
    }

    /// Whether a token of `did` for session `sid` (empty: none named),
    /// issued at `iat_ms`, has been revoked.
    pub async fn shard_revoked(&self, did: &str, sid: &str, iat_ms: i64) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM shard_revocation WHERE did = {1} AND expires_at > {now} \
             AND ((sid = {2} AND sid <> '') OR (sid = '' AND not_before > {3})) LIMIT 1",
            self.is_postgres(),
        );
        let r: Option<(i32,)> = run!(self, q, fetch_optional, did, sid, iat_ms)?;
        Ok(r.is_some())
    }

    /// Removes revocations whose tokens have all expired.
    pub async fn prune_shard_revocations(&self) -> CoreResult<u64> {
        let q = sql(
            "DELETE FROM shard_revocation WHERE expires_at <= {now}",
            self.is_postgres(),
        );
        exec!(self, q,)
    }

    /// Installs the shard-callback token's hash, replacing any.
    pub async fn set_callback_token_hash(&self, hash: &str) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO shard_callback_token (id, token_hash, installed_at) VALUES (1, {1}, {now}) \
             ON CONFLICT (id) DO UPDATE SET token_hash = excluded.token_hash, \
             installed_at = excluded.installed_at",
            self.is_postgres(),
        );
        exec!(self, q, hash)?;
        Ok(())
    }

    /// The installed shard-callback token's hash, if one is installed.
    pub async fn callback_token_hash(&self) -> CoreResult<Option<String>> {
        let q = "SELECT token_hash FROM shard_callback_token WHERE id = 1";
        let r: Option<(String,)> = run!(self, q, fetch_optional,)?;
        Ok(r.map(|(h,)| h))
    }
}
