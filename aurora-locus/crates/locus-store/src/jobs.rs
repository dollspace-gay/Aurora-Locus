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

/// An operator's control of a leased job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobControl {
    pub paused: bool,
    pub paused_by: Option<String>,
    /// When a run-now was asked for, if one is waiting.
    pub run_requested: Option<i64>,
}

impl Store {
    /// The control row of a job (defaults: running, nothing asked).
    pub async fn job_control(&self, name: &str) -> CoreResult<JobControl> {
        let q = crate::blob::sql(
            "SELECT paused, paused_by, run_requested FROM job_control WHERE name = {1}",
            self.is_postgres(),
        );
        let row: Option<(bool, Option<String>, Option<i64>)> = match self {
            Self::Sqlite(p) => sqlx::query_as(&q).bind(name).fetch_optional(p).await,
            Self::Postgres(p) => sqlx::query_as(&q).bind(name).fetch_optional(p).await,
        }
        .map_err(db_err)?;
        Ok(
            row.map_or_else(JobControl::default, |(paused, paused_by, run_requested)| {
                JobControl {
                    paused,
                    paused_by,
                    run_requested,
                }
            }),
        )
    }

    /// Every job's control row: (name, control).
    pub async fn job_controls(&self) -> CoreResult<Vec<(String, JobControl)>> {
        let q = "SELECT name, paused, paused_by, run_requested FROM job_control ORDER BY name";
        let rows: Vec<(String, bool, Option<String>, Option<i64>)> = match self {
            Self::Sqlite(p) => sqlx::query_as(q).fetch_all(p).await,
            Self::Postgres(p) => sqlx::query_as(q).fetch_all(p).await,
        }
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(n, paused, paused_by, run_requested)| {
                (
                    n,
                    JobControl {
                        paused,
                        paused_by,
                        run_requested,
                    },
                )
            })
            .collect())
    }

    /// Pauses (or resumes) a job.
    pub async fn set_job_paused(&self, name: &str, paused: bool, by: &str) -> CoreResult<()> {
        let q = crate::blob::sql(
            "INSERT INTO job_control (name, paused, paused_by) VALUES ({1}, {2}, {3}) \
             ON CONFLICT (name) DO UPDATE SET paused = excluded.paused, paused_by = excluded.paused_by",
            self.is_postgres(),
        );
        let by = paused.then_some(by);
        match self {
            Self::Sqlite(p) => sqlx::query(&q)
                .bind(name)
                .bind(paused)
                .bind(by)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
            Self::Postgres(p) => sqlx::query(&q)
                .bind(name)
                .bind(paused)
                .bind(by)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)?;
        Ok(())
    }

    /// Asks the job's holder to run it now.
    pub async fn request_job_run(&self, name: &str) -> CoreResult<()> {
        let q = crate::blob::sql(
            "INSERT INTO job_control (name, run_requested) VALUES ({1}, {now}) \
             ON CONFLICT (name) DO UPDATE SET run_requested = excluded.run_requested",
            self.is_postgres(),
        );
        match self {
            Self::Sqlite(p) => sqlx::query(&q)
                .bind(name)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
            Self::Postgres(p) => sqlx::query(&q)
                .bind(name)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)?;
        Ok(())
    }

    /// Takes a waiting run-now request (only one taker wins).
    pub async fn take_job_run_request(&self, name: &str) -> CoreResult<bool> {
        let q = crate::blob::sql(
            "UPDATE job_control SET run_requested = NULL WHERE name = {1} AND run_requested IS NOT NULL",
            self.is_postgres(),
        );
        let n = match self {
            Self::Sqlite(p) => sqlx::query(&q)
                .bind(name)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
            Self::Postgres(p) => sqlx::query(&q)
                .bind(name)
                .execute(p)
                .await
                .map(|r| r.rows_affected()),
        }
        .map_err(db_err)?;
        Ok(n == 1)
    }
}
