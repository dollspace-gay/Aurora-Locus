//! Job run bookkeeping: last run, last success, next run and last error per
//! job, for the console and CLI.

use locus_core::CoreResult;

use crate::{PG_NOW_MS, SQLITE_NOW_MS, Store, db_err};

/// One job's recorded state.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct JobState {
    pub name: String,
    pub last_run_at: Option<i64>,
    pub last_ok_at: Option<i64>,
    pub next_run_at: Option<i64>,
    pub last_error: Option<String>,
    pub runs: i64,
}

impl Store {
    /// Records that `name` ran, successfully or with `error`, and when it
    /// runs next (ms since the epoch), if scheduled.
    pub async fn record_job_run(
        &self,
        name: &str,
        error: Option<&str>,
        next_run_at: Option<i64>,
    ) -> CoreResult<()> {
        let ok = error.is_none();
        match self {
            Self::Sqlite(p) => sqlx::query(&format!(
                "INSERT INTO job_state (name, last_run_at, last_ok_at, next_run_at, last_error, runs) \
                 VALUES (?1, {SQLITE_NOW_MS}, CASE WHEN ?2 THEN {SQLITE_NOW_MS} END, ?3, ?4, 1) \
                 ON CONFLICT (name) DO UPDATE SET last_run_at = excluded.last_run_at, \
                 last_ok_at = COALESCE(excluded.last_ok_at, job_state.last_ok_at), \
                 next_run_at = excluded.next_run_at, last_error = excluded.last_error, \
                 runs = job_state.runs + 1"
            ))
            .bind(name)
            .bind(ok)
            .bind(next_run_at)
            .bind(error)
            .execute(p)
            .await
            .map(|_| ()),
            Self::Postgres(p) => sqlx::query(&format!(
                "INSERT INTO job_state (name, last_run_at, last_ok_at, next_run_at, last_error, runs) \
                 VALUES ($1, {PG_NOW_MS}, CASE WHEN $2 THEN {PG_NOW_MS} END, $3, $4, 1) \
                 ON CONFLICT (name) DO UPDATE SET last_run_at = excluded.last_run_at, \
                 last_ok_at = COALESCE(excluded.last_ok_at, job_state.last_ok_at), \
                 next_run_at = excluded.next_run_at, last_error = excluded.last_error, \
                 runs = job_state.runs + 1"
            ))
            .bind(name)
            .bind(ok)
            .bind(next_run_at)
            .bind(error)
            .execute(p)
            .await
            .map(|_| ()),
        }
        .map_err(db_err)
    }

    /// Every job's recorded state, by name.
    pub async fn job_states(&self) -> CoreResult<Vec<JobState>> {
        let sql = "SELECT name, last_run_at, last_ok_at, next_run_at, last_error, runs \
                   FROM job_state ORDER BY name";
        match self {
            Self::Sqlite(p) => sqlx::query_as(sql).fetch_all(p).await,
            Self::Postgres(p) => sqlx::query_as(sql).fetch_all(p).await,
        }
        .map_err(db_err)
    }
}
