//! Synthetic benchmark for Arc 10's GC sweep query (chainlink
//! #57). Confirms `SELECT cid FROM blob WHERE cid IN (...)`
//! stays index-driven up to page-size 500 — the proposed Step 2
//! query shape from Step 0 Q5.
//!
//! Step 0 Q5 deferred `EXPLAIN ANALYZE` because the dev tree's
//! blob count is <100; this benchmark seeds a synthetic dataset
//! large enough to exercise the planner's index-vs-scan choice
//! and confirms that the plan remains index-driven.
//!
//! `--ignored` by default so `cargo test --lib` doesn't pay
//! the seed cost. Run manually with:
//!
//! ```text
//! cargo test --test blob_in_clause_benchmark -- --ignored --nocapture
//! ```
//!
//! Postgres variant: this benchmark targets SQLite only. Step 0
//! Q5's recommendation noted that Postgres `EXPLAIN ANALYZE` is
//! the right tool there and would need testcontainers
//! scaffolding equivalent to `tests/distributed_substrate_test.rs`.
//! Postgres verification is a Step 2 gate; Step 1's deliverable
//! is the SQLite-side confirmation that the IN-clause query
//! shape stays index-driven on the typical local-dev backend.

use sqlx::any::AnyPoolOptions;
use sqlx::{AnyPool, Row};
use std::sync::Once;
use std::time::Instant;

/// Seed size. Brief suggested 1M; 100k is large enough to
/// detect a planner regression (the autoindex on a TEXT PK
/// is consulted for IN-clause lookups regardless of cardinality)
/// while keeping the benchmark's runtime tractable for `--ignored`
/// invocations during cycle close.
const SEED_ROWS: usize = 100_000;

/// Page size to validate. Matches Step 2's proposed sweep
/// page size from Step 0 Q5.
const PAGE_SIZE: usize = 500;

/// Pass-fail threshold from the brief.
const THRESHOLD_MS: u128 = 50;

async fn open_pool() -> AnyPool {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(sqlx::any::install_default_drivers);
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("open sqlite::memory: pool");

    // Mirror migrations/0001_initial.sql:156-165's `blob` table.
    sqlx::query(
        "CREATE TABLE blob (
            cid          TEXT PRIMARY KEY,
            did          TEXT NOT NULL,
            size         INTEGER NOT NULL,
            mime_type    TEXT NOT NULL,
            created_at   TEXT NOT NULL,
            takedown     INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(&pool)
    .await
    .expect("create blob table");

    sqlx::query("CREATE INDEX idx_blob_did ON blob(did)")
        .execute(&pool)
        .await
        .expect("create idx_blob_did");

    pool
}

/// Deterministic CID synthesis. Hashes the seed index into a
/// fixed-width hex string; uniform distribution across the
/// keyspace keeps the autoindex's B-tree balanced.
fn synthesise_cid(i: usize) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(i.to_le_bytes());
    let bytes = h.finalize();
    format!("bafy{}", hex::encode(&bytes[..16]))
}

async fn seed(pool: &AnyPool, n: usize) {
    // Single transaction so SQLite's WAL flush amortises.
    let mut tx = pool.begin().await.expect("begin tx");
    for i in 0..n {
        sqlx::query(
            "INSERT INTO blob (cid, did, size, mime_type, created_at, takedown) \
             VALUES ($1, $2, $3, $4, $5, 0)",
        )
        .bind(synthesise_cid(i))
        .bind(format!("did:plc:synth{:04}", i % 10_000))
        .bind(1024i64)
        .bind("application/octet-stream")
        .bind("2026-05-13T00:00:00Z")
        .execute(&mut *tx)
        .await
        .expect("insert");
    }
    tx.commit().await.expect("commit seed tx");
}

/// Build a `cid IN ($1, $2, ..., $N)` clause + bind params.
fn placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("${}", i))
        .collect::<Vec<_>>()
        .join(", ")
}

#[tokio::test]
#[ignore = "synthetic benchmark; run manually with --ignored --nocapture"]
async fn benchmark_in_clause_query_at_page_500() {
    let pool = open_pool().await;

    eprintln!("Seeding {} synthetic blob rows...", SEED_ROWS);
    let seed_start = Instant::now();
    seed(&pool, SEED_ROWS).await;
    eprintln!("Seed complete in {:?}", seed_start.elapsed());

    // Half present, half absent — mixed worst/best case for
    // the planner. Present rows are pulled from the lower half
    // of the seed; absent CIDs use offsets past `SEED_ROWS` so
    // their hashes don't collide with seeded ones.
    let mut candidates: Vec<String> = Vec::with_capacity(PAGE_SIZE);
    for i in 0..(PAGE_SIZE / 2) {
        candidates.push(synthesise_cid(i * 7)); // present
    }
    for i in 0..(PAGE_SIZE / 2) {
        candidates.push(synthesise_cid(SEED_ROWS + i)); // absent
    }
    assert_eq!(candidates.len(), PAGE_SIZE);

    // Run the candidate IN-clause query.
    let sql = format!("SELECT cid FROM blob WHERE cid IN ({})", placeholders(PAGE_SIZE));
    let mut q = sqlx::query(&sql);
    for c in &candidates {
        q = q.bind(c);
    }
    let start = Instant::now();
    let rows = q.fetch_all(&pool).await.expect("IN-clause query");
    let elapsed = start.elapsed();

    let present_cids: Vec<String> = rows
        .iter()
        .map(|r| r.try_get::<String, _>("cid").unwrap())
        .collect();

    eprintln!(
        "IN-clause query at page_size={}: returned {} present CIDs in {:?}",
        PAGE_SIZE,
        present_cids.len(),
        elapsed
    );

    // EXPLAIN QUERY PLAN — surfaces whether the planner used
    // the autoindex on `cid` (the TEXT PK's unique index) or
    // fell back to a scan.
    let explain_sql = format!(
        "EXPLAIN QUERY PLAN SELECT cid FROM blob WHERE cid IN ({})",
        placeholders(PAGE_SIZE)
    );
    let mut eq = sqlx::query(&explain_sql);
    for c in &candidates {
        eq = eq.bind(c);
    }
    let plan_rows = eq.fetch_all(&pool).await.expect("EXPLAIN");
    eprintln!("Query plan:");
    for row in &plan_rows {
        // sqlite EXPLAIN QUERY PLAN returns (id, parent, notused, detail)
        let detail: String = row.try_get("detail").unwrap_or_default();
        eprintln!("  {}", detail);
    }

    // Assert that the planner is using an index — the plan
    // should mention SEARCH (index-driven) rather than SCAN
    // (table scan). SQLite's autoindex on a TEXT PRIMARY KEY
    // satisfies this naturally.
    let plan_text = plan_rows
        .iter()
        .map(|r| r.try_get::<String, _>("detail").unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan_text.contains("SEARCH") || plan_text.contains("USING INDEX"),
        "expected index-driven plan; got:\n{}",
        plan_text
    );

    // Assert ~present-count matches the lower-half seed
    // (some collisions across the seed are theoretically
    // possible but with SHA-256 truncated to 16 bytes are
    // astronomically unlikely at 100k rows).
    assert_eq!(
        present_cids.len(),
        PAGE_SIZE / 2,
        "expected half of the candidate CIDs to be present"
    );

    // Pass-fail threshold from the brief.
    assert!(
        elapsed.as_millis() < THRESHOLD_MS,
        "IN-clause query at page_size={} took {:?} (>{}ms threshold); \
         consider Step 0 Q5's fallback hierarchy: \
         (a) drop page size, (b) per-CID point-lookups, (c) temp-table-join",
        PAGE_SIZE,
        elapsed,
        THRESHOLD_MS
    );
}
