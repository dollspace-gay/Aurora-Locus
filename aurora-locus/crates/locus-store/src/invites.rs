//! Reading invite codes with their uses (`getAccountInviteCodes`).

use locus_core::CoreResult;

use crate::{Store, Tx, db_err};

/// An invite code as the reference reports it: `available` is the total
/// number of uses it was made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteCode {
    pub code: String,
    pub available: i64,
    pub disabled: bool,
    pub for_account: String,
    pub created_by: String,
    pub created_at: i64,
    /// (used_by, used_at)
    pub uses: Vec<(String, i64)>,
}

impl Store {
    /// Every invite code made for `did`, with its uses.
    pub async fn invite_codes_for(&self, did: &str) -> CoreResult<Vec<InviteCode>> {
        let codes: Vec<(String, i64, bool, String, String, i64)> = match self {
            Self::Sqlite(p) => sqlx::query_as(
                "SELECT code, available_uses, disabled, for_account, created_by, created_at \
                 FROM invite_code WHERE for_account = ?1 ORDER BY created_at",
            )
            .bind(did)
            .fetch_all(p)
            .await,
            Self::Postgres(p) => sqlx::query_as(
                "SELECT code, available_uses::bigint, disabled, for_account, created_by, created_at \
                 FROM invite_code WHERE for_account = $1 ORDER BY created_at",
            )
            .bind(did)
            .fetch_all(p)
            .await,
        }
        .map_err(db_err)?;
        let uses: Vec<(String, String, i64)> = match self {
            Self::Sqlite(p) => {
                sqlx::query_as(
                    "SELECT u.code, u.used_by, u.used_at FROM invite_code_use u \
                 JOIN invite_code c ON c.code = u.code WHERE c.for_account = ?1 ORDER BY u.used_at",
                )
                .bind(did)
                .fetch_all(p)
                .await
            }
            Self::Postgres(p) => {
                sqlx::query_as(
                    "SELECT u.code, u.used_by, u.used_at FROM invite_code_use u \
                 JOIN invite_code c ON c.code = u.code WHERE c.for_account = $1 ORDER BY u.used_at",
                )
                .bind(did)
                .fetch_all(p)
                .await
            }
        }
        .map_err(db_err)?;
        Ok(codes
            .into_iter()
            .map(
                |(code, remaining, disabled, for_account, created_by, created_at)| {
                    let uses: Vec<(String, i64)> = uses
                        .iter()
                        .filter(|(c, _, _)| *c == code)
                        .map(|(_, by, at)| (by.clone(), *at))
                        .collect();
                    InviteCode {
                        available: remaining + i64::try_from(uses.len()).unwrap_or(i64::MAX),
                        code,
                        disabled,
                        for_account,
                        created_by,
                        created_at,
                        uses,
                    }
                },
            )
            .collect())
    }
}

impl Tx {
    pub async fn set_invite_disabled(&mut self, code: &str, disabled: bool) -> CoreResult<()> {
        match self {
            Self::Sqlite(t) => sqlx::query("UPDATE invite_code SET disabled = ?2 WHERE code = ?1")
                .bind(code)
                .bind(disabled)
                .execute(&mut **t)
                .await
                .map(|_| ()),
            Self::Postgres(t) => {
                sqlx::query("UPDATE invite_code SET disabled = $2 WHERE code = $1")
                    .bind(code)
                    .bind(disabled)
                    .execute(&mut **t)
                    .await
                    .map(|_| ())
            }
        }
        .map_err(db_err)
    }

    /// The number of codes made for `did` that weren't gifted by an
    /// admin, counted inside the transaction (Postgres locks the account
    /// row first, so two requests can't both top the account up).
    pub async fn routine_invite_count(&mut self, did: &str) -> CoreResult<i64> {
        match self {
            Self::Sqlite(t) => {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM invite_code WHERE for_account = ?1 AND created_by <> 'admin'",
                )
                .bind(did)
                .fetch_one(&mut **t)
                .await
            }
            Self::Postgres(t) => {
                sqlx::query("SELECT 1 FROM account WHERE did = $1 FOR UPDATE")
                    .bind(did)
                    .execute(&mut **t)
                    .await
                    .map_err(db_err)?;
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM invite_code WHERE for_account = $1 AND created_by <> 'admin'",
                )
                .bind(did)
                .fetch_one(&mut **t)
                .await
            }
        }
        .map_err(db_err)
    }
}
