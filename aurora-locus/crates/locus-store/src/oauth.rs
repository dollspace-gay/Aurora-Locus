//! OAuth storage: replay windows now; requests, devices, clients and tokens
//! alongside the flows that use them.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, db_err};

impl Store {
    /// Records `value` in `namespace` for `ttl_ms`; false if it was already
    /// recorded and the window hasn't ended (a replay).
    pub async fn oauth_unique(
        &self,
        namespace: &str,
        value: &str,
        ttl_ms: i64,
    ) -> CoreResult<bool> {
        let q = sql(
            "INSERT INTO oauth_replay (namespace, value, expires_at) VALUES ({1}, {2}, {now} + {3}) \
             ON CONFLICT (namespace, value) DO UPDATE SET expires_at = excluded.expires_at \
             WHERE oauth_replay.expires_at <= {now} RETURNING 1",
            self.is_postgres(),
        );
        let row: Option<i32> = match self {
            Store::Sqlite(p) => {
                sqlx::query_scalar(&q)
                    .bind(namespace)
                    .bind(value)
                    .bind(ttl_ms)
                    .fetch_optional(p)
                    .await
            }
            Store::Postgres(p) => {
                sqlx::query_scalar(&q)
                    .bind(namespace)
                    .bind(value)
                    .bind(ttl_ms)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)?;
        Ok(row.is_some())
    }

    /// Drops ended replay windows and expired authorization requests.
    pub async fn prune_oauth(&self, limit: i64) -> CoreResult<u64> {
        let mut total = 0;
        for table in ["oauth_replay", "oauth_request"] {
            let key = if table == "oauth_replay" {
                "namespace || ' ' || value"
            } else {
                "id"
            };
            let q = sql(
                &format!(
                    "DELETE FROM {table} WHERE {key} IN (SELECT {key} FROM {table} WHERE expires_at <= {{now}} LIMIT {{1}})"
                ),
                self.is_postgres(),
            );
            total += match self {
                Store::Sqlite(p) => sqlx::query(&q)
                    .bind(limit)
                    .execute(p)
                    .await
                    .map(|r| r.rows_affected()),
                Store::Postgres(p) => sqlx::query(&q)
                    .bind(limit)
                    .execute(p)
                    .await
                    .map(|r| r.rows_affected()),
            }
            .map_err(db_err)?;
        }
        Ok(total)
    }
}
