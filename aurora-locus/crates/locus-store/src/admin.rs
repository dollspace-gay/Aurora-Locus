//! Queries the admin API needs: takedowns of records and blobs, invite
//! administration, account search.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::invites::InviteCode;
use crate::{Store, db_err};

macro_rules! exec {
    ($store:expr, $q:expr, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
            Store::Postgres(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
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

macro_rules! rows {
    ($store:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
            Store::Postgres(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
        }
        .map_err(db_err)
    };
}

impl Store {
    /// Sets (or, with `None`, clears) a record's takedown, by path whatever
    /// its CID. False if no record is there.
    pub async fn set_record_takedown(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
        takedown_ref: Option<&str>,
    ) -> CoreResult<bool> {
        let q = sql(
            "UPDATE record SET takedown_ref = {4} WHERE did = {1} AND collection = {2} AND rkey = {3}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, did, collection, rkey, takedown_ref)? == 1)
    }

    /// Sets (or clears) a blob's takedown. False if there's no such blob.
    pub async fn set_blob_takedown(
        &self,
        did: &str,
        cid: &str,
        takedown_ref: Option<&str>,
    ) -> CoreResult<bool> {
        let q = sql(
            "UPDATE blob SET takedown_ref = {3} WHERE did = {1} AND cid = {2}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, did, cid, takedown_ref)? == 1)
    }

    pub async fn set_account_invites_disabled(&self, did: &str, disabled: bool) -> CoreResult<()> {
        let q = sql(
            "UPDATE account SET invites_disabled = {2} WHERE did = {1}",
            self.is_postgres(),
        );
        exec!(self, q, did, disabled)?;
        Ok(())
    }

    /// Disables codes, and every code made for the given accounts.
    pub async fn disable_invite_codes(
        &self,
        codes: &[String],
        accounts: &[String],
    ) -> CoreResult<()> {
        let by_code = sql(
            "UPDATE invite_code SET disabled = {2} WHERE code = {1}",
            self.is_postgres(),
        );
        for c in codes {
            exec!(self, by_code, c, true)?;
        }
        let by_account = sql(
            "UPDATE invite_code SET disabled = {2} WHERE for_account = {1}",
            self.is_postgres(),
        );
        for a in accounts {
            exec!(self, by_account, a, true)?;
        }
        Ok(())
    }

    /// The code an account signed up with.
    pub async fn invited_by(&self, did: &str) -> CoreResult<Option<String>> {
        let q = sql(
            "SELECT code FROM invite_code_use WHERE used_by = {1} ORDER BY used_at LIMIT 1",
            self.is_postgres(),
        );
        scalar!(self, q, fetch_optional, did)
    }

    /// One invite code with its uses.
    pub async fn invite_code(&self, code: &str) -> CoreResult<Option<InviteCode>> {
        Ok(self.invite_codes_where(code).await?.into_iter().next())
    }

    /// Every invite code, newest first (`recent`) or most used first
    /// (`usage`), after `cursor` (the last code of the previous page).
    pub async fn all_invite_codes(
        &self,
        usage: bool,
        cursor: Option<&str>,
        limit: i64,
    ) -> CoreResult<Vec<InviteCode>> {
        let order = if usage {
            "used DESC, c.code DESC"
        } else {
            "c.created_at DESC, c.code DESC"
        };
        let cursor_row = match cursor {
            Some(c) => self.invite_code(c).await?,
            None => None,
        };
        let filter = match (&cursor_row, usage) {
            (None, _) => String::new(),
            (Some(_), false) => "WHERE (c.created_at < {2} OR (c.created_at = {2} AND c.code < {3}))".into(),
            (Some(_), true) => "WHERE ((SELECT COUNT(*) FROM invite_code_use u WHERE u.code = c.code) < {2} \
                 OR ((SELECT COUNT(*) FROM invite_code_use u WHERE u.code = c.code) = {2} AND c.code < {3}))"
                .into(),
        };
        let q = sql(
            &format!(
                "SELECT c.code, (SELECT COUNT(*) FROM invite_code_use u WHERE u.code = c.code) AS used \
                 FROM invite_code c {filter} ORDER BY {order} LIMIT {{1}}"
            ),
            self.is_postgres(),
        );
        let pivot = cursor_row.as_ref().map_or(0, |c| {
            if usage {
                i64::try_from(c.uses.len()).unwrap_or(0)
            } else {
                c.created_at
            }
        });
        let pivot_code = cursor_row
            .as_ref()
            .map(|c| c.code.clone())
            .unwrap_or_default();
        let picked: Vec<(String, i64)> = if cursor_row.is_some() {
            rows!(self, q, fetch_all, limit, pivot, &pivot_code)?
        } else {
            rows!(self, q, fetch_all, limit)?
        };
        let mut out = Vec::with_capacity(picked.len());
        for (code, _) in picked {
            if let Some(c) = self.invite_code(&code).await? {
                out.push(c);
            }
        }
        Ok(out)
    }

    async fn invite_codes_where(&self, code: &str) -> CoreResult<Vec<InviteCode>> {
        let q = sql(
            "SELECT code, CAST(available_uses AS BIGINT), disabled, for_account, created_by, created_at \
             FROM invite_code WHERE code = {1}",
            self.is_postgres(),
        );
        let codes: Vec<(String, i64, bool, String, String, i64)> = rows!(self, q, fetch_all, code)?;
        let uq = sql(
            "SELECT used_by, used_at FROM invite_code_use WHERE code = {1} ORDER BY used_at",
            self.is_postgres(),
        );
        let mut out = Vec::new();
        for (code, remaining, disabled, for_account, created_by, created_at) in codes {
            let uses: Vec<(String, i64)> = rows!(self, uq, fetch_all, &code)?;
            out.push(InviteCode {
                available: remaining + i64::try_from(uses.len()).unwrap_or(0),
                code,
                disabled,
                for_account,
                created_by,
                created_at,
                uses,
            });
        }
        Ok(out)
    }

    /// Accounts whose handle or email starts with `query` (case-blind),
    /// after `cursor` (a DID), by DID.
    pub async fn search_accounts(
        &self,
        query: &str,
        cursor: Option<&str>,
        limit: i64,
    ) -> CoreResult<Vec<String>> {
        let pattern = format!(
            "{}%",
            query
                .to_ascii_lowercase()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let q = sql(
            "SELECT did FROM account WHERE status <> 'deleted' AND (handle LIKE {1} ESCAPE '\\' \
             OR email LIKE {1} ESCAPE '\\' OR did = {4}) AND did > {2} ORDER BY did LIMIT {3}",
            self.is_postgres(),
        );
        scalar!(
            self,
            q,
            fetch_all,
            &pattern,
            cursor.unwrap_or(""),
            limit,
            query
        )
    }
}
