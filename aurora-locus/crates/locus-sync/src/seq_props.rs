//! The sequencer as a state machine, on SQLite and (with
//! `LOCUS_TEST_POSTGRES_URL`) Postgres: random interleavings of commits
//! from several accounts, sequencer passes by two nodes, lease loss and
//! takeover between them, and passes paused before their transaction and
//! after their fence check while the other node takes over. After every
//! step: seqs run 1..=n with no gaps, each account's events are in commit
//! order, every committed event is in the log or the outbox exactly once,
//! and a holder whose lease was taken sequences nothing.

use std::collections::BTreeMap;
use std::time::Duration;

use locus_core::CoreError;
use locus_core::bootstrap::DatabaseUrl;
use locus_core::lease::LeaseToken;
use proptest::prelude::*;

use super::*;

const LEASE: &str = "sequencer";
const NODES: [&str; 2] = ["node-a", "node-b"];
const DIDS: usize = 4;
const TTL: Duration = Duration::from_secs(600);
/// Deleted-account purges never come due here.
const KEEP: i64 = 86_400_000;

fn did(i: usize) -> String {
    format!("did:plc:seqprop{i:017}")
}

#[derive(Debug, Clone)]
enum Op {
    /// One transaction queueing `n` events for an account.
    Commit { did: usize, n: usize },
    /// A pass of up to `batch` rows by a node.
    Pass { node: usize, batch: i64 },
    /// The holder's lease runs out and `node` takes it.
    Takeover { node: usize },
    /// `node` starts a pass and pauses before its transaction; the other
    /// node takes over and runs a pass; the paused pass resumes.
    PausedBeforeBegin { node: usize, batch: i64 },
    /// `node` starts a pass and pauses after its fence check; the other
    /// node tries to take over meanwhile; the paused pass resumes.
    PausedAfterFence { node: usize, batch: i64 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => (0..DIDS, 1..4usize).prop_map(|(did, n)| Op::Commit { did, n }),
        4 => (0..2usize, 1..5i64).prop_map(|(node, batch)| Op::Pass { node, batch }),
        1 => (0..2usize).prop_map(|node| Op::Takeover { node }),
        1 => (0..2usize, 1..5i64).prop_map(|(node, batch)| Op::PausedBeforeBegin { node, batch }),
        1 => (0..2usize, 1..5i64).prop_map(|(node, batch)| Op::PausedAfterFence { node, batch }),
    ]
}

/// What the test knows: each account's events in commit order, which node
/// holds the lease, and every node's latest token.
struct Model {
    committed: Vec<Vec<u64>>,
    next: u64,
    holder: usize,
    tokens: [Option<LeaseToken>; 2],
}

fn payload(did: usize, n: u64) -> Vec<u8> {
    let mut m = BTreeMap::new();
    m.insert("did".to_owned(), LexValue::Integer(did as i64));
    m.insert("n".to_owned(), LexValue::Integer(n as i64));
    proto_blue_lex_cbor::encode(&LexValue::Map(m)).unwrap()
}

fn unpack(bytes: &[u8]) -> (usize, u64) {
    let v = proto_blue_lex_cbor::decode(bytes).unwrap();
    let m = v.as_map().unwrap();
    (
        m["did"].as_integer().unwrap() as usize,
        m["n"].as_integer().unwrap() as u64,
    )
}

async fn open(pg: Option<&str>) -> (Store, Option<tempfile::TempDir>) {
    match pg {
        None => {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
                .await
                .unwrap();
            (store, Some(dir))
        }
        Some(admin_url) => {
            let admin = sqlx::PgPool::connect(admin_url).await.unwrap();
            let db = format!("locus_test_{}", hex::encode(rand::random::<[u8; 8]>()));
            sqlx::query(&format!("CREATE DATABASE {db}"))
                .execute(&admin)
                .await
                .unwrap();
            admin.close().await;
            let (base, _) = admin_url.rsplit_once('/').unwrap();
            let store = Store::open(&DatabaseUrl::Postgres(format!("{base}/{db}")))
                .await
                .unwrap();
            (store, None)
        }
    }
}

/// The lease runs out (no release: the holder just stopped renewing).
async fn expire(store: &Store) {
    let q = "UPDATE job_lease SET expires_at = 0 WHERE name = 'sequencer'";
    match store {
        Store::Sqlite(p) => sqlx::query(q).execute(p).await.map(|_| ()),
        Store::Postgres(p) => sqlx::query(q).execute(p).await.map(|_| ()),
    }
    .unwrap();
}

async fn take(store: &Store, node: usize) -> LeaseToken {
    expire(store).await;
    store
        .try_acquire_lease(LEASE, NODES[node], TTL)
        .await
        .unwrap()
        .expect("an expired lease is free")
}

async fn outbox(store: &Store) -> Vec<(usize, u64)> {
    let mut tx = store.begin().await.unwrap();
    let rows = tx.outbox_batch(1_000_000).await.unwrap();
    tx.rollback().await.unwrap();
    rows.iter().map(|r| unpack(&r.payload)).collect()
}

/// The invariants, after every step. Returns how many events are logged.
async fn check(store: &Store, model: &Model, step: usize) -> usize {
    let log = store.seq_events_after(0, 1_000_000).await.unwrap();
    for (i, ev) in log.iter().enumerate() {
        assert_eq!(ev.seq, i as i64 + 1, "step {step}: seqs run 1..=n, no gaps");
    }
    assert_eq!(
        store.seq_state().await.unwrap().last_seq,
        log.len() as i64,
        "step {step}: last_seq"
    );
    // Per account: the log holds a prefix of the commit order, the outbox
    // the rest, so nothing is lost, repeated or reordered.
    let mut logged: Vec<Vec<u64>> = vec![Vec::new(); DIDS];
    for ev in &log {
        let (d, n) = unpack(&ev.payload);
        assert_eq!(ev.did, did(d), "step {step}: event's did");
        logged[d].push(n);
    }
    let mut queued: Vec<Vec<u64>> = vec![Vec::new(); DIDS];
    for (d, n) in outbox(store).await {
        queued[d].push(n);
    }
    for d in 0..DIDS {
        let mut all = logged[d].clone();
        all.extend(&queued[d]);
        assert_eq!(all, model.committed[d], "step {step}: account {d}");
    }
    log.len()
}

async fn pass(store: &Store, model: &Model, node: usize, batch: i64) -> CoreResult<usize> {
    let token = model.tokens[node]
        .clone()
        .expect("node has acquired once");
    sequence_once(store, &token, batch, KEEP).await
}

/// A pass's outcome: a holder's pass moves min(batch, queued) rows; a
/// stale token's pass is refused and moves nothing.
fn expect_pass(r: CoreResult<usize>, holder: bool, batch: i64, queued: usize, before: usize, after: usize, step: usize) {
    if holder {
        let n = r.unwrap_or_else(|e| panic!("step {step}: holder's pass failed: {e}"));
        assert_eq!(n, queued.min(batch as usize), "step {step}: rows moved");
        assert_eq!(after, before + n, "step {step}");
    } else {
        assert!(
            matches!(r, Err(CoreError::LeaseLost(_))),
            "step {step}: stale pass: {r:?}"
        );
        assert_eq!(after, before, "step {step}: stale pass moved rows");
    }
}

async fn run(ops: Vec<Op>, pg: Option<String>) {
    let (store, _dir) = open(pg.as_deref()).await;
    let first = take(&store, 0).await;
    let mut model = Model {
        committed: vec![Vec::new(); DIDS],
        next: 0,
        holder: 0,
        tokens: [Some(first), None],
    };
    for (step, op) in ops.into_iter().enumerate() {
        let before = check(&store, &model, step).await;
        let queued = outbox(&store).await.len();
        match op {
            Op::Commit { did: d, n } => {
                let mut tx = store.begin().await.unwrap();
                for _ in 0..n {
                    model.next += 1;
                    tx.insert_outbox(&did(d), "commit", &payload(d, model.next))
                        .await
                        .unwrap();
                    model.committed[d].push(model.next);
                }
                tx.commit().await.unwrap();
            }
            Op::Pass { node, batch } => {
                if model.tokens[node].is_none() {
                    continue;
                }
                let r = pass(&store, &model, node, batch).await;
                let after = check(&store, &model, step).await;
                expect_pass(r, model.holder == node, batch, queued, before, after, step);
            }
            Op::Takeover { node } => {
                model.tokens[node] = Some(take(&store, node).await);
                model.holder = node;
            }
            Op::PausedBeforeBegin { node, batch } => {
                if model.tokens[node].is_none() {
                    continue;
                }
                let other = 1 - node;
                let token = model.tokens[node].clone().unwrap();
                let p = fault::pause("begin", NODES[node]);
                let s = store.clone();
                let paused =
                    tokio::spawn(async move { sequence_once(&s, &token, batch, KEEP).await });
                p.reached.notified().await;
                // Meanwhile the other node takes over and sequences.
                model.tokens[other] = Some(take(&store, other).await);
                model.holder = other;
                let r = pass(&store, &model, other, 1_000).await;
                let mid = check(&store, &model, step).await;
                expect_pass(r, true, 1_000, queued, before, mid, step);
                p.resume.notify_one();
                let r = paused.await.unwrap();
                let after = check(&store, &model, step).await;
                expect_pass(r, false, batch, 0, mid, after, step);
            }
            Op::PausedAfterFence { node, batch } => {
                // Only the holder gets past the fence (a stale node's pass
                // is the plain stale case, checked after every step).
                if model.holder != node {
                    continue;
                }
                let other = 1 - node;
                let token = model.tokens[node].clone().unwrap();
                let was_holder = true;
                let p = fault::pause("fenced", NODES[node]);
                let s = store.clone();
                let paused =
                    tokio::spawn(async move { sequence_once(&s, &token, batch, KEEP).await });
                p.reached.notified().await;
                // A takeover now waits for the fenced pass (it holds the
                // lease row or the write lock), or gives up.
                let s = store.clone();
                let takeover = tokio::spawn(async move {
                    expire(&s).await;
                    s.try_acquire_lease(LEASE, NODES[other], TTL).await
                });
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert!(!takeover.is_finished(), "step {step}: takeover waited for the fenced pass");
                p.resume.notify_one();
                let r = paused.await.unwrap();
                let after = check(&store, &model, step).await;
                // Only a holder gets past the fence; it finishes its pass.
                expect_pass(r, was_holder, batch, queued, before, after, step);
                let t = takeover.await.unwrap().unwrap().expect("the lease was expired");
                model.tokens[other] = Some(t);
                model.holder = other;
            }
        }
        check(&store, &model, step).await;
        // A stale node (if it has a token) can't sequence after takeover.
        let stale = 1 - model.holder;
        if model.tokens[stale].is_some() {
            let before = check(&store, &model, step).await;
            let r = pass(&store, &model, stale, 1_000).await;
            let after = check(&store, &model, step).await;
            expect_pass(r, false, 1_000, 0, before, after, step);
        }
    }
    // Finally the holder drains the outbox: everything is logged once.
    let r = pass(&store, &model, model.holder, 1_000_000).await;
    assert!(r.is_ok(), "{r:?}");
    assert!(outbox(&store).await.is_empty());
    let total: usize = model.committed.iter().map(Vec::len).sum();
    assert_eq!(check(&store, &model, usize::MAX).await, total);
}

fn run_case(ops: Vec<Op>, pg: Option<String>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(ops, pg));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn the_sequencer_keeps_order_without_gaps_through_takeovers(
        ops in prop::collection::vec(op(), 1..30)
    ) {
        run_case(ops.clone(), None);
        if let Ok(url) = std::env::var("LOCUS_TEST_POSTGRES_URL") {
            run_case(ops, Some(url));
        }
    }
}
