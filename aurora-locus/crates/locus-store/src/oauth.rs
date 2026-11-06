//! OAuth storage: replay windows now; requests, devices, clients and tokens
//! alongside the flows that use them.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, Tx, db_err};

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

// ---------------------------------------------------------------- requests

/// A pending authorization request.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RequestRow {
    pub id: String,
    pub did: Option<String>,
    pub device_id: Option<String>,
    pub client_id: String,
    pub client_auth: Option<String>,
    pub parameters: String,
    pub expires_at: i64,
    pub code: Option<String>,
}

/// A grant: the token row behind an access/refresh token pair.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TokenRow {
    pub id: i64,
    pub did: String,
    pub token_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub expires_at: i64,
    pub client_id: String,
    pub client_auth: String,
    pub device_id: Option<String>,
    pub parameters: String,
    pub details: Option<String>,
    pub code: Option<String>,
    pub current_refresh_token: Option<String>,
    pub scope: Option<String>,
}

const REQUEST_COLS: &str =
    "id, did, device_id, client_id, client_auth, parameters, expires_at, code";
const TOKEN_COLS: &str = "id, did, token_id, created_at, updated_at, expires_at, client_id, client_auth, \
     device_id, parameters, details, code, current_refresh_token, scope";

/// Builds and runs one query against either pool, binding `$args` in order.
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

impl Store {
    pub async fn oauth_create_request(&self, r: &RequestRow) -> CoreResult<()> {
        let q = sql(
            &format!(
                "INSERT INTO oauth_request ({REQUEST_COLS}) VALUES ({{1}}, {{2}}, {{3}}, {{4}}, {{5}}, {{6}}, {{7}}, {{8}})"
            ),
            self.is_postgres(),
        );
        exec!(
            self,
            q,
            &r.id,
            &r.did,
            &r.device_id,
            &r.client_id,
            &r.client_auth,
            &r.parameters,
            r.expires_at,
            &r.code
        )?;
        Ok(())
    }

    pub async fn oauth_read_request(&self, id: &str) -> CoreResult<Option<RequestRow>> {
        let q = sql(
            &format!("SELECT {REQUEST_COLS} FROM oauth_request WHERE id = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, id)
    }

    /// Writes a request's mutable fields back.
    pub async fn oauth_update_request(&self, r: &RequestRow) -> CoreResult<()> {
        let q = sql(
            "UPDATE oauth_request SET did = {2}, device_id = {3}, parameters = {4}, expires_at = {5}, code = {6} WHERE id = {1}",
            self.is_postgres(),
        );
        exec!(
            self,
            q,
            &r.id,
            &r.did,
            &r.device_id,
            &r.parameters,
            r.expires_at,
            &r.code
        )?;
        Ok(())
    }

    pub async fn oauth_delete_request(&self, id: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM oauth_request WHERE id = {1}",
            self.is_postgres(),
        );
        exec!(self, q, id)?;
        Ok(())
    }

    /// Takes the request a code belongs to (deleted, so a code works once).
    pub async fn oauth_consume_code(&self, code: &str) -> CoreResult<Option<RequestRow>> {
        let q = sql(
            &format!("DELETE FROM oauth_request WHERE code = {{1}} RETURNING {REQUEST_COLS}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, code)
    }

    // ------------------------------------------------------------ tokens

    pub async fn oauth_create_token(&self, t: &TokenRow) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO oauth_token (did, token_id, created_at, updated_at, expires_at, client_id, client_auth, \
             device_id, parameters, details, code, current_refresh_token, scope) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {7}, {8}, {9}, {10}, {11}, {12}, {13})",
            self.is_postgres(),
        );
        exec!(
            self,
            q,
            &t.did,
            &t.token_id,
            t.created_at,
            t.updated_at,
            t.expires_at,
            &t.client_id,
            &t.client_auth,
            &t.device_id,
            &t.parameters,
            &t.details,
            &t.code,
            &t.current_refresh_token,
            &t.scope
        )?;
        Ok(())
    }

    pub async fn oauth_token(&self, token_id: &str) -> CoreResult<Option<TokenRow>> {
        let q = sql(
            &format!("SELECT {TOKEN_COLS} FROM oauth_token WHERE token_id = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, token_id)
    }

    pub async fn oauth_token_by_code(&self, code: &str) -> CoreResult<Option<TokenRow>> {
        let q = sql(
            &format!("SELECT {TOKEN_COLS} FROM oauth_token WHERE code = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, code)
    }

    /// The grant a refresh token belongs to, current or already spent.
    pub async fn oauth_token_by_refresh(&self, refresh: &str) -> CoreResult<Option<TokenRow>> {
        let q = sql(
            &format!(
                "SELECT {TOKEN_COLS} FROM oauth_token WHERE current_refresh_token = {{1}} \
                 OR id = (SELECT token_id FROM oauth_used_refresh WHERE refresh_token = {{1}})"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, refresh)
    }

    /// Rotates a grant to a new token id and refresh token; the old refresh
    /// token is recorded as spent (so a replay is recognized).
    #[allow(clippy::too_many_arguments)]
    pub async fn oauth_rotate_token(
        &self,
        id: i64,
        new_token_id: &str,
        new_refresh: &str,
        updated_at: i64,
        expires_at: i64,
        client_auth: &str,
        scope: Option<&str>,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let pg = matches!(tx, crate::Tx::Postgres(_));
        let spent = sql(
            "INSERT INTO oauth_used_refresh (refresh_token, token_id) \
             SELECT current_refresh_token, id FROM oauth_token WHERE id = {1} AND current_refresh_token IS NOT NULL",
            pg,
        );
        let upd = sql(
            "UPDATE oauth_token SET token_id = {2}, current_refresh_token = {3}, updated_at = {4}, expires_at = {5}, \
             client_auth = {6}, scope = {7} WHERE id = {1}",
            pg,
        );
        match &mut tx {
            crate::Tx::Sqlite(t) => {
                sqlx::query(&spent)
                    .bind(id)
                    .execute(&mut **t)
                    .await
                    .map_err(db_err)?;
                sqlx::query(&upd)
                    .bind(id)
                    .bind(new_token_id)
                    .bind(new_refresh)
                    .bind(updated_at)
                    .bind(expires_at)
                    .bind(client_auth)
                    .bind(scope)
                    .execute(&mut **t)
                    .await
                    .map_err(db_err)?;
            }
            crate::Tx::Postgres(t) => {
                sqlx::query(&spent)
                    .bind(id)
                    .execute(&mut **t)
                    .await
                    .map_err(db_err)?;
                sqlx::query(&upd)
                    .bind(id)
                    .bind(new_token_id)
                    .bind(new_refresh)
                    .bind(updated_at)
                    .bind(expires_at)
                    .bind(client_auth)
                    .bind(scope)
                    .execute(&mut **t)
                    .await
                    .map_err(db_err)?;
            }
        }
        tx.commit().await
    }

    /// Deletes a grant (and its spent refresh tokens).
    /// Ends a grant. On an entryway, the account's locus shard is told to
    /// refuse its tokens (`sid` = the grant's id), in the same transaction.
    pub async fn oauth_delete_token(&self, id: i64) -> CoreResult<()> {
        let pg = self.is_postgres();
        let mut tx = self.begin().await?;
        let q = sql("SELECT did FROM oauth_token WHERE id = {1}", pg);
        let did: Option<(String,)> = match &mut tx {
            Tx::Sqlite(t) => sqlx::query_as(&q).bind(id).fetch_optional(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(&q).bind(id).fetch_optional(&mut **t).await,
        }
        .map_err(db_err)?;
        for q in [
            "DELETE FROM oauth_used_refresh WHERE token_id = {1}",
            "DELETE FROM oauth_token WHERE id = {1}",
        ] {
            let q = sql(q, pg);
            match &mut tx {
                Tx::Sqlite(t) => sqlx::query(&q).bind(id).execute(&mut **t).await.map(|_| ()),
                Tx::Postgres(t) => sqlx::query(&q).bind(id).execute(&mut **t).await.map(|_| ()),
            }
            .map_err(db_err)?;
        }
        if let Some((did,)) = did {
            tx.queue_revocation(&did, Some(&[id.to_string()])).await?;
        }
        tx.commit().await
    }

    /// Every grant of an account (password reset and takedown revoke them
    /// all **[deviation]**, the reference's password reset doesn't).
    pub async fn oauth_delete_account_tokens(&self, did: &str) -> CoreResult<u64> {
        let q1 = sql(
            "DELETE FROM oauth_used_refresh WHERE token_id IN (SELECT id FROM oauth_token WHERE did = {1})",
            self.is_postgres(),
        );
        exec!(self, q1, did)?;
        let q2 = sql(
            "DELETE FROM oauth_token WHERE did = {1}",
            self.is_postgres(),
        );
        exec!(self, q2, did)
    }

    pub async fn oauth_account_tokens(&self, did: &str) -> CoreResult<Vec<TokenRow>> {
        let q = sql(
            &format!("SELECT {TOKEN_COLS} FROM oauth_token WHERE did = {{1}} ORDER BY id"),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, did)
    }

    // ------------------------------------------------------------ devices, accounts, clients

    /// Creates or refreshes a device row.
    pub async fn oauth_upsert_device(
        &self,
        id: &str,
        session_id: &str,
        user_agent: Option<&str>,
        ip: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO oauth_device (id, session_id, user_agent, ip_address, last_seen_at) VALUES ({1}, {2}, {3}, {4}, {now}) \
             ON CONFLICT (id) DO UPDATE SET session_id = excluded.session_id, user_agent = excluded.user_agent, \
             ip_address = excluded.ip_address, last_seen_at = excluded.last_seen_at",
            self.is_postgres(),
        );
        exec!(self, q, id, session_id, user_agent, ip)?;
        Ok(())
    }

    /// A device's session id.
    pub async fn oauth_device_session(&self, id: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT session_id FROM oauth_device WHERE id = {1}",
            self.is_postgres(),
        );
        let r: Option<(String,)> = run!(self, q, fetch_optional, id)?;
        Ok(r.map(|r| r.0))
    }

    /// Records (or refreshes) that an account signed in on a device.
    pub async fn oauth_touch_account_device(&self, did: &str, device_id: &str) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO oauth_account_device (did, device_id, created_at, updated_at) VALUES ({1}, {2}, {now}, {now}) \
             ON CONFLICT (did, device_id) DO UPDATE SET updated_at = excluded.updated_at",
            self.is_postgres(),
        );
        exec!(self, q, did, device_id)?;
        Ok(())
    }

    /// The accounts signed in on a device: (did, signed in at, updated at).
    pub async fn oauth_device_accounts(
        &self,
        device_id: &str,
    ) -> CoreResult<Vec<(String, i64, i64)>> {
        let q = sql(
            "SELECT did, created_at, updated_at FROM oauth_account_device WHERE device_id = {1} ORDER BY updated_at DESC",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, device_id)
    }

    pub async fn oauth_remove_account_device(&self, did: &str, device_id: &str) -> CoreResult<()> {
        let q = sql(
            "DELETE FROM oauth_account_device WHERE did = {1} AND device_id = {2}",
            self.is_postgres(),
        );
        exec!(self, q, did, device_id)?;
        Ok(())
    }

    /// What the account approved for a client (`{"authorizedScopes": [...]}`).
    pub async fn oauth_authorized_client(
        &self,
        did: &str,
        client_id: &str,
    ) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT data FROM oauth_authorized_client WHERE did = {1} AND client_id = {2}",
            self.is_postgres(),
        );
        let r: Option<(String,)> = run!(self, q, fetch_optional, did, client_id)?;
        Ok(r.map(|r| r.0))
    }

    pub async fn oauth_set_authorized_client(
        &self,
        did: &str,
        client_id: &str,
        data: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO oauth_authorized_client (did, client_id, created_at, updated_at, data) VALUES ({1}, {2}, {now}, {now}, {3}) \
             ON CONFLICT (did, client_id) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data",
            self.is_postgres(),
        );
        exec!(self, q, did, client_id, data)?;
        Ok(())
    }
}

impl Store {
    /// A cached lexicon: (updated_at ms, document JSON when one was ever
    /// resolved).
    pub async fn oauth_lexicon(&self, nsid: &str) -> CoreResult<Option<(i64, Option<String>)>> {
        let q = sql(
            "SELECT updated_at, lexicon FROM oauth_lexicon WHERE nsid = {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, nsid)
    }

    /// Records a resolution attempt; a failed one (`lexicon` None) keeps the
    /// last good document.
    pub async fn oauth_put_lexicon(
        &self,
        nsid: &str,
        uri: Option<&str>,
        lexicon: Option<&str>,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO oauth_lexicon (nsid, created_at, updated_at, last_succeeded_at, uri, lexicon) \
             VALUES ({1}, {now}, {now}, CASE WHEN {3} IS NULL THEN NULL ELSE {now} END, {2}, {3}) \
             ON CONFLICT (nsid) DO UPDATE SET updated_at = excluded.updated_at, \
             last_succeeded_at = COALESCE(excluded.last_succeeded_at, oauth_lexicon.last_succeeded_at), \
             uri = COALESCE(excluded.uri, oauth_lexicon.uri), \
             lexicon = COALESCE(excluded.lexicon, oauth_lexicon.lexicon)",
            self.is_postgres(),
        );
        exec!(self, q, nsid, uri, lexicon)?;
        Ok(())
    }
}

/// A device row: (session id, user agent, IP, last seen ms).
pub type DeviceRow = (String, Option<String>, String, i64);

impl Store {
    pub async fn oauth_device(&self, id: &str) -> CoreResult<Option<DeviceRow>> {
        let q = sql(
            "SELECT session_id, user_agent, ip_address, last_seen_at FROM oauth_device WHERE id = {1}",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, id)
    }

    pub async fn oauth_delete_device(&self, id: &str) -> CoreResult<()> {
        for q in [
            "DELETE FROM oauth_account_device WHERE device_id = {1}",
            "DELETE FROM oauth_device WHERE id = {1}",
        ] {
            let q = sql(q, self.is_postgres());
            exec!(self, q, id)?;
        }
        Ok(())
    }
}
