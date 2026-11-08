//! Running nodes and offline commands keep out of each other's way. On
//! SQLite a node holds an exclusive `flock` on `<data>/node.lock` (one node
//! per data dir) and an offline command takes the same lock; on Postgres
//! every node holds a shared session advisory lock and an offline command
//! needs the exclusive one.

use locus_core::bootstrap::{Bootstrap, DatabaseUrl};
use locus_core::{CoreError, CoreResult};

/// The advisory lock key (any constant every node agrees on).
const ADVISORY_KEY: i64 = 0x6c6f_6375_7300_0001;

/// A held lock; dropped, it's released.
pub struct NodeLock {
    #[cfg(unix)]
    _file: Option<std::fs::File>,
    _pg: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
}

/// Whether a lock is for a node (shared on Postgres) or an offline
/// command (exclusive).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Holder {
    Node,
    Offline,
}

#[cfg(unix)]
fn lock_file(bootstrap: &Bootstrap, who: Holder) -> CoreResult<std::fs::File> {
    use rustix::fs::{FlockOperation, flock};
    std::fs::create_dir_all(&bootstrap.data_dir)
        .map_err(|e| CoreError::Config(format!("{}: {e}", bootstrap.data_dir.display())))?;
    let path = bootstrap.data_dir.join("node.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| CoreError::Config(format!("{}: {e}", path.display())))?;
    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|_| {
        CoreError::Conflict(match who {
            Holder::Node => format!(
                "another node or an offline command holds {} (one node per SQLite data dir)",
                path.display()
            ),
            Holder::Offline => {
                "a node is running on this install; stop it first (or this command can't run online)"
                    .to_owned()
            }
        })
    })?;
    Ok(file)
}

/// Takes the lock for `who`, or explains who holds it.
pub async fn acquire(bootstrap: &Bootstrap, who: Holder) -> CoreResult<NodeLock> {
    match &bootstrap.database {
        DatabaseUrl::Sqlite(_) => Ok(NodeLock {
            #[cfg(unix)]
            _file: Some(lock_file(bootstrap, who)?),
            _pg: None,
        }),
        DatabaseUrl::Postgres(url) => {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(url)
                .await
                .map_err(|e| CoreError::Storage(format!("lock connection: {e}")))?;
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| CoreError::Storage(format!("lock connection: {e}")))?;
            let q = match who {
                Holder::Node => "SELECT pg_try_advisory_lock_shared($1)",
                Holder::Offline => "SELECT pg_try_advisory_lock($1)",
            };
            let got: bool = sqlx::query_scalar(q)
                .bind(ADVISORY_KEY)
                .fetch_one(&mut *conn)
                .await
                .map_err(|e| CoreError::Storage(format!("advisory lock: {e}")))?;
            if !got {
                return Err(CoreError::Conflict(match who {
                    Holder::Node => {
                        "an offline command is running on this database; wait for it".to_owned()
                    }
                    Holder::Offline => {
                        "a node is running on this database; stop every node first".to_owned()
                    }
                }));
            }
            Ok(NodeLock {
                #[cfg(unix)]
                _file: None,
                _pg: Some(conn),
            })
        }
    }
}

/// An offline command's lock. With `force`, a running node is warned
/// about instead of refused, and the command runs without the lock.
pub async fn acquire_offline(
    bootstrap: &Bootstrap,
    command: &str,
    force: bool,
) -> CoreResult<NodeLock> {
    match acquire(bootstrap, Holder::Offline).await {
        Err(CoreError::Conflict(why)) if force => {
            eprintln!(
                "Warning: a node is running on this install; `{command}` runs anyway (--force), \
                 on data the node is using."
            );
            tracing::warn!(command, "forced past a running node: {why}");
            Ok(NodeLock {
                #[cfg(unix)]
                _file: None,
                _pg: None,
            })
        }
        r => r,
    }
}

/// Refuses `--force` on the commands that can never be forced.
pub fn never_forced(command: &str, force: bool) -> Result<(), String> {
    if force {
        return Err(format!(
            "`{command}` can never be forced: stop every node first"
        ));
    }
    Ok(())
}
