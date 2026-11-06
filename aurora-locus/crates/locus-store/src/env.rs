//! Env/file values for runtime settings.
//!
//! Env values seed settings at first boot, and a value that later **changes**
//! is applied again (audited as "environment"), so the reference workflow of
//! editing `pds.env` and restarting keeps working. "Changed" is judged
//! install-wide: each node reports a hash of every env value it has; a value
//! applies only when every live node (one holding a clock-id slot) reports
//! the same hash for the key and it differs from the last applied hash. The
//! apply is a compare-and-set on `env_applied`, so concurrent nodes apply and
//! audit a value exactly once. An unchanged env value never overrides a later
//! console change.

use std::collections::{BTreeMap, HashMap};

use locus_core::settings::REGISTRY;
use locus_core::{CoreError, CoreResult};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// The lease family of per-node TID clock-id slots. Holding one makes a node live.
pub const CLOCK_SLOT_FAMILY: &str = "clock";

/// Actor name recorded when an env value is applied.
pub const ENV_ACTOR: &str = "environment";

/// One env value for a registered setting.
#[derive(Debug, Clone)]
pub struct EnvValue {
    pub key: &'static str,
    pub value: Value,
    pub hash: String,
}

/// Env values for every registered setting this node has. Values that don't
/// parse are returned as errors, one per variable, so the caller can warn and
/// carry on.
pub fn collect_env(env: &HashMap<String, String>) -> (Vec<EnvValue>, Vec<CoreError>) {
    let mut values = Vec::new();
    let mut errors = Vec::new();
    for def in REGISTRY {
        let Some(var) = def.env else { continue };
        let Some(raw) = env.get(var) else { continue };
        match def.parse_env(raw) {
            Ok(value) => values.push(EnvValue {
                key: def.key,
                hash: value_hash(&value),
                value,
            }),
            Err(e) => errors.push(CoreError::Invalid(format!("{var}: {e}"))),
        }
    }
    (values, errors)
}

pub fn value_hash(value: &Value) -> String {
    hex::encode(Sha256::digest(value.to_string().as_bytes()))
}

impl Store {
    /// Records this node's env values in `env_report` (and removes rows for
    /// keys it no longer has), prunes reports of nodes that aren't live, then
    /// applies every value that the rule allows. Returns the keys applied.
    pub async fn report_and_apply_env(
        &self,
        node_id: &str,
        values: &[EnvValue],
    ) -> CoreResult<Vec<String>> {
        let reported: Vec<&str> = values.iter().map(|v| v.key).collect();
        let mut tx = self.begin().await?;
        for v in values {
            tx.upsert_env_report(node_id, v.key, &v.hash).await?;
        }
        tx.delete_env_reports_except(node_id, &reported).await?;
        tx.commit().await?;

        self.apply_env(values).await
    }

    /// Re-checks the rule for this node's values (run at boot and on a timer:
    /// a node's lease lapsing produces no event).
    pub async fn apply_env(&self, values: &[EnvValue]) -> CoreResult<Vec<String>> {
        let live = self.live_slot_holders(CLOCK_SLOT_FAMILY).await?;
        self.prune_env_reports(&live).await?;
        let reports = self.env_reports().await?;
        let applied = self.env_applied_hashes().await?;

        let mut done = Vec::new();
        for v in values {
            let mut hashes = Vec::new();
            for node in &live {
                hashes.push(reports.get(&(node.clone(), v.key.to_owned())));
            }
            // Every live node must report this key with this node's hash.
            let unanimous = !hashes.is_empty() && hashes.iter().all(|h| h == &Some(&v.hash));
            if !unanimous {
                continue;
            }
            let previous = applied.get(v.key);
            if previous == Some(&v.hash) {
                continue;
            }
            let mut tx = self.begin().await?;
            if !tx
                .cas_env_applied(v.key, previous.map(String::as_str), &v.hash)
                .await?
            {
                // Another node applied first.
                tx.rollback().await?;
                continue;
            }
            tx.set_setting(v.key, v.value.clone(), ENV_ACTOR).await?;
            tx.commit().await?;
            done.push(v.key.to_owned());
        }
        Ok(done)
    }

    async fn prune_env_reports(&self, live: &[String]) -> CoreResult<()> {
        let all: Vec<String> = match self {
            Self::Sqlite(p) => {
                sqlx::query_scalar("SELECT DISTINCT node_id FROM env_report")
                    .fetch_all(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_scalar("SELECT DISTINCT node_id FROM env_report")
                    .fetch_all(p)
                    .await
            }
        }
        .map_err(db_err)?;
        for node in all.iter().filter(|n| !live.contains(n)) {
            match self {
                Self::Sqlite(p) => sqlx::query("DELETE FROM env_report WHERE node_id = ?1")
                    .bind(node)
                    .execute(p)
                    .await
                    .map(|_| ()),
                Self::Postgres(p) => sqlx::query("DELETE FROM env_report WHERE node_id = $1")
                    .bind(node)
                    .execute(p)
                    .await
                    .map(|_| ()),
            }
            .map_err(db_err)?;
        }
        Ok(())
    }

    async fn env_reports(&self) -> CoreResult<BTreeMap<(String, String), String>> {
        let sql = "SELECT node_id, key, hash FROM env_report";
        let rows: Vec<(String, String, String)> = match self {
            Self::Sqlite(p) => sqlx::query_as(sql).fetch_all(p).await,
            Self::Postgres(p) => sqlx::query_as(sql).fetch_all(p).await,
        }
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(n, k, h)| ((n, k), h)).collect())
    }

    pub async fn env_applied_hashes(&self) -> CoreResult<BTreeMap<String, String>> {
        let sql = "SELECT key, hash FROM env_applied";
        let rows: Vec<(String, String)> = match self {
            Self::Sqlite(p) => sqlx::query_as(sql).fetch_all(p).await,
            Self::Postgres(p) => sqlx::query_as(sql).fetch_all(p).await,
        }
        .map_err(db_err)?;
        Ok(rows.into_iter().collect())
    }
}

impl Tx {
    async fn upsert_env_report(&mut self, node_id: &str, key: &str, hash: &str) -> CoreResult<()> {
        match self {
            Tx::Sqlite(t) => sqlx::query(&format!(
                "INSERT INTO env_report (node_id, key, hash, reported_at) VALUES (?1, ?2, ?3, {SQLITE_NOW_MS}) \
                 ON CONFLICT (node_id, key) DO UPDATE SET hash = excluded.hash, reported_at = excluded.reported_at"
            ))
            .bind(node_id)
            .bind(key)
            .bind(hash)
            .execute(&mut **t)
            .await
            .map(|_| ()),
            Tx::Postgres(t) => sqlx::query(&format!(
                "INSERT INTO env_report (node_id, key, hash, reported_at) VALUES ($1, $2, $3, {PG_NOW_MS}) \
                 ON CONFLICT (node_id, key) DO UPDATE SET hash = excluded.hash, reported_at = excluded.reported_at"
            ))
            .bind(node_id)
            .bind(key)
            .bind(hash)
            .execute(&mut **t)
            .await
            .map(|_| ()),
        }
        .map_err(db_err)
    }

    async fn delete_env_reports_except(&mut self, node_id: &str, keep: &[&str]) -> CoreResult<()> {
        let existing: Vec<String> = match self {
            Tx::Sqlite(t) => {
                sqlx::query_scalar("SELECT key FROM env_report WHERE node_id = ?1")
                    .bind(node_id)
                    .fetch_all(&mut **t)
                    .await
            }
            Tx::Postgres(t) => {
                sqlx::query_scalar("SELECT key FROM env_report WHERE node_id = $1")
                    .bind(node_id)
                    .fetch_all(&mut **t)
                    .await
            }
        }
        .map_err(db_err)?;
        for key in existing.iter().filter(|k| !keep.contains(&k.as_str())) {
            match self {
                Tx::Sqlite(t) => {
                    sqlx::query("DELETE FROM env_report WHERE node_id = ?1 AND key = ?2")
                        .bind(node_id)
                        .bind(key)
                        .execute(&mut **t)
                        .await
                        .map(|_| ())
                }
                Tx::Postgres(t) => {
                    sqlx::query("DELETE FROM env_report WHERE node_id = $1 AND key = $2")
                        .bind(node_id)
                        .bind(key)
                        .execute(&mut **t)
                        .await
                        .map(|_| ())
                }
            }
            .map_err(db_err)?;
        }
        Ok(())
    }

    /// Compare-and-set of `env_applied`: succeeds only if the stored hash is
    /// still `expected` (or absent when `expected` is None).
    pub async fn cas_env_applied(
        &mut self,
        key: &str,
        expected: Option<&str>,
        new: &str,
    ) -> CoreResult<bool> {
        let rows = match (self, expected) {
            (Tx::Sqlite(t), None) => sqlx::query(&format!(
                "INSERT INTO env_applied (key, hash, applied_at) VALUES (?1, ?2, {SQLITE_NOW_MS}) \
                 ON CONFLICT (key) DO NOTHING"
            ))
            .bind(key)
            .bind(new)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected()),
            (Tx::Sqlite(t), Some(old)) => sqlx::query(&format!(
                "UPDATE env_applied SET hash = ?2, applied_at = {SQLITE_NOW_MS} WHERE key = ?1 AND hash = ?3"
            ))
            .bind(key)
            .bind(new)
            .bind(old)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected()),
            (Tx::Postgres(t), None) => sqlx::query(&format!(
                "INSERT INTO env_applied (key, hash, applied_at) VALUES ($1, $2, {PG_NOW_MS}) \
                 ON CONFLICT (key) DO NOTHING"
            ))
            .bind(key)
            .bind(new)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected()),
            (Tx::Postgres(t), Some(old)) => sqlx::query(&format!(
                "UPDATE env_applied SET hash = $2, applied_at = {PG_NOW_MS} WHERE key = $1 AND hash = $3"
            ))
            .bind(key)
            .bind(new)
            .bind(old)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected()),
        }
        .map_err(db_err)?;
        Ok(rows == 1)
    }
}
