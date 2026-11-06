//! Property tests over random batch sequences. Each step is checked
//! against the model and the eager tree; every `#commit` event is verified
//! from its own blocks alone (signature, ops resolve in the new tree, and
//! inverting them lands on `prevData`, as sync 1.1 consumers check); and
//! `getRepo` since every earlier rev, with what a consumer held then,
//! rebuilds the current tree.

use proptest::prelude::*;
use proto_blue_repo::BlockMap;
use proto_blue_repo::mst::Mst;

use super::*;

#[derive(Debug, Clone)]
enum Op {
    Create { coll: usize, rkey: u16, n: u64 },
    Update { pick: usize, n: u64 },
    Delete { pick: usize },
}

const COLLS: [&str; 3] = [
    "app.bsky.feed.post",
    "app.bsky.feed.like",
    "tools.kryphocron.feed.postPublic",
];

fn op() -> impl Strategy<Value = Op> {
    // Small value spaces, so records share CIDs and rkeys collide.
    prop_oneof![
        3 => (0..3usize, 0..300u16, 0..6u64).prop_map(|(coll, rkey, n)| Op::Create { coll, rkey, n }),
        1 => (any::<usize>(), 0..6u64).prop_map(|(pick, n)| Op::Update { pick, n }),
        1 => any::<usize>().prop_map(|pick| Op::Delete { pick }),
    ]
}

type Model = BTreeMap<String, (Cid, Vec<u8>)>;

/// The batch's writes against `model`, and the model after it. Ops that
/// would be refused (a create of an existing path) are skipped.
fn plan(ops: &[Op], model: &Model) -> (Vec<PreparedWrite>, Model) {
    let mut next = model.clone();
    let mut writes = Vec::new();
    for o in ops {
        match o {
            Op::Create { coll, rkey, n } => {
                let c = COLLS[*coll];
                let rkey = format!("3m{rkey:011}");
                let path = format!("{c}/{rkey}");
                if next.contains_key(&path) {
                    continue;
                }
                writes.push(write(Action::Create, c, &rkey, Some(*n)));
                next.insert(path, record(c, *n));
            }
            Op::Update { pick, .. } | Op::Delete { pick } if !next.is_empty() => {
                let path = next.keys().nth(pick % next.len()).cloned().unwrap();
                let (c, rkey) = path.split_once('/').unwrap();
                if let Op::Update { n, .. } = o {
                    writes.push(write(Action::Update, c, rkey, Some(*n)));
                    next.insert(path.clone(), record(c, *n));
                } else {
                    writes.push(write(Action::Delete, c, rkey, None));
                    next.remove(&path);
                }
            }
            _ => {}
        }
    }
    (writes, next)
}

/// Verifies a `#commit` event from its blocks alone.
fn verify_event(body: &LexValue, key: &K256Keypair) {
    let m = body.as_map().unwrap();
    let LexValue::Bytes(car) = &m["blocks"] else {
        panic!("blocks")
    };
    let (_, blocks): (_, BlockMap) = read_car(car).unwrap();
    let commit_cid = m["commit"].as_cid().unwrap();
    let commit = SignedCommit::from_lex_value(
        &proto_blue_lex_cbor::decode(blocks.get(commit_cid).expect("the commit block")).unwrap(),
    )
    .unwrap();
    assert!(verify_commit_sig(&commit, &key.did()).unwrap(), "signature");
    let ops = m["ops"].as_array().unwrap();
    let mut tree = Mst::load(commit.data.clone());
    // Each path's last op is what the new tree holds (a batch may name a
    // path twice).
    let mut last: BTreeMap<&str, &BTreeMap<String, LexValue>> = BTreeMap::new();
    for o in ops {
        let o = o.as_map().unwrap();
        last.insert(o["path"].as_str().unwrap(), o);
    }
    for (path, o) in &last {
        let now = tree
            .get(path, &blocks)
            .expect("a covering proof for each op");
        match o["action"].as_str().unwrap() {
            "delete" => assert_eq!(now, None, "{path} gone"),
            _ => assert_eq!(now.as_ref(), o["cid"].as_cid(), "{path} holds its cid"),
        }
    }
    // Invert the ops, last first, back to the previous data root.
    for o in ops.iter().rev() {
        let o = o.as_map().unwrap();
        let path = o["path"].as_str().unwrap();
        let prev = o.get("prev").and_then(LexValue::as_cid).cloned();
        tree = match (o["action"].as_str().unwrap(), prev) {
            ("create", _) => tree.delete(path, &blocks),
            ("update", Some(p)) => tree.update(path, p, &blocks),
            ("delete", Some(p)) => tree.add(path, p, &blocks),
            (a, p) => panic!("{a} {path} without prev ({p:?})"),
        }
        .unwrap_or_else(|e| panic!("inverting {path}: {e}"));
    }
    assert_eq!(
        Some(&tree.root_cid().unwrap()),
        m["prevData"].as_cid(),
        "the inverted tree is prevData"
    );
}

async fn all_blocks(store: &Store, since: Option<&str>) -> BlockMap {
    let mut out = BlockMap::new();
    for (cid, bytes) in store
        .repo_blocks_page(DID, None, since, 1_000_000)
        .await
        .unwrap()
    {
        out.set(Cid::from_str_multibase(&cid).unwrap(), bytes);
    }
    out
}

async fn run_case(batches: Vec<Vec<Op>>) {
    let dir = tempfile::tempdir().unwrap();
    let b = Backend {
        name: "sqlite",
        store: Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
            .await
            .unwrap(),
        _dir: None,
    };
    let key = K256Keypair::generate();
    let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(1)));
    engine.create_repo(DID, &key).await.unwrap();
    let mut model = Model::new();
    // What a consumer held at each rev: (rev, every block then).
    let mut held: Vec<(String, BlockMap)> = vec![(
        root_of(&b.store).await.rev,
        all_blocks(&b.store, None).await,
    )];
    for (round, ops) in batches.iter().enumerate() {
        let (writes, next) = plan(ops, &model);
        if writes.is_empty() {
            continue;
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
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
        model = next;
        check_against_model(&b, &model, round, &key).await;

        let events = b.store.outbox_for(DID).await.unwrap();
        let (kind, payload) = events.last().unwrap();
        assert_eq!(kind, "commit");
        verify_event(&proto_blue_lex_cbor::decode(payload).unwrap(), &key);

        // getRepo since each earlier rev, plus what was held then, gives
        // the whole current tree.
        let root = root_of(&b.store).await;
        let data = Cid::from_str_multibase(&root.data_cid).unwrap();
        let want: Vec<(String, Cid)> = model
            .iter()
            .map(|(p, (c, _))| (p.clone(), c.clone()))
            .collect();
        for (rev, then) in &held {
            let mut have = then.clone();
            let since = all_blocks(&b.store, Some(rev)).await;
            for (cid, bytes) in since.iter() {
                have.set(cid.clone(), bytes.to_vec());
            }
            let got = Mst::load(data.clone())
                .leaves(&have)
                .unwrap_or_else(|e| panic!("round {round}: since {rev} misses a block: {e}"));
            assert_eq!(got, want, "round {round}: since {rev}");
        }
        held.push((root.rev, all_blocks(&b.store, None).await));
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn random_histories_verify_from_their_events_and_since_every_rev(
        batches in prop::collection::vec(prop::collection::vec(op(), 1..7), 1..10)
    ) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_case(batches));
    }
}

/// The lazy tree on its own, for the history the property test shrank to:
/// is the canonical root reached in memory?
#[test]
fn lazy_tree_matches_the_eager_tree_for_the_shrunk_history() {
    use proto_blue_repo::mst::{MstOp, NoBlocks};
    let p = |r: &str| format!("tools.kryphocron.feed.postPublic/3m{r}");
    let (a, b, c) = (p("00000000033"), p("00000000058"), p("00000000292"));
    let v = |n: u64| record("tools.kryphocron.feed.postPublic", n).0;
    let one = Mst::empty().add(&a, v(0), &NoBlocks).unwrap();
    let ops = vec![
        MstOp::Delete { key: a.clone() },
        MstOp::Add {
            key: b.clone(),
            value: v(4),
        },
        MstOp::Add {
            key: c.clone(),
            value: v(1),
        },
    ];
    let lazy = one.apply(&ops, &NoBlocks).unwrap().root_cid().unwrap();
    let step = one
        .delete(&a, &NoBlocks)
        .unwrap()
        .add(&b, v(4), &NoBlocks)
        .unwrap()
        .add(&c, v(1), &NoBlocks)
        .unwrap()
        .root_cid()
        .unwrap();
    let eager = MstNode::empty()
        .add(&b, v(4))
        .unwrap()
        .add(&c, v(1))
        .unwrap()
        .get_all_blocks()
        .unwrap()
        .0;
    assert_eq!(step, eager, "one op at a time");
    assert_eq!(lazy, eager, "as a batch");
}
