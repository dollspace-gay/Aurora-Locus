//! Accounts, legacy sessions, refresh tokens, app passwords and invites.
//!
//! Handles and emails are lowercased by callers before they get here.
//! Uniqueness is enforced by unique indexes across every account; a clash
//! comes back as [`CoreError::Conflict`] naming the field, never as a
//! storage error.

use locus_core::{CoreError, CoreResult};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// Account status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Active,
    Deactivated,
    Takendown,
    Suspended,
    Deleted,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deactivated => "deactivated",
            Self::Takendown => "takendown",
            Self::Suspended => "suspended",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(s: &str) -> CoreResult<Self> {
        Ok(match s {
            "active" => Self::Active,
            "deactivated" => Self::Deactivated,
            "takendown" => Self::Takendown,
            "suspended" => Self::Suspended,
            "deleted" => Self::Deleted,
            other => {
                return Err(CoreError::Storage(format!(
                    "unknown account status {other:?}"
                )));
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub did: String,
    pub handle: String,
    pub email: Option<String>,
    pub email_confirmed_at: Option<i64>,
    pub password_hash: Option<String>,
    pub created_at: i64,
    pub status: Status,
    pub status_ref: Option<String>,
    pub deactivated_at: Option<i64>,
    pub delete_after: Option<i64>,
    pub invites_disabled: bool,
    pub did_method: String,
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    did: String,
    handle: String,
    email: Option<String>,
    email_confirmed_at: Option<i64>,
    password_hash: Option<String>,
    created_at: i64,
    status: String,
    status_ref: Option<String>,
    deactivated_at: Option<i64>,
    delete_after: Option<i64>,
    invites_disabled: bool,
    did_method: String,
}

impl TryFrom<AccountRow> for Account {
    type Error = CoreError;
    fn try_from(r: AccountRow) -> CoreResult<Self> {
        Ok(Self {
            did: r.did,
            handle: r.handle,
            email: r.email,
            email_confirmed_at: r.email_confirmed_at,
            password_hash: r.password_hash,
            created_at: r.created_at,
            status: Status::parse(&r.status)?,
            status_ref: r.status_ref,
            deactivated_at: r.deactivated_at,
            delete_after: r.delete_after,
            invites_disabled: r.invites_disabled,
            did_method: r.did_method,
        })
    }
}

const ACCOUNT_COLS: &str = "did, handle, email, email_confirmed_at, password_hash, created_at, status, \
     status_ref, deactivated_at, delete_after, invites_disabled, did_method";

/// A new account row.
#[derive(Debug, Clone)]
pub struct NewAccount<'a> {
    pub did: &'a str,
    pub handle: &'a str,
    pub email: Option<&'a str>,
    pub password_hash: Option<&'a str>,
    pub status: Status,
    pub did_method: &'a str,
}

/// A session row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub did: String,
    pub kind: String,
    pub scope: String,
    pub app_password_name: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

/// A refresh token row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RefreshToken {
    pub id: String,
    pub session_id: String,
    pub expires_at: i64,
    pub next_id: Option<String>,
}

/// An app password row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AppPassword {
    pub did: String,
    pub name: String,
    pub lookup_hash: String,
    pub hash: String,
    pub privileged: bool,
    pub created_at: i64,
}

fn unique_violation(e: &sqlx::Error) -> Option<String> {
    let sqlx::Error::Database(db) = e else {
        return None;
    };
    let is_unique = db.code().as_deref() == Some("23505") // Postgres unique_violation
        || db.message().contains("UNIQUE constraint failed");
    is_unique.then(|| db.message().to_owned())
}

fn account_conflict(e: sqlx::Error) -> CoreError {
    match unique_violation(&e) {
        Some(m) if m.contains("handle") => CoreError::Conflict("Handle already taken".into()),
        Some(m) if m.contains("email") => CoreError::Conflict("Email already taken".into()),
        Some(_) => CoreError::Conflict("Account already exists".into()),
        None => db_err(e),
    }
}

macro_rules! by_backend {
    ($self:expr, $sqlite:expr, $pg:expr, |$q:ident, $exec:ident| $body:expr) => {
        match $self {
            Tx::Sqlite(t) => {
                let $q = $sqlite;
                let $exec = &mut **t;
                $body
            }
            Tx::Postgres(t) => {
                let $q = $pg;
                let $exec = &mut **t;
                $body
            }
        }
    };
}

impl Tx {
    pub async fn insert_account(&mut self, a: &NewAccount<'_>) -> CoreResult<()> {
        let sqlite = format!(
            "INSERT INTO account (did, handle, email, password_hash, created_at, status, did_method) \
             VALUES (?1, ?2, ?3, ?4, {SQLITE_NOW_MS}, ?5, ?6)"
        );
        let pg = format!(
            "INSERT INTO account (did, handle, email, password_hash, created_at, status, did_method) \
             VALUES ($1, $2, $3, $4, {PG_NOW_MS}, $5, $6)"
        );
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(&q)
            .bind(a.did)
            .bind(a.handle)
            .bind(a.email)
            .bind(a.password_hash)
            .bind(a.status.as_str())
            .bind(a.did_method)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(account_conflict)
    }

    pub async fn set_account_status(
        &mut self,
        did: &str,
        status: Status,
        status_ref: Option<&str>,
    ) -> CoreResult<()> {
        let sqlite = format!(
            "UPDATE account SET status = ?2, status_ref = ?3, \
             deactivated_at = CASE WHEN ?2 = 'deactivated' THEN {SQLITE_NOW_MS} ELSE NULL END WHERE did = ?1"
        );
        let pg = format!(
            "UPDATE account SET status = $2, status_ref = $3, \
             deactivated_at = CASE WHEN $2 = 'deactivated' THEN {PG_NOW_MS} ELSE NULL END WHERE did = $1"
        );
        let rows = by_backend!(self, sqlite, pg, |q, ex| sqlx::query(&q)
            .bind(did)
            .bind(status.as_str())
            .bind(status_ref)
            .execute(ex)
            .await
            .map(|r| r.rows_affected()))
        .map_err(db_err)?;
        if rows == 1 {
            Ok(())
        } else {
            Err(CoreError::NotFound(format!("account {did}")))
        }
    }

    pub async fn insert_session(&mut self, s: &Session) -> CoreResult<()> {
        let sqlite = "INSERT INTO session (id, did, kind, scope, app_password_name, created_at, expires_at) \
                      VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)";
        let pg = "INSERT INTO session (id, did, kind, scope, app_password_name, created_at, expires_at) \
                  VALUES ($1, $2, $3, $4, $5, $6, $7)";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(q)
            .bind(&s.id)
            .bind(&s.did)
            .bind(&s.kind)
            .bind(&s.scope)
            .bind(&s.app_password_name)
            .bind(s.created_at)
            .bind(s.expires_at)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(db_err)
    }

    pub async fn insert_refresh_token(&mut self, r: &RefreshToken) -> CoreResult<()> {
        let sqlite = "INSERT INTO refresh_token (id, session_id, expires_at, next_id) VALUES (?1, ?2, ?3, ?4)";
        let pg = "INSERT INTO refresh_token (id, session_id, expires_at, next_id) VALUES ($1, $2, $3, $4)";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(q)
            .bind(&r.id)
            .bind(&r.session_id)
            .bind(r.expires_at)
            .bind(&r.next_id)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(db_err)
    }

    /// The refresh token row, locked for this transaction (Postgres).
    pub async fn refresh_token_for_update(&mut self, id: &str) -> CoreResult<Option<RefreshToken>> {
        let sqlite = "SELECT id, session_id, expires_at, next_id FROM refresh_token WHERE id = ?1";
        let pg = "SELECT id, session_id, expires_at, next_id FROM refresh_token WHERE id = $1 FOR UPDATE";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query_as(q)
            .bind(id)
            .fetch_optional(ex)
            .await)
        .map_err(db_err)
    }

    /// Marks a refresh token used: records its successor and shortens its
    /// life to the replay grace.
    pub async fn mark_refresh_used(
        &mut self,
        id: &str,
        next_id: &str,
        expires_at: i64,
    ) -> CoreResult<()> {
        let sqlite =
            "UPDATE refresh_token SET next_id = ?2, expires_at = MIN(expires_at, ?3) WHERE id = ?1";
        let pg = "UPDATE refresh_token SET next_id = $2, expires_at = LEAST(expires_at, $3) WHERE id = $1";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(q)
            .bind(id)
            .bind(next_id)
            .bind(expires_at)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(db_err)
    }

    pub async fn session_for_update(&mut self, id: &str) -> CoreResult<Option<Session>> {
        let sqlite = "SELECT id, did, kind, scope, app_password_name, created_at, expires_at, revoked_at \
                      FROM session WHERE id = ?1";
        let pg = "SELECT id, did, kind, scope, app_password_name, created_at, expires_at, revoked_at \
                  FROM session WHERE id = $1 FOR UPDATE";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query_as(q)
            .bind(id)
            .fetch_optional(ex)
            .await)
        .map_err(db_err)
    }

    /// Revokes sessions: one by id, every session of an account, or every
    /// session made with one app password. Returns how many were revoked.
    pub async fn revoke_sessions(
        &mut self,
        did: &str,
        filter: SessionFilter<'_>,
    ) -> CoreResult<u64> {
        let (extra, arg) = match filter {
            SessionFilter::One(id) => (Some("id"), Some(id)),
            SessionFilter::All => (None, None),
            SessionFilter::AppPassword(name) => (Some("app_password_name"), Some(name)),
        };
        let cond_s = extra.map(|c| format!(" AND {c} = ?2")).unwrap_or_default();
        let cond_p = extra.map(|c| format!(" AND {c} = $2")).unwrap_or_default();
        let sqlite = format!(
            "UPDATE session SET revoked_at = {SQLITE_NOW_MS} WHERE did = ?1 AND revoked_at IS NULL{cond_s}"
        );
        let pg = format!(
            "UPDATE session SET revoked_at = {PG_NOW_MS} WHERE did = $1 AND revoked_at IS NULL{cond_p}"
        );
        by_backend!(self, sqlite, pg, |q, ex| {
            let mut query = sqlx::query(&q).bind(did);
            if let Some(a) = arg {
                query = query.bind(a);
            }
            query.execute(ex).await.map(|r| r.rows_affected())
        })
        .map_err(db_err)
    }

    pub async fn insert_app_password(&mut self, p: &AppPassword) -> CoreResult<()> {
        let sqlite = "INSERT INTO app_password (did, name, lookup_hash, hash, privileged, created_at) \
                      VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
        let pg = "INSERT INTO app_password (did, name, lookup_hash, hash, privileged, created_at) \
                  VALUES ($1, $2, $3, $4, $5, $6)";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(q)
            .bind(&p.did)
            .bind(&p.name)
            .bind(&p.lookup_hash)
            .bind(&p.hash)
            .bind(p.privileged)
            .bind(p.created_at)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(|e| match unique_violation(&e) {
            Some(_) => CoreError::Conflict("An app password with this name already exists".into()),
            None => db_err(e),
        })
    }

    pub async fn delete_app_password(&mut self, did: &str, name: &str) -> CoreResult<bool> {
        let sqlite = "DELETE FROM app_password WHERE did = ?1 AND name = ?2";
        let pg = "DELETE FROM app_password WHERE did = $1 AND name = $2";
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(q)
            .bind(did)
            .bind(name)
            .execute(ex)
            .await
            .map(|r| r.rows_affected() == 1))
        .map_err(db_err)
    }

    /// Takes one use of an invite code for `used_by`. Fails with
    /// [`CoreError::Invalid`] if the code is unknown, disabled or used up.
    pub async fn use_invite_code(&mut self, code: &str, used_by: &str) -> CoreResult<()> {
        let sqlite = "UPDATE invite_code SET available_uses = available_uses - 1 \
                      WHERE code = ?1 AND NOT disabled AND available_uses > 0";
        let pg = "UPDATE invite_code SET available_uses = available_uses - 1 \
                  WHERE code = $1 AND NOT disabled AND available_uses > 0";
        let rows = by_backend!(self, sqlite, pg, |q, ex| sqlx::query(q)
            .bind(code)
            .execute(ex)
            .await
            .map(|r| r.rows_affected()))
        .map_err(db_err)?;
        if rows != 1 {
            return Err(CoreError::Invalid(
                "Provided invite code not available".into(),
            ));
        }
        let sqlite = format!(
            "INSERT INTO invite_code_use (code, used_by, used_at) VALUES (?1, ?2, {SQLITE_NOW_MS})"
        );
        let pg = format!(
            "INSERT INTO invite_code_use (code, used_by, used_at) VALUES ($1, $2, {PG_NOW_MS})"
        );
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(&q)
            .bind(code)
            .bind(used_by)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(db_err)
    }

    pub async fn insert_invite_code(
        &mut self,
        code: &str,
        uses: i64,
        for_account: &str,
        created_by: &str,
    ) -> CoreResult<()> {
        let sqlite = format!(
            "INSERT INTO invite_code (code, available_uses, for_account, created_by, created_at) \
             VALUES (?1, ?2, ?3, ?4, {SQLITE_NOW_MS})"
        );
        let pg = format!(
            "INSERT INTO invite_code (code, available_uses, for_account, created_by, created_at) \
             VALUES ($1, $2, $3, $4, {PG_NOW_MS})"
        );
        by_backend!(self, sqlite, pg, |q, ex| sqlx::query(&q)
            .bind(code)
            .bind(uses)
            .bind(for_account)
            .bind(created_by)
            .execute(ex)
            .await
            .map(|_| ()))
        .map_err(db_err)
    }
}

/// Which sessions [`Tx::revoke_sessions`] revokes.
#[derive(Debug, Clone, Copy)]
pub enum SessionFilter<'a> {
    One(&'a str),
    All,
    AppPassword(&'a str),
}

impl Store {
    async fn account_where(&self, column: &str, value: &str) -> CoreResult<Option<Account>> {
        let row: Option<AccountRow> = match self {
            Self::Sqlite(p) => {
                sqlx::query_as(&format!(
                    "SELECT {ACCOUNT_COLS} FROM account WHERE {column} = ?1"
                ))
                .bind(value)
                .fetch_optional(p)
                .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(&format!(
                    "SELECT {ACCOUNT_COLS} FROM account WHERE {column} = $1"
                ))
                .bind(value)
                .fetch_optional(p)
                .await
            }
        }
        .map_err(db_err)?;
        row.map(Account::try_from).transpose()
    }

    pub async fn account_by_did(&self, did: &str) -> CoreResult<Option<Account>> {
        self.account_where("did", did).await
    }

    pub async fn account_by_handle(&self, handle: &str) -> CoreResult<Option<Account>> {
        self.account_where("handle", &handle.to_ascii_lowercase())
            .await
    }

    pub async fn account_by_email(&self, email: &str) -> CoreResult<Option<Account>> {
        self.account_where("email", &email.to_ascii_lowercase())
            .await
    }

    pub async fn session(&self, id: &str) -> CoreResult<Option<Session>> {
        let sql_s = "SELECT id, did, kind, scope, app_password_name, created_at, expires_at, revoked_at \
                     FROM session WHERE id = ?1";
        let sql_p = "SELECT id, did, kind, scope, app_password_name, created_at, expires_at, revoked_at \
                     FROM session WHERE id = $1";
        match self {
            Self::Sqlite(p) => sqlx::query_as(sql_s).bind(id).fetch_optional(p).await,
            Self::Postgres(p) => sqlx::query_as(sql_p).bind(id).fetch_optional(p).await,
        }
        .map_err(db_err)
    }

    pub async fn app_passwords(&self, did: &str) -> CoreResult<Vec<AppPassword>> {
        let sql_s = "SELECT did, name, lookup_hash, hash, privileged, created_at FROM app_password \
                     WHERE did = ?1 ORDER BY created_at";
        let sql_p = "SELECT did, name, lookup_hash, hash, privileged, created_at FROM app_password \
                     WHERE did = $1 ORDER BY created_at";
        match self {
            Self::Sqlite(p) => sqlx::query_as(sql_s).bind(did).fetch_all(p).await,
            Self::Postgres(p) => sqlx::query_as(sql_p).bind(did).fetch_all(p).await,
        }
        .map_err(db_err)
    }

    /// The app password whose lookup hash matches, if any.
    pub async fn app_password_by_lookup(
        &self,
        did: &str,
        lookup_hash: &str,
    ) -> CoreResult<Option<AppPassword>> {
        let sql_s = "SELECT did, name, lookup_hash, hash, privileged, created_at FROM app_password \
                     WHERE did = ?1 AND lookup_hash = ?2";
        let sql_p = "SELECT did, name, lookup_hash, hash, privileged, created_at FROM app_password \
                     WHERE did = $1 AND lookup_hash = $2";
        match self {
            Self::Sqlite(p) => {
                sqlx::query_as(sql_s)
                    .bind(did)
                    .bind(lookup_hash)
                    .fetch_optional(p)
                    .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(sql_p)
                    .bind(did)
                    .bind(lookup_hash)
                    .fetch_optional(p)
                    .await
            }
        }
        .map_err(db_err)
    }
}
