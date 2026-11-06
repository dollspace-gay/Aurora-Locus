//! The entryway role's shard registry and each account's shard.

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

/// A shard behind the entryway.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Shard {
    pub name: String,
    pub url: String,
    pub did: String,
    /// It answered `describeCapabilities` as locus at the last `shard add`.
    pub locus: bool,
    pub weight: i64,
    pub draining: bool,
    pub added_at: i64,
    /// Accounts assigned to it.
    pub accounts: i64,
}

const COLS: &str = "s.name, s.url, s.did, s.locus, s.weight, s.draining, s.added_at, \
     (SELECT COUNT(*) FROM account a WHERE a.shard = s.name) AS accounts";

/// The secret holding a shard's admin password.
#[must_use]
pub fn admin_password_secret(name: &str) -> String {
    format!("shard_admin_password:{name}")
}

/// The secret holding a locus shard's callback token.
#[must_use]
pub fn callback_token_secret(name: &str) -> String {
    format!("shard_callback_token:{name}")
}

impl Store {
    /// Every shard, by name.
    pub async fn shards(&self) -> CoreResult<Vec<Shard>> {
        let q = format!("SELECT {COLS} FROM shard s ORDER BY s.name");
        run!(self, q, fetch_all,)
    }

    pub async fn shard(&self, name: &str) -> CoreResult<Option<Shard>> {
        let q = sql(
            &format!("SELECT {COLS} FROM shard s WHERE s.name = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, name)
    }

    /// The shard an account is on.
    pub async fn account_shard(&self, did: &str) -> CoreResult<Option<Shard>> {
        let q = sql(
            &format!(
                "SELECT {COLS} FROM shard s JOIN account x ON x.shard = s.name WHERE x.did = {{1}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did)
    }

    /// The DID of the shard an account is on.
    pub async fn account_shard_did(&self, did: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT s.did FROM shard s JOIN account a ON a.shard = s.name WHERE a.did = {1}",
            self.is_postgres(),
        );
        let r: Option<(String,)> = run!(self, q, fetch_optional, did)?;
        Ok(r.map(|(d,)| d))
    }

    /// The shard new accounts go to: the one with the most room for its
    /// weight (fewest accounts per unit of weight), not draining.
    pub async fn placement_shard(&self) -> CoreResult<Option<Shard>> {
        let mut best: Option<Shard> = None;
        for s in self.shards().await? {
            if s.draining || s.weight <= 0 {
                continue;
            }
            let better = match &best {
                None => true,
                // a/w < b/v  ⇔  a·v < b·w
                Some(b) => s.accounts * b.weight < b.accounts * s.weight,
            };
            if better {
                best = Some(s);
            }
        }
        Ok(best)
    }

    /// Adds a shard, or updates a listed one's URL, DID and kind.
    pub async fn upsert_shard(
        &self,
        name: &str,
        url: &str,
        did: &str,
        locus: bool,
        weight: i64,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO shard (name, url, did, locus, weight, draining, added_at) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {now}) \
             ON CONFLICT (name) DO UPDATE SET url = excluded.url, did = excluded.did, \
             locus = excluded.locus",
            self.is_postgres(),
        );
        exec!(self, q, name, url, did, locus, weight, false)?;
        Ok(())
    }

    pub async fn set_shard_weight(&self, name: &str, weight: i64) -> CoreResult<bool> {
        let q = sql(
            "UPDATE shard SET weight = {2} WHERE name = {1}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, name, weight)? == 1)
    }

    pub async fn set_shard_draining(&self, name: &str, draining: bool) -> CoreResult<bool> {
        let q = sql(
            "UPDATE shard SET draining = {2} WHERE name = {1}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, name, draining)? == 1)
    }
}

impl Tx {
    /// Assigns an account to a shard.
    pub async fn set_account_shard(&mut self, did: &str, shard: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE account SET shard = {2} WHERE did = {1}",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(self, q, did, shard)?;
        Ok(())
    }
}

// ---------------------------------------------------------------- outbox

macro_rules! tx_run {
    ($tx:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
        }
        .map_err(db_err)
    };
}

/// A queued locus callback to a shard.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ShardPush {
    pub id: i64,
    pub shard: String,
    /// `revokeSessions`, `setPrivateTierBlock` or `setVerifyKeys`.
    pub kind: String,
    /// The account, or `''` (setVerifyKeys).
    pub subject: String,
    pub body: String,
    pub attempts: i64,
    pub next_at: i64,
    pub last_error: Option<String>,
}

const PUSH_COLS: &str = "id, shard, kind, subject, body, attempts, next_at, last_error";

/// The entryway's token-key rotation.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct KeyRotation {
    pub old_key: String,
    pub new_key: String,
    pub started_at: i64,
    /// JSON list of the shards that took (or were confirmed for) `{old, new}`.
    pub acknowledged: String,
    pub switched_at: Option<i64>,
    pub finished_at: Option<i64>,
}

impl KeyRotation {
    #[must_use]
    pub fn acknowledged_shards(&self) -> Vec<String> {
        serde_json::from_str(&self.acknowledged).unwrap_or_default()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Tx {
    /// Queues a locus callback to a shard. A later `setPrivateTierBlock`
    /// for the same account, or a later `setVerifyKeys`, drops the queued
    /// earlier ones unsent.
    pub async fn queue_shard_push(
        &mut self,
        shard: &str,
        kind: &str,
        subject: &str,
        body: &serde_json::Value,
    ) -> CoreResult<()> {
        let pg = matches!(self, Tx::Postgres(_));
        if kind == "setPrivateTierBlock" || kind == "setVerifyKeys" {
            let q = sql(
                "DELETE FROM shard_outbox WHERE shard = {1} AND kind = {2} AND subject = {3}",
                pg,
            );
            tx_exec!(self, q, shard, kind, subject)?;
        }
        let q = sql(
            "INSERT INTO shard_outbox (shard, kind, subject, body, created_at, next_at) \
             VALUES ({1}, {2}, {3}, {4}, {now}, {now})",
            pg,
        );
        tx_exec!(self, q, shard, kind, subject, body.to_string())?;
        Ok(())
    }

    /// The account's locus shard, if it's on one (only in the entryway
    /// role do accounts have shards).
    pub async fn account_locus_shard(&mut self, did: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT s.name FROM shard s JOIN account a ON a.shard = s.name \
             WHERE a.did = {1} AND s.locus = {2}",
            matches!(self, Tx::Postgres(_)),
        );
        let r: Option<(String,)> = tx_run!(self, q, fetch_optional, did, true)?;
        Ok(r.map(|(n,)| n))
    }

    /// Queues `revokeSessions` for revoked sessions of an account on a
    /// locus shard: these `sids`, or (None) every token issued before now.
    pub async fn queue_revocation(&mut self, did: &str, sids: Option<&[String]>) -> CoreResult<()> {
        let Some(shard) = self.account_locus_shard(did).await? else {
            return Ok(());
        };
        match sids {
            Some(list) => {
                for sid in list {
                    self.queue_shard_push(
                        &shard,
                        "revokeSessions",
                        did,
                        &serde_json::json!({"did": did, "sid": sid}),
                    )
                    .await?;
                }
            }
            None => {
                let at = locus_core::time::iso8601(now_ms());
                self.queue_shard_push(
                    &shard,
                    "revokeSessions",
                    did,
                    &serde_json::json!({"did": did, "notBefore": at}),
                )
                .await?;
            }
        }
        Ok(())
    }

    /// The live sessions of an account made with an app password.
    pub async fn app_password_sessions(
        &mut self,
        did: &str,
        name: &str,
    ) -> CoreResult<Vec<String>> {
        let q = sql(
            "SELECT id FROM session WHERE did = {1} AND app_password_name = {2} AND revoked_at IS NULL",
            matches!(self, Tx::Postgres(_)),
        );
        let rows: Vec<(String,)> = tx_run!(self, q, fetch_all, did, name)?;
        Ok(rows.into_iter().map(|(i,)| i).collect())
    }

    /// Starts a token-key rotation (refused while one runs).
    pub async fn start_key_rotation(&mut self, old_key: &str, new_key: &str) -> CoreResult<bool> {
        let q = sql(
            "INSERT INTO entryway_key_rotation (id, old_key, new_key, started_at, acknowledged) \
             VALUES (1, {1}, {2}, {now}, '[]') \
             ON CONFLICT (id) DO UPDATE SET old_key = excluded.old_key, new_key = excluded.new_key, \
             started_at = excluded.started_at, acknowledged = '[]', switched_at = NULL, finished_at = NULL \
             WHERE entryway_key_rotation.finished_at IS NOT NULL",
            matches!(self, Tx::Postgres(_)),
        );
        Ok(tx_exec!(self, q, old_key, new_key)? == 1)
    }
}

impl Store {
    /// The oldest queued callback for a shard, if it's due (FIFO: a later
    /// row waits for it).
    pub async fn next_shard_push(&self, shard: &str) -> CoreResult<Option<ShardPush>> {
        let q = sql(
            &format!(
                "SELECT {PUSH_COLS} FROM shard_outbox WHERE shard = {{1}} ORDER BY id LIMIT 1"
            ),
            self.is_postgres(),
        );
        let row: Option<ShardPush> = run!(self, q, fetch_optional, shard)?;
        Ok(row.filter(|r| r.next_at <= now_ms()))
    }

    pub async fn delete_shard_push(&self, id: i64) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM shard_outbox WHERE id = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id)?;
        Ok(())
    }

    /// Records a failed delivery; the row is tried again after `delay_ms`.
    pub async fn retry_shard_push(&self, id: i64, error: &str, delay_ms: i64) -> CoreResult<()> {
        let q = sql(
            "UPDATE shard_outbox SET attempts = attempts + 1, last_error = {2}, next_at = {now} + {3} \
             WHERE id = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id, error, delay_ms)?;
        Ok(())
    }

    /// Each shard's queued callbacks: (shard, count, oldest error).
    pub async fn shard_backlog(&self) -> CoreResult<Vec<(String, i64, Option<String>)>> {
        let q = "SELECT shard, COUNT(*), MAX(last_error) FROM shard_outbox GROUP BY shard ORDER BY shard";
        run!(self, q, fetch_all,)
    }

    /// Drops a shard's queued callbacks (it's no longer locus). Returns them.
    pub async fn drop_shard_pushes(&self, shard: &str) -> CoreResult<Vec<ShardPush>> {
        let q = sql(
            &format!("SELECT {PUSH_COLS} FROM shard_outbox WHERE shard = {{1}} ORDER BY id"),
            self.is_postgres(),
        );
        let rows: Vec<ShardPush> = run!(self, q, fetch_all, shard)?;
        let q = sql(
            "DELETE FROM shard_outbox WHERE shard = {1}",
            self.is_postgres(),
        );
        exec!(self, q, shard)?;
        Ok(rows)
    }

    /// Queues one callback (its own transaction).
    pub async fn queue_shard_push(
        &self,
        shard: &str,
        kind: &str,
        subject: &str,
        body: &serde_json::Value,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.queue_shard_push(shard, kind, subject, body).await?;
        tx.commit().await
    }

    /// The accounts on a shard, with their private-tier block.
    pub async fn shard_accounts_blocks(&self, shard: &str) -> CoreResult<Vec<(String, bool)>> {
        let q = sql(
            "SELECT did, private_tier_blocked FROM account WHERE shard = {1} AND status <> 'deleted'",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, shard)
    }

    /// The token-key rotation, if one was ever started.
    pub async fn key_rotation(&self) -> CoreResult<Option<KeyRotation>> {
        let q = "SELECT old_key, new_key, started_at, acknowledged, switched_at, finished_at \
                 FROM entryway_key_rotation WHERE id = 1";
        run!(self, q, fetch_optional,)
    }

    /// Adds a shard to the rotation's acknowledgements.
    pub async fn acknowledge_rotation(&self, shard: &str) -> CoreResult<()> {
        let Some(r) = self.key_rotation().await? else {
            return Ok(());
        };
        let mut list = r.acknowledged_shards();
        if !list.iter().any(|s| s == shard) {
            list.push(shard.to_owned());
        }
        let q = sql(
            "UPDATE entryway_key_rotation SET acknowledged = {1} WHERE id = 1 AND switched_at IS NULL",
            self.is_postgres(),
        );
        exec!(self, q, serde_json::to_string(&list).unwrap_or_default())?;
        Ok(())
    }

    /// Marks the rotation switched (new key signing) or finished.
    pub async fn mark_rotation(&self, switched: bool) -> CoreResult<()> {
        let col = if switched {
            "switched_at"
        } else {
            "finished_at"
        };
        let q = sql(
            &format!("UPDATE entryway_key_rotation SET {col} = {{now}} WHERE id = 1"),
            self.is_postgres(),
        );
        exec!(self, q,)?;
        Ok(())
    }
}
