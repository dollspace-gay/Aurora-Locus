//! Autocommit-wrapper primitives for SQL statements issued outside
//! a `pool.begin()` / `&mut conn` transaction context.
//!
//! Arc 16d (V05_DESIGN.md §9.4.3.1 + §9.4.4 Step 2.1) introduces this
//! module as the typed-wrapper substrate that `sweep_untethered_rows`
//! issues every SQL statement through. The discipline is enforced by
//! Step 5 audit item 8: any `.execute(` / `.fetch_*(` / `pool.begin(`
//! / `pool.acquire(` / `.transaction(` / `&mut conn` site inside
//! `src/blob_store/gc.rs` is a banned pattern; all SQL there must
//! call one of the four typed wrappers below.
//!
//! ## Why autocommit, not a per-page transaction
//!
//! Per §9.4.3.1 "Two-phase autocommit", the sweep runs Phase 1
//! (page-select SELECT) and Phase 2 (per-row predicate-guarded
//! DELETE) as separate statements with independent statement-scoped
//! snapshots. Wrapping a page of work in `pool.begin()` would either
//! (a) hold the row lock for the entire page on Postgres (stalling
//! concurrent uploadBlob / STRICT / unreference_blob for the page
//! duration), or (b) hold the SQLite WAL writer lock for the same
//! window. The single-statement autocommit form keeps each lock
//! window down to one row.
//!
//! ## Audit-scope clarification (V05_DESIGN.md §9.4.4 Step 5 item 8)
//!
//! These wrapper bodies use the bare sqlx methods by construction.
//! The audit scope EXPLICITLY EXCLUDES this file
//! (`src/db/autocommit.rs`) — the bare-sqlx pattern here is
//! intentional and is what the rest of the codebase routes through
//! via the typed wrappers. The audit grep scopes to
//! `src/blob_store/gc.rs` only.
//!
//! ## Streaming-query deferral (round-4 F10)
//!
//! No `.fetch(` / streaming variant in v0.5. Page-materialization is
//! forced via `autocommit_fetch_all`. If scale requires streaming,
//! a wrapper variant lands v0.6+ per V05_DESIGN.md §9.4.1.2.

use sqlx::any::AnyArguments;
use sqlx::any::AnyQueryResult;
use sqlx::query::Query;
use sqlx::Any;
use sqlx::AnyPool;

/// Execute a non-returning statement (INSERT / UPDATE / DELETE)
/// outside a transaction context. Thin pass-through to
/// `query.execute(pool)`; the discipline value is at the call site,
/// not in the wrapper body.
pub async fn autocommit_execute<'q>(
    pool: &AnyPool,
    query: Query<'q, Any, AnyArguments<'q>>,
) -> sqlx::Result<AnyQueryResult> {
    query.execute(pool).await
}

/// Fetch all rows from a SELECT outside a transaction context.
/// Page-materializes the result set — `.fetch(` streaming is
/// explicitly out of scope for v0.5 (round-4 F10, deferred to v0.6+
/// per V05_DESIGN.md §9.4.1.2).
pub async fn autocommit_fetch_all<'q>(
    pool: &AnyPool,
    query: Query<'q, Any, AnyArguments<'q>>,
) -> sqlx::Result<Vec<sqlx::any::AnyRow>> {
    query.fetch_all(pool).await
}

/// Fetch exactly one row from a SELECT outside a transaction context.
/// Errors with `sqlx::Error::RowNotFound` if zero rows match.
#[allow(dead_code)] // Reserved for sweep helpers that need exact-one shape.
pub async fn autocommit_fetch_one<'q>(
    pool: &AnyPool,
    query: Query<'q, Any, AnyArguments<'q>>,
) -> sqlx::Result<sqlx::any::AnyRow> {
    query.fetch_one(pool).await
}

/// Fetch zero or one row from a SELECT outside a transaction
/// context. The fresh-row check in `sweep_untethered_rows`
/// (V05_DESIGN.md §9.4.3.2 Case 2a mitigation) calls this:
/// `SELECT 1 FROM blob_metadata WHERE cid = $1 LIMIT 1`.
pub async fn autocommit_fetch_optional<'q>(
    pool: &AnyPool,
    query: Query<'q, Any, AnyArguments<'q>>,
) -> sqlx::Result<Option<sqlx::any::AnyRow>> {
    query.fetch_optional(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Once;

    static INSTALL: Once = Once::new();

    async fn pool() -> AnyPool {
        INSTALL.call_once(sqlx::any::install_default_drivers);
        AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn execute_runs_create_table() {
        let pool = pool().await;
        let result = autocommit_execute(
            &pool,
            sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY)"),
        )
        .await
        .expect("CREATE TABLE via wrapper");
        // SQLite reports rows_affected = 0 for DDL.
        assert_eq!(result.rows_affected(), 0);
    }

    #[tokio::test]
    async fn fetch_all_returns_all_rows() {
        let pool = pool().await;
        autocommit_execute(&pool, sqlx::query("CREATE TABLE t (n INTEGER)"))
            .await
            .unwrap();
        autocommit_execute(&pool, sqlx::query("INSERT INTO t VALUES (1), (2), (3)"))
            .await
            .unwrap();
        let rows = autocommit_fetch_all(&pool, sqlx::query("SELECT n FROM t ORDER BY n"))
            .await
            .expect("fetch_all via wrapper");
        assert_eq!(rows.len(), 3);
    }

    #[tokio::test]
    async fn fetch_optional_returns_some_when_row_exists() {
        let pool = pool().await;
        autocommit_execute(&pool, sqlx::query("CREATE TABLE t (n INTEGER)"))
            .await
            .unwrap();
        autocommit_execute(&pool, sqlx::query("INSERT INTO t VALUES (42)"))
            .await
            .unwrap();
        let row = autocommit_fetch_optional(
            &pool,
            sqlx::query("SELECT n FROM t WHERE n = 42 LIMIT 1"),
        )
        .await
        .expect("fetch_optional via wrapper");
        assert!(row.is_some());
    }

    #[tokio::test]
    async fn fetch_optional_returns_none_when_row_absent() {
        let pool = pool().await;
        autocommit_execute(&pool, sqlx::query("CREATE TABLE t (n INTEGER)"))
            .await
            .unwrap();
        let row = autocommit_fetch_optional(
            &pool,
            sqlx::query("SELECT n FROM t WHERE n = 999 LIMIT 1"),
        )
        .await
        .expect("fetch_optional empty result");
        assert!(row.is_none());
    }
}
