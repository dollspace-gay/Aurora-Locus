//! Operators, their API tokens, and the audit log they write.

use locus_core::CoreResult;

use crate::blob::sql;
use crate::{Store, db_err};

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

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct OperatorRow {
    pub username: String,
    pub password_hash: String,
    pub totp_secret: Option<String>,
    pub role: String,
    pub created_at: i64,
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TokenRow {
    pub id: String,
    pub operator: String,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub expires_at: Option<i64>,
}

/// One audit-log entry.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub detail_json: Option<String>,
}

const OP_COLS: &str = "username, password_hash, totp_secret, role, created_at, disabled";

impl Store {
    /// Adds an operator; false if the username is taken.
    pub async fn create_operator(
        &self,
        username: &str,
        password_hash: &str,
        role: &str,
    ) -> CoreResult<bool> {
        let q = sql(
            "INSERT INTO operator (username, password_hash, role, created_at) VALUES ({1}, {2}, {3}, {now}) \
             ON CONFLICT (username) DO NOTHING",
            self.is_postgres(),
        );
        Ok(exec!(self, q, username, password_hash, role)? == 1)
    }

    pub async fn operator(&self, username: &str) -> CoreResult<Option<OperatorRow>> {
        let q = sql(
            &format!("SELECT {OP_COLS} FROM operator WHERE username = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, username)
    }

    pub async fn operators(&self) -> CoreResult<Vec<OperatorRow>> {
        let q = format!("SELECT {OP_COLS} FROM operator ORDER BY username");
        run!(self, q, fetch_all,)
    }

    /// Changes an operator's role, password hash or disabled flag (each
    /// only when given).
    pub async fn update_operator(
        &self,
        username: &str,
        role: Option<&str>,
        password_hash: Option<&str>,
        disabled: Option<bool>,
    ) -> CoreResult<bool> {
        let q = sql(
            "UPDATE operator SET role = COALESCE({2}, role), password_hash = COALESCE({3}, password_hash), \
             disabled = COALESCE({4}, disabled) WHERE username = {1}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, username, role, password_hash, disabled)? == 1)
    }

    /// How many enabled owners there are (the last one can't be removed).
    pub async fn count_owners(&self) -> CoreResult<i64> {
        let q = sql(
            "SELECT COUNT(*) FROM operator WHERE role = 'owner' AND disabled = {1}",
            self.is_postgres(),
        );
        match self {
            Store::Sqlite(p) => sqlx::query_scalar(&q).bind(false).fetch_one(p).await,
            Store::Postgres(p) => sqlx::query_scalar(&q).bind(false).fetch_one(p).await,
        }
        .map_err(db_err)
    }

    pub async fn create_operator_token(
        &self,
        id: &str,
        token_hash: &str,
        operator: &str,
        name: &str,
        expires_at: Option<i64>,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO operator_token (id, token_hash, operator, name, created_at, expires_at) \
             VALUES ({1}, {2}, {3}, {4}, {now}, {5})",
            self.is_postgres(),
        );
        exec!(self, q, id, token_hash, operator, name, expires_at)?;
        Ok(())
    }

    /// The enabled operator a live token belongs to, marking the token used.
    pub async fn operator_by_token(&self, token_hash: &str) -> CoreResult<Option<OperatorRow>> {
        let q = sql(
            "SELECT o.username, o.password_hash, o.totp_secret, o.role, o.created_at, o.disabled \
             FROM operator_token t JOIN operator o ON o.username = t.operator \
             WHERE t.token_hash = {1} AND (t.expires_at IS NULL OR t.expires_at > {now}) AND o.disabled = {2}",
            self.is_postgres(),
        );
        let row: Option<OperatorRow> = run!(self, q, fetch_optional, token_hash, false)?;
        if row.is_some() {
            let t = sql(
                "UPDATE operator_token SET last_used_at = {now} WHERE token_hash = {1}",
                self.is_postgres(),
            );
            exec!(self, t, token_hash)?;
        }
        Ok(row)
    }

    pub async fn operator_tokens(&self, operator: &str) -> CoreResult<Vec<TokenRow>> {
        let q = sql(
            "SELECT id, operator, name, created_at, last_used_at, expires_at FROM operator_token \
             WHERE operator = {1} ORDER BY created_at",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, operator)
    }

    pub async fn revoke_operator_token(&self, id: &str) -> CoreResult<bool> {
        let q = sql(
            "DELETE FROM operator_token WHERE id = {1}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, id)? == 1)
    }

    /// Appends an audit-log row outside any other transaction.
    pub async fn audit(
        &self,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: Option<&serde_json::Value>,
    ) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        tx.audit(actor, action, target, detail).await?;
        tx.commit().await
    }

    /// Audit-log entries, newest first, before `before` (an id) if given.
    pub async fn audit_entries(
        &self,
        before: Option<i64>,
        limit: i64,
    ) -> CoreResult<Vec<AuditEntry>> {
        let q = sql(
            "SELECT id, at, actor, action, target, detail_json FROM audit_log \
             WHERE ({1} IS NULL OR id < {1}) ORDER BY id DESC LIMIT {2}",
            self.is_postgres(),
        );
        run!(self, q, fetch_all, before, limit)
    }
}
