//! PDS-liveness cooperative lock acquired by the `serve` subcommand.
//!
//! The forthcoming `grant-admin` CLI (Arc 1 Step 4) needs to
//! fast-fail when a PDS is already running against the same database.
//! This module provides the lock infrastructure both sides talk to:
//!
//! - **Postgres**: a session-scoped advisory lock keyed by
//!   [`PDS_LIVENESS_LOCK_KEY`], held on a dedicated long-lived
//!   connection outside the application pool. A 30-second keepalive
//!   task pings `SELECT 1` to defeat NAT / load-balancer idle-killers.
//!   The lock auto-releases when the session ends (process death,
//!   crash, or graceful shutdown — all close the TCP connection).
//! - **SQLite**: an exclusive `flock(2)` on `<db_path>.aurora-lock` —
//!   a sibling lockfile, NOT the SQLite database file itself. The
//!   kernel releases the flock on process death (graceful or crash),
//!   so no manual cleanup is needed even if `serve` crashes.
//!
//! Acquisition is non-blocking on both backends:
//! `pg_try_advisory_lock` and `fs2::FileExt::try_lock_exclusive`
//! fast-fail if the lock is held. The serve entry surfaces a single
//! actionable error message.
//!
//! This module is the PDS-side acquisition only. The CLI offline-check
//! call sites (the second half of the contract) land in Step 4.

use crate::config::{DatabaseBackend, ServerConfig};
use crate::db::advisory_locks::PDS_LIVENESS_LOCK_KEY;
use crate::error::{PdsError, PdsResult};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use sqlx::{AnyConnection, Connection};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Human-actionable error message surfaced when the liveness lock is
/// already held. Worded for an operator who started a second PDS
/// instance against the same database without realizing the first
/// was still running.
const LOCK_HELD_MESSAGE: &str =
    "Failed to acquire PDS liveness lock: another Aurora-Locus PDS \
    instance appears to be running against this database. Stop the \
    running instance before starting a new one.";

/// Cooperative PDS-liveness lock. Held by the `serve` subcommand for
/// the lifetime of the running PDS process; dropping releases.
///
/// Drop semantics are backend-specific:
/// - **Postgres**: aborts the keepalive task; the task's owned
///   connection drops, the TCP socket closes, and Postgres releases
///   the session-scoped advisory lock. (Connection-drop release is
///   the correctness guarantee — explicit unlock is a best-effort
///   nicety.)
/// - **SQLite**: drops the held [`File`] handle; the kernel releases
///   the `flock(2)` immediately. (Process death also releases via the
///   same kernel path — no manual cleanup on crash.)
pub enum LivenessLock {
    /// Postgres backend — keepalive task owns the dedicated
    /// connection. Aborting the task drops the connection; the
    /// session-scoped lock releases when Postgres sees the TCP
    /// close.
    Postgres { keepalive_task: JoinHandle<()> },
    /// SQLite backend — `_file` holds the flock; lockfile path kept
    /// for diagnostic logging.
    Sqlite {
        _file: File,
        #[allow(dead_code)] // Diagnostic only — `Drop` reads it.
        path: PathBuf,
    },
}

impl LivenessLock {
    /// Acquire the PDS-liveness lock on the configured backend.
    /// Fast-fails with [`LOCK_HELD_MESSAGE`] if the lock is already
    /// held (i.e. a PDS is running).
    #[allow(dead_code)] // Wired in Step 0.11; called by `server::serve`
    pub async fn acquire(config: &ServerConfig) -> PdsResult<Self> {
        match config.database.backend {
            DatabaseBackend::Postgres => {
                let url = config.database.url.as_deref().ok_or_else(|| {
                    PdsError::Validation(
                        "PDS_DB_URL is required for Postgres backend".to_string(),
                    )
                })?;
                Self::acquire_postgres(url).await
            }
            DatabaseBackend::Sqlite => Self::acquire_sqlite(
                config.database.url.as_deref(),
                &config.storage.account_db,
            ),
        }
    }

    async fn acquire_postgres(db_url: &str) -> PdsResult<Self> {
        // Open a dedicated connection — never returned to a pool. The
        // session-scoped lock's lifetime equals this connection's;
        // the keepalive task takes ownership below.
        let mut conn = AnyConnection::connect(db_url).await.map_err(|e| {
            PdsError::Internal(format!(
                "Failed to open dedicated PDS-liveness connection: {}",
                e
            ))
        })?;
        let acquired: bool =
            sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
                .bind(PDS_LIVENESS_LOCK_KEY)
                .fetch_one(&mut conn)
                .await
                .map_err(|e| {
                    PdsError::Internal(format!(
                        "pg_try_advisory_lock failed: {}",
                        e
                    ))
                })?;
        if !acquired {
            // Lock held elsewhere — close the dedicated connection so
            // we don't hold an idle session-scoped connection on the
            // database for no reason.
            let _ = conn.close().await;
            return Err(PdsError::Internal(LOCK_HELD_MESSAGE.to_string()));
        }
        info!(
            key = PDS_LIVENESS_LOCK_KEY,
            "liveness_lock: acquired Postgres advisory lock"
        );
        // Move the connection into the keepalive task. The task pings
        // every 30s to defeat middlebox idle-killers. Aborting the
        // task drops `conn` → TCP close → Postgres releases lock.
        let keepalive_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            // First tick fires immediately; skip it so we don't ping
            // before any meaningful idle period elapsed.
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(e) = sqlx::query("SELECT 1").execute(&mut conn).await {
                    warn!(
                        error = %e,
                        "liveness_lock: keepalive ping failed; \
                         dropping connection — Postgres will release \
                         the lock on session end"
                    );
                    return;
                }
                debug!("liveness_lock: keepalive ping ok");
            }
        });
        Ok(LivenessLock::Postgres { keepalive_task })
    }

    fn acquire_sqlite(database_url: Option<&str>, account_db: &Path) -> PdsResult<Self> {
        let lockfile_path = sqlite_lockfile_path(database_url, account_db);
        // Ensure the parent directory exists so create() doesn't fail
        // with ENOENT on a fresh deployment whose data dir hasn't
        // been materialized yet. AppContext::ensure_directories
        // creates the data dir during normal startup, but liveness
        // lock acquisition runs *before* server bind — and on a
        // fresh deployment the dir may not yet exist.
        if let Some(parent) = lockfile_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                PdsError::Internal(format!(
                    "Failed to create parent directory for PDS-liveness \
                     lock file at {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lockfile_path)
            .map_err(|e| {
                PdsError::Internal(format!(
                    "Failed to open PDS-liveness lock file at {}: {}",
                    lockfile_path.display(),
                    e
                ))
            })?;
        FileExt::try_lock_exclusive(&file).map_err(|_| {
            // try_lock_exclusive returns Err(io::Error) for both the
            // "lock held" case and genuine I/O errors. The held case
            // is overwhelmingly more likely from the operator's
            // standpoint, so we surface the actionable message.
            PdsError::Internal(LOCK_HELD_MESSAGE.to_string())
        })?;
        info!(
            path = %lockfile_path.display(),
            "liveness_lock: acquired SQLite flock"
        );
        Ok(LivenessLock::Sqlite {
            _file: file,
            path: lockfile_path,
        })
    }
}

impl Drop for LivenessLock {
    fn drop(&mut self) {
        match self {
            LivenessLock::Postgres { keepalive_task } => {
                // Abort the keepalive task — the task's owned
                // `AnyConnection` drops, TCP closes, Postgres releases
                // the session-scoped lock. JoinHandle::abort is a
                // synchronous signal; drop returns immediately.
                keepalive_task.abort();
                debug!("liveness_lock: Postgres keepalive aborted on drop");
            }
            LivenessLock::Sqlite { path, .. } => {
                // The held `File` drops via struct destructuring; the
                // kernel releases the flock immediately. No manual
                // unlock call needed.
                debug!(
                    path = %path.display(),
                    "liveness_lock: SQLite flock released on drop"
                );
            }
        }
    }
}

/// Resolve the SQLite database file the application opens, then
/// derive the sibling lockfile path. Mirrors the SQLite-path
/// resolution in [`crate::db::any_url_for`]: prefer `database.url`
/// (treated as a raw filesystem path, not a URL — see that
/// function's body) when set; otherwise fall back to
/// `storage.account_db`. The lockfile is `<db_path>.aurora-lock` —
/// never the DB file itself.
fn sqlite_lockfile_path(database_url: Option<&str>, account_db: &Path) -> PathBuf {
    let db_path: PathBuf = match database_url {
        Some(url) => PathBuf::from(url),
        None => account_db.to_path_buf(),
    };
    let new_extension = match db_path.extension() {
        Some(ext) => format!("{}.aurora-lock", ext.to_string_lossy()),
        None => "aurora-lock".to_string(),
    };
    let mut lockfile = db_path.clone();
    lockfile.set_extension(new_extension);
    lockfile
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn sqlite_lockfile_path_appends_aurora_lock_extension() {
        let lockfile = sqlite_lockfile_path(
            None,
            &PathBuf::from("/tmp/aurora/accounts.db"),
        );
        assert_eq!(
            lockfile,
            PathBuf::from("/tmp/aurora/accounts.db.aurora-lock"),
        );
    }

    #[test]
    fn sqlite_lockfile_path_handles_extensionless_db() {
        let lockfile = sqlite_lockfile_path(
            None,
            &PathBuf::from("/tmp/aurora/accounts"),
        );
        assert_eq!(
            lockfile,
            PathBuf::from("/tmp/aurora/accounts.aurora-lock"),
        );
    }

    #[test]
    fn sqlite_lockfile_path_prefers_database_url_when_set() {
        let lockfile = sqlite_lockfile_path(
            Some("/var/lib/aurora/custom.db"),
            &PathBuf::from("/should/be/ignored.db"),
        );
        assert_eq!(
            lockfile,
            PathBuf::from("/var/lib/aurora/custom.db.aurora-lock"),
        );
    }

    #[test]
    fn sqlite_acquire_succeeds_on_first_attempt() {
        let dir = tempdir().unwrap();
        let account_db = dir.path().join("accounts.db");
        let lock = LivenessLock::acquire_sqlite(None, &account_db).unwrap();
        // Hold the lock until end of scope; lockfile must exist.
        assert!(dir.path().join("accounts.db.aurora-lock").exists());
        drop(lock);
    }

    #[test]
    fn sqlite_second_acquire_in_same_process_fails_while_first_held() {
        // Same-process flock semantics: `fs2::FileExt::try_lock_exclusive`
        // uses `flock(2)` with `LOCK_EX | LOCK_NB` on Unix. Linux flock
        // is per-open-file-description, so two `OpenOptions::open` calls
        // produce separate descriptions and the second flock attempt
        // conflicts with the first. This pins that semantic.
        let dir = tempdir().unwrap();
        let account_db = dir.path().join("accounts.db");
        let first = LivenessLock::acquire_sqlite(None, &account_db).unwrap();
        match LivenessLock::acquire_sqlite(None, &account_db) {
            Ok(_) => panic!("second acquire must fail while first is held"),
            Err(e) => {
                let err_msg = format!("{}", e);
                assert!(
                    err_msg.contains("another Aurora-Locus PDS instance"),
                    "error must surface the actionable LOCK_HELD_MESSAGE; got: {}",
                    err_msg,
                );
            }
        }
        // Drop first → next acquire must now succeed.
        drop(first);
        match LivenessLock::acquire_sqlite(None, &account_db) {
            Ok(_) => {}
            Err(e) => panic!("acquire after first drop must succeed; got: {}", e),
        }
    }
}
