//! Email tokens (one per account and purpose, stored as a keyed hash),
//! account email changes, and the state of each email sender.

use locus_core::{CoreError, CoreResult};

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, Tx, db_err};

/// What an emailed token is for (the reference's purposes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenPurpose {
    ConfirmEmail,
    UpdateEmail,
    ResetPassword,
    DeleteAccount,
    PlcOperation,
    AuthFactor,
}

impl TokenPurpose {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmEmail => "confirm_email",
            Self::UpdateEmail => "update_email",
            Self::ResetPassword => "reset_password",
            Self::DeleteAccount => "delete_account",
            Self::PlcOperation => "plc_operation",
            Self::AuthFactor => "auth_factor",
        }
    }
}

/// An email sender: account mail or moderation mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    Main,
    Moderation,
}

impl Sender {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Moderation => "moderation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderStatus {
    /// A test email arrived.
    Verified,
    /// Connect and auth passed; delivery not yet confirmed.
    Unverified,
    Failed,
}

impl SenderStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
            Self::Failed => "failed",
        }
    }

    fn parse(s: &str) -> CoreResult<Self> {
        match s {
            "verified" => Ok(Self::Verified),
            "unverified" => Ok(Self::Unverified),
            "failed" => Ok(Self::Failed),
            other => Err(CoreError::Storage(format!(
                "unknown sender status {other:?}"
            ))),
        }
    }

    /// Whether mail can be sent.
    #[must_use]
    pub fn configured(self) -> bool {
        !matches!(self, Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SenderState {
    pub status: SenderStatus,
    /// Hash of the SMTP URL the status is for.
    pub url_hash: String,
    pub last_error: Option<String>,
    pub failures: i64,
}

macro_rules! on_pool {
    ($self:expr, $sqlite:expr, $pg:expr, |$q:ident, $pool:ident| $body:expr) => {
        match $self {
            Store::Sqlite($pool) => {
                let $q: &str = &$sqlite;
                $body
            }
            Store::Postgres($pool) => {
                let $q: &str = &$pg;
                $body
            }
        }
    };
}

macro_rules! on_tx {
    ($self:expr, $sqlite:expr, $pg:expr, |$q:ident, $ex:ident| $body:expr) => {
        match $self {
            Tx::Sqlite(t) => {
                let $q: &str = &$sqlite;
                let $ex = &mut **t;
                $body
            }
            Tx::Postgres(t) => {
                let $q: &str = &$pg;
                let $ex = &mut **t;
                $body
            }
        }
    };
}

impl Store {
    /// Stores a token for (did, purpose), replacing any earlier one.
    pub async fn put_email_token(
        &self,
        did: &str,
        purpose: TokenPurpose,
        token_hash: &str,
    ) -> CoreResult<()> {
        let upsert = |now: &str, a: &str, b: &str, c: &str| {
            format!(
                "INSERT INTO email_token (did, purpose, token_hash, requested_at) VALUES ({a}, {b}, {c}, {now}) \
                 ON CONFLICT (did, purpose) DO UPDATE SET token_hash = excluded.token_hash, \
                 requested_at = excluded.requested_at"
            )
        };
        on_pool!(
            self,
            upsert(SQLITE_NOW_MS, "?1", "?2", "?3"),
            upsert(PG_NOW_MS, "$1", "$2", "$3"),
            |q, p| sqlx::query(q)
                .bind(did)
                .bind(purpose.as_str())
                .bind(token_hash)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// The stored token hash for (did, purpose) and its age in ms.
    pub async fn email_token(
        &self,
        did: &str,
        purpose: TokenPurpose,
    ) -> CoreResult<Option<(String, i64)>> {
        on_pool!(
            self,
            format!(
                "SELECT token_hash, {SQLITE_NOW_MS} - requested_at FROM email_token WHERE did = ?1 AND purpose = ?2"
            ),
            format!(
                "SELECT token_hash, {PG_NOW_MS} - requested_at FROM email_token WHERE did = $1 AND purpose = $2"
            ),
            |q, p| sqlx::query_as(q)
                .bind(did)
                .bind(purpose.as_str())
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )
    }

    /// The account a token belongs to and the token's age in ms.
    pub async fn email_token_owner(
        &self,
        purpose: TokenPurpose,
        token_hash: &str,
    ) -> CoreResult<Option<(String, i64)>> {
        on_pool!(
            self,
            format!(
                "SELECT did, {SQLITE_NOW_MS} - requested_at FROM email_token WHERE purpose = ?1 AND token_hash = ?2"
            ),
            format!(
                "SELECT did, {PG_NOW_MS} - requested_at FROM email_token WHERE purpose = $1 AND token_hash = $2"
            ),
            |q, p| sqlx::query_as(q)
                .bind(purpose.as_str())
                .bind(token_hash)
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )
    }

    pub async fn sender_state(&self, sender: Sender) -> CoreResult<Option<SenderState>> {
        let row: Option<(String, String, Option<String>, i64)> = on_pool!(
            self,
            "SELECT status, url_hash, last_error, failures FROM email_sender WHERE name = ?1",
            "SELECT status, url_hash, last_error, failures::bigint FROM email_sender WHERE name = $1",
            |q, p| sqlx::query_as(q)
                .bind(sender.as_str())
                .fetch_optional(p)
                .await
                .map_err(db_err)
        )?;
        row.map(|(status, url_hash, last_error, failures)| {
            Ok(SenderState {
                status: SenderStatus::parse(&status)?,
                url_hash,
                last_error,
                failures,
            })
        })
        .transpose()
    }

    /// Records a sender's status for the URL with `url_hash`.
    pub async fn set_sender_state(
        &self,
        sender: Sender,
        status: SenderStatus,
        url_hash: &str,
        error: Option<&str>,
    ) -> CoreResult<()> {
        let upsert = |now: &str, a: &str, b: &str, c: &str, d: &str| {
            format!(
                "INSERT INTO email_sender (name, status, url_hash, checked_at, last_error, failures) \
                 VALUES ({a}, {b}, {c}, {now}, {d}, 0) \
                 ON CONFLICT (name) DO UPDATE SET status = excluded.status, url_hash = excluded.url_hash, \
                 checked_at = excluded.checked_at, last_error = excluded.last_error, failures = 0"
            )
        };
        on_pool!(
            self,
            upsert(SQLITE_NOW_MS, "?1", "?2", "?3", "?4"),
            upsert(PG_NOW_MS, "$1", "$2", "$3", "$4"),
            |q, p| sqlx::query(q)
                .bind(sender.as_str())
                .bind(status.as_str())
                .bind(url_hash)
                .bind(error)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Counts a failed send (the console warns after repeated failures).
    pub async fn count_send_failure(&self, sender: Sender, error: &str) -> CoreResult<()> {
        on_pool!(
            self,
            "UPDATE email_sender SET failures = failures + 1, last_error = ?2 WHERE name = ?1",
            "UPDATE email_sender SET failures = failures + 1, last_error = $2 WHERE name = $1",
            |q, p| sqlx::query(q)
                .bind(sender.as_str())
                .bind(error)
                .execute(p)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }
}

impl Tx {
    pub async fn delete_email_token(&mut self, did: &str, purpose: TokenPurpose) -> CoreResult<()> {
        on_tx!(
            self,
            "DELETE FROM email_token WHERE did = ?1 AND purpose = ?2",
            "DELETE FROM email_token WHERE did = $1 AND purpose = $2",
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(purpose.as_str())
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    pub async fn delete_all_email_tokens(&mut self, did: &str) -> CoreResult<()> {
        on_tx!(
            self,
            "DELETE FROM email_token WHERE did = ?1",
            "DELETE FROM email_token WHERE did = $1",
            |q, ex| sqlx::query(q)
                .bind(did)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    pub async fn set_email_confirmed(&mut self, did: &str) -> CoreResult<()> {
        on_tx!(
            self,
            format!("UPDATE account SET email_confirmed_at = {SQLITE_NOW_MS} WHERE did = ?1"),
            format!("UPDATE account SET email_confirmed_at = {PG_NOW_MS} WHERE did = $1"),
            |q, ex| sqlx::query(q)
                .bind(did)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }

    /// Changes an account's email: unconfirmed, with the sign-in second
    /// factor off and every outstanding token dropped. An address another
    /// account holds is a [`CoreError::Conflict`].
    pub async fn set_email(&mut self, did: &str, email: &str) -> CoreResult<()> {
        let res = on_tx!(
            self,
            "UPDATE account SET email = ?2, email_confirmed_at = NULL, email_auth_factor_at = NULL WHERE did = ?1",
            "UPDATE account SET email = $2, email_confirmed_at = NULL, email_auth_factor_at = NULL WHERE did = $1",
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(email)
                .execute(ex)
                .await
                .map(|r| r.rows_affected())
        );
        match res {
            Ok(_) => {}
            Err(e) if crate::accounts::is_unique_violation(&e) => {
                return Err(CoreError::Conflict(
                    "This email address is already in use, please use a different email.".into(),
                ));
            }
            Err(e) => return Err(db_err(e)),
        }
        self.delete_all_email_tokens(did).await
    }

    /// Turns email sign-in codes on (only while the email is confirmed and
    /// still `email`) or off. Returns whether a row changed.
    pub async fn set_email_auth_factor(
        &mut self,
        did: &str,
        email: &str,
        on: bool,
    ) -> CoreResult<bool> {
        let res = if on {
            on_tx!(
                self,
                format!(
                    "UPDATE account SET email_auth_factor_at = {SQLITE_NOW_MS} \
                     WHERE did = ?1 AND email = ?2 AND email_confirmed_at IS NOT NULL"
                ),
                format!(
                    "UPDATE account SET email_auth_factor_at = {PG_NOW_MS} \
                     WHERE did = $1 AND email = $2 AND email_confirmed_at IS NOT NULL"
                ),
                |q, ex| sqlx::query(q)
                    .bind(did)
                    .bind(email)
                    .execute(ex)
                    .await
                    .map(|r| r.rows_affected())
            )
        } else {
            on_tx!(
                self,
                "UPDATE account SET email_auth_factor_at = NULL WHERE did = ?1 AND email = ?2",
                "UPDATE account SET email_auth_factor_at = NULL WHERE did = $1 AND email = $2",
                |q, ex| sqlx::query(q)
                    .bind(did)
                    .bind(email)
                    .execute(ex)
                    .await
                    .map(|r| r.rows_affected())
            )
        };
        res.map(|n| n == 1).map_err(db_err)
    }

    pub async fn set_password_hash(&mut self, did: &str, hash: &str) -> CoreResult<()> {
        on_tx!(
            self,
            "UPDATE account SET password_hash = ?2 WHERE did = ?1",
            "UPDATE account SET password_hash = $2 WHERE did = $1",
            |q, ex| sqlx::query(q)
                .bind(did)
                .bind(hash)
                .execute(ex)
                .await
                .map(|_| ())
                .map_err(db_err)
        )
    }
}
