//! Engine tests against SQLite, and Postgres when `LOCUS_TEST_POSTGRES_URL`
//! is set. Every commit is checked against an in-memory model and against
//! proto-blue's eager MST built from scratch (the oracle).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use locus_core::bootstrap::DatabaseUrl;
use locus_store::Store;
use proto_blue_crypto::{K256Keypair, Keypair};
use proto_blue_lex_data::{Cid, LexValue};
use proto_blue_repo::car::read_car;
use proto_blue_repo::commit::{SignedCommit, verify_commit_sig};
use proto_blue_repo::mst::MstNode;

use crate::engine::{Action, CommitError, CommitRequest, PreparedWrite, RepoEngine};
use crate::tid::TidClock;

const DID: &str = "did:plc:testrepo0000000000000000";

struct Backend {
    name: &'static str,
    store: Store,
    _dir: Option<tempfile::TempDir>,
}

async fn backends() -> Vec<Backend> {
    let mut out = Vec::new();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
        .await
        .unwrap();
    out.push(Backend {
        name: "sqlite",
        store,
        _dir: Some(dir),
    });
    if let Ok(admin_url) = std::env::var("LOCUS_TEST_POSTGRES_URL") {
        let admin = sqlx::PgPool::connect(&admin_url).await.unwrap();
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
        out.push(Backend {
            name: "postgres",
            store,
            _dir: None,
        });
    }
    out
}

fn record(collection: &str, n: u64) -> (Cid, Vec<u8>) {
    let mut m = BTreeMap::new();
    m.insert("$type".to_owned(), LexValue::String(collection.to_owned()));
    m.insert("n".to_owned(), LexValue::Integer(n as i64));
    let v = LexValue::Map(m);
    let bytes = proto_blue_lex_cbor::encode(&v).unwrap();
    (Cid::for_cbor(&bytes), bytes)
}

fn write(action: Action, collection: &str, rkey: &str, n: Option<u64>) -> PreparedWrite {
    PreparedWrite {
        action,
        collection: collection.to_owned(),
        rkey: rkey.to_owned(),
        record: n.map(|n| record(collection, n)),
        swap_record: None,
        blobs: Vec::new(),
        backlinks: Vec::new(),
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

async fn root_of(store: &Store) -> locus_store::repo::RepoRoot {
    let mut tx = store.begin().await.unwrap();
    let r = tx.repo_root(DID).await.unwrap().unwrap();
    tx.rollback().await.unwrap();
    r
}

/// Checks the stored repository against the model: data root equals the
/// eager MST of the model, and live blocks with their counts are exactly the
/// tree's nodes (1 each), the records' blocks (one per path holding the CID)
/// and the current commit (1).
async fn check_against_model(
    b: &Backend,
    model: &BTreeMap<String, (Cid, Vec<u8>)>,
    round: usize,
    key: &K256Keypair,
) {
    let mut eager = MstNode::empty();
    for (path, (cid, _)) in model {
        eager = eager.add(path, cid.clone()).unwrap();
    }
    let (eager_root, eager_blocks) = eager.get_all_blocks().unwrap();
    let root = root_of(&b.store).await;
    assert_eq!(
        root.data_cid,
        eager_root.to_string_base32(),
        "{} round {round}: data root",
        b.name
    );

    let mut want: BTreeMap<String, i64> = BTreeMap::new();
    for cid in eager_blocks.cids() {
        *want.entry(cid.to_string_base32()).or_default() += 1;
    }
    for (cid, _) in model.values() {
        *want.entry(cid.to_string_base32()).or_default() += 1;
    }
    *want.entry(root.commit_cid.clone()).or_default() += 1;
    let got: BTreeMap<String, i64> = b
        .store
        .repo_block_counts(DID)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        got, want,
        "{} round {round}: live blocks and counts",
        b.name
    );

    // The commit is signed by the account key and points at the data root.
    let mut tx = b.store.begin().await.unwrap();
    let bytes = tx.repo_block(DID, &root.commit_cid).await.unwrap().unwrap();
    tx.rollback().await.unwrap();
    let commit =
        SignedCommit::from_lex_value(&proto_blue_lex_cbor::decode(&bytes).unwrap()).unwrap();
    assert!(
        verify_commit_sig(&commit, &key.did()).unwrap(),
        "{} round {round}: signature",
        b.name
    );
    assert_eq!(commit.prev, None);
    assert_eq!(commit.rev, root.rev);
}

#[tokio::test]
async fn random_batches_match_the_model_and_the_eager_tree() {
    for b in backends().await {
        let key = K256Keypair::generate();
        let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(1)));
        engine.create_repo(DID, &key).await.unwrap();
        let mut model: BTreeMap<String, (Cid, Vec<u8>)> = BTreeMap::new();
        check_against_model(&b, &model, 0, &key).await;

        let mut rng = Rng(0x5eed_1234_abcd_ef01);
        let mut revs = BTreeSet::new();
        for round in 1..=50 {
            let mut writes = Vec::new();
            let mut next = model.clone();
            for _ in 0..(1 + rng.below(8)) {
                let coll = [
                    "app.bsky.feed.post",
                    "app.bsky.feed.like",
                    "tools.kryphocron.feed.postPublic",
                ][rng.below(3) as usize];
                let roll = rng.below(4);
                // Small value space so records often share a CID.
                let n = rng.below(6);
                if roll <= 1 || next.is_empty() {
                    let rkey = format!("3m{:011}", rng.below(5_000));
                    let path = format!("{coll}/{rkey}");
                    if next.contains_key(&path) {
                        continue;
                    }
                    writes.push(write(Action::Create, coll, &rkey, Some(n)));
                    next.insert(path, record(coll, n));
                } else {
                    let paths: Vec<String> = next.keys().cloned().collect();
                    let path = paths[rng.below(paths.len() as u64) as usize].clone();
                    let (c, rkey) = path.split_once('/').unwrap();
                    if roll == 2 {
                        writes.push(write(Action::Update, c, rkey, Some(n)));
                        next.insert(path.clone(), record(c, n));
                    } else {
                        writes.push(write(Action::Delete, c, rkey, None));
                        next.remove(&path);
                    }
                }
            }
            if writes.is_empty() {
                continue;
            }
            let out = engine
                .commit(CommitRequest {
                    did: DID,
                    writes,
                    swap_commit: None,
                    signer: &key,
                    dedupe_backlinks: false,
                })
                .await
                .unwrap_or_else(|e| panic!("{} round {round}: {e}", b.name));
            assert!(revs.insert(out.rev.clone()), "rev reused");
            model = next;
            check_against_model(&b, &model, round, &key).await;
        }
        // Revs only ever increased.
        let ordered: Vec<_> = revs.iter().cloned().collect();
        assert_eq!(ordered.len(), revs.len());
    }
}

#[tokio::test]
async fn swaps_and_conflicts_are_refused_without_writing() {
    for b in backends().await {
        let key = K256Keypair::generate();
        let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(2)));
        let first = engine.create_repo(DID, &key).await.unwrap();
        let c = |writes| CommitRequest {
            did: DID,
            writes,
            swap_commit: None,
            signer: &key,
            dedupe_backlinks: false,
        };
        engine
            .commit(c(vec![write(Action::Create, "a.b.c", "one", Some(1))]))
            .await
            .unwrap();
        let before = root_of(&b.store).await;

        // swapCommit at an old commit.
        let res = engine
            .commit(CommitRequest {
                did: DID,
                writes: vec![write(Action::Create, "a.b.c", "two", Some(2))],
                swap_commit: Some(first.commit_cid.clone()),
                signer: &key,
                dedupe_backlinks: false,
            })
            .await;
        assert!(
            matches!(res, Err(CommitError::InvalidSwap(_))),
            "{}",
            b.name
        );

        // swapRecord mismatch, and create-requires-empty.
        let mut w = write(Action::Update, "a.b.c", "one", Some(3));
        w.swap_record = Some(Some(record("a.b.c", 9).0));
        assert!(matches!(
            engine.commit(c(vec![w])).await,
            Err(CommitError::InvalidSwap(_))
        ));
        let mut w = write(Action::Create, "a.b.c", "one", Some(3));
        w.swap_record = Some(None);
        assert!(matches!(
            engine.commit(c(vec![w])).await,
            Err(CommitError::InvalidSwap(_))
        ));

        // Conflicts with the index are 400s, not 500s.
        for w in [
            write(Action::Create, "a.b.c", "one", Some(4)),
            write(Action::Update, "a.b.c", "missing", Some(4)),
            write(Action::Delete, "a.b.c", "missing", None),
        ] {
            assert!(
                matches!(
                    engine.commit(c(vec![w])).await,
                    Err(CommitError::InvalidRequest(_))
                ),
                "{}",
                b.name
            );
        }
        // Nothing was written by any refused commit.
        assert_eq!(root_of(&b.store).await, before, "{}", b.name);
    }
}

#[tokio::test]
async fn a_batch_that_nets_out_still_commits_with_both_ops() {
    for b in backends().await {
        let key = K256Keypair::generate();
        let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(3)));
        engine.create_repo(DID, &key).await.unwrap();
        let before = root_of(&b.store).await;
        let out = engine
            .commit(CommitRequest {
                did: DID,
                writes: vec![
                    write(Action::Create, "a.b.c", "x", Some(1)),
                    write(Action::Delete, "a.b.c", "x", None),
                ],
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
            .await
            .unwrap();
        let after = root_of(&b.store).await;
        assert_eq!(
            after.data_cid, before.data_cid,
            "{}: tree unchanged",
            b.name
        );
        assert!(after.rev > before.rev, "{}: new rev", b.name);
        assert_eq!(after.commit_cid, out.commit_cid.to_string_base32());
        check_against_model(&b, &BTreeMap::new(), 1, &key).await;

        let events = b.store.outbox_for(DID).await.unwrap();
        let body = proto_blue_lex_cbor::decode(&events.last().unwrap().1).unwrap();
        let ops = body.as_map().unwrap()["ops"].as_array().unwrap();
        assert_eq!(ops.len(), 2, "{}", b.name);
    }
}

#[tokio::test]
async fn commit_events_carry_what_consumers_need() {
    for b in backends().await {
        let key = K256Keypair::generate();
        let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(4)));
        let created = engine.create_repo(DID, &key).await.unwrap();
        let mut writes = Vec::new();
        for i in 0..40 {
            writes.push(write(
                Action::Create,
                "app.bsky.feed.post",
                &format!("3m{i:011}"),
                Some(i),
            ));
        }
        let first = engine
            .commit(CommitRequest {
                did: DID,
                writes,
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
            .await
            .unwrap();
        let second = engine
            .commit(CommitRequest {
                did: DID,
                writes: vec![
                    write(
                        Action::Update,
                        "app.bsky.feed.post",
                        "3m00000000007",
                        Some(99),
                    ),
                    write(Action::Delete, "app.bsky.feed.post", "3m00000000020", None),
                ],
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
            .await
            .unwrap();

        let events = b.store.outbox_for(DID).await.unwrap();
        assert_eq!(events.len(), 3, "{}: create + 2 commits", b.name);
        let first_body = proto_blue_lex_cbor::decode(&events[0].1).unwrap();
        assert!(
            first_body.as_map().unwrap().get("prevData").is_none(),
            "first commit has no prevData"
        );
        assert_eq!(first_body.as_map().unwrap()["since"], LexValue::Null);
        let _ = created;

        let body = proto_blue_lex_cbor::decode(&events[2].1).unwrap();
        let m = body.as_map().unwrap();
        assert_eq!(m["commit"], LexValue::Cid(second.commit_cid.clone()));
        assert_eq!(m["since"], LexValue::String(first.rev.clone()));
        assert!(matches!(m["prevData"], LexValue::Cid(_)));
        let ops = m["ops"].as_array().unwrap();
        let update = ops[0].as_map().unwrap();
        assert_eq!(update["action"], LexValue::String("update".into()));
        assert_eq!(
            update["prev"],
            LexValue::Cid(record("app.bsky.feed.post", 7).0)
        );
        let delete = ops[1].as_map().unwrap();
        assert_eq!(delete["cid"], LexValue::Null);

        // The CAR is rooted at the commit and holds it, the new record, and
        // enough of the tree to check both ops.
        let LexValue::Bytes(car) = &m["blocks"] else {
            panic!("blocks not bytes")
        };
        let (roots, blocks) = read_car(car).unwrap();
        assert_eq!(roots, vec![second.commit_cid.clone()]);
        assert!(blocks.has(&second.commit_cid));
        assert!(blocks.has(&record("app.bsky.feed.post", 99).0));
        let commit = SignedCommit::from_lex_value(
            &proto_blue_lex_cbor::decode(blocks.get(&second.commit_cid).unwrap()).unwrap(),
        )
        .unwrap();
        let tree = proto_blue_repo::mst::Mst::load(commit.data.clone());
        assert_eq!(
            tree.get("app.bsky.feed.post/3m00000000007", &blocks)
                .unwrap(),
            Some(record("app.bsky.feed.post", 99).0),
            "{}: update provable from the event alone",
            b.name
        );
        assert_eq!(
            tree.get("app.bsky.feed.post/3m00000000020", &blocks)
                .unwrap(),
            None,
            "{}: delete provable from the event alone",
            b.name
        );
    }
}

#[tokio::test]
async fn gc_removes_only_unreferenced_blocks() {
    for b in backends().await {
        let key = K256Keypair::generate();
        let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(5)));
        engine.create_repo(DID, &key).await.unwrap();
        let mut model = BTreeMap::new();
        let mut writes = Vec::new();
        for i in 0..30 {
            let rkey = format!("3m{i:011}");
            writes.push(write(Action::Create, "a.b.c", &rkey, Some(i)));
            model.insert(format!("a.b.c/{rkey}"), record("a.b.c", i));
        }
        engine
            .commit(CommitRequest {
                did: DID,
                writes,
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
            .await
            .unwrap();
        let mut writes = Vec::new();
        for i in 0..15 {
            let rkey = format!("3m{i:011}");
            writes.push(write(Action::Delete, "a.b.c", &rkey, None));
            model.remove(&format!("a.b.c/{rkey}"));
        }
        engine
            .commit(CommitRequest {
                did: DID,
                writes,
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
            .await
            .unwrap();

        // Within the grace period nothing goes; after it, only zero-count blocks.
        assert_eq!(
            b.store.gc_zero_blocks(60_000, 1_000).await.unwrap(),
            0,
            "{}",
            b.name
        );
        let removed = b.store.gc_zero_blocks(-1, 1_000).await.unwrap();
        assert!(
            removed > 15,
            "{}: deleted records, old nodes and the old commit go",
            b.name
        );
        check_against_model(&b, &model, 2, &key).await;

        // Re-creating a deleted record re-creates its block row.
        engine
            .commit(CommitRequest {
                did: DID,
                writes: vec![write(Action::Create, "a.b.c", "3m00000000000", Some(0))],
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
            .await
            .unwrap();
        model.insert("a.b.c/3m00000000000".into(), record("a.b.c", 0));
        check_against_model(&b, &model, 3, &key).await;
    }
}

#[tokio::test]
async fn concurrent_commits_to_one_repo_serialize() {
    for b in backends().await {
        let key = Arc::new(K256Keypair::generate());
        let engine = Arc::new(RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(6))));
        engine.create_repo(DID, key.as_ref()).await.unwrap();
        let mut handles = Vec::new();
        for i in 0..20u64 {
            let (engine, key) = (engine.clone(), key.clone());
            handles.push(tokio::spawn(async move {
                engine
                    .commit(CommitRequest {
                        did: DID,
                        writes: vec![write(
                            Action::Create,
                            "a.b.c",
                            &format!("3m{i:011}"),
                            Some(i),
                        )],
                        swap_commit: None,
                        signer: key.as_ref(),
                        dedupe_backlinks: false,
                    })
                    .await
                    .unwrap()
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        let model: BTreeMap<_, _> = (0..20u64)
            .map(|i| (format!("a.b.c/3m{i:011}"), record("a.b.c", i)))
            .collect();
        check_against_model(&b, &model, 1, &key).await;
        assert_eq!(
            b.store.outbox_for(DID).await.unwrap().len(),
            21,
            "{}",
            b.name
        );
    }
}
