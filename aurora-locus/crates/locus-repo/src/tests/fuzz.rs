//! Robustness of the parsers that read uploads: the streaming CAR reader,
//! the bounded tree walk, record decoding, and whole imports of mutated
//! CARs. Arbitrary and mutated input must fail with a refusal (400), never
//! a panic or an internal error, and a failed import leaves the repo as it
//! was.

use proptest::prelude::*;
use proto_blue_repo::commit::{UnsignedCommit, sign_commit};

use super::*;
use crate::import::{CarReader, ImportError, TreeWalk, decode_record, import_blob_cids};

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// Reads every block; returns how many, or the refusal.
async fn read_all(bytes: &[u8]) -> Result<usize, ImportError> {
    let (mut r, _) = CarReader::open(bytes).await?;
    let mut n = 0;
    while r.next_block().await?.is_some() {
        n += 1;
    }
    Ok(n)
}

fn refused<T: std::fmt::Debug>(r: &Result<T, ImportError>) -> bool {
    matches!(r, Err(ImportError::Invalid(_)))
}

/// A small valid CAR: a commit over a tree of `n` records.
fn valid_car(key: &K256Keypair, n: u64) -> Vec<u8> {
    let (root, blocks) = valid_blocks(key, n);
    proto_blue_repo::car::blocks_to_car(Some(&root), &blocks).unwrap()
}

/// The commit CID and every block of [`valid_car`].
fn valid_blocks(key: &K256Keypair, n: u64) -> (Cid, proto_blue_repo::BlockMap) {
    let mut eager = MstNode::empty();
    let mut blocks = proto_blue_repo::BlockMap::new();
    for i in 0..n {
        let (cid, bytes) = record("app.bsky.feed.post", i);
        eager = eager
            .add(&format!("app.bsky.feed.post/3m{i:011}"), cid.clone())
            .unwrap();
        blocks.set(cid, bytes);
    }
    let (data, nodes) = eager.get_all_blocks().unwrap();
    blocks.add_map(&nodes);
    let commit = sign_commit(
        &UnsignedCommit::new(DID.to_owned(), data, "2222222222222".into(), None),
        key,
    )
    .unwrap();
    let root = commit.cid().unwrap();
    blocks.set(root.clone(), commit.to_cbor().unwrap());
    (root, blocks)
}

#[derive(Debug, Clone)]
enum Mutation {
    Flip(usize, u8),
    Truncate(usize),
    Insert(usize, u8),
    Delete(usize),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        4 => (any::<usize>(), 1..=255u8).prop_map(|(i, x)| Mutation::Flip(i, x)),
        1 => any::<usize>().prop_map(Mutation::Truncate),
        1 => (any::<usize>(), any::<u8>()).prop_map(|(i, x)| Mutation::Insert(i, x)),
        1 => any::<usize>().prop_map(Mutation::Delete),
    ]
}

fn mutate(mut bytes: Vec<u8>, ms: &[Mutation]) -> Vec<u8> {
    for m in ms {
        let len = bytes.len().max(1);
        match *m {
            Mutation::Flip(i, x) if !bytes.is_empty() => bytes[i % len] ^= x,
            Mutation::Truncate(i) => bytes.truncate(i % len),
            Mutation::Insert(i, x) => bytes.insert(i % (bytes.len() + 1), x),
            Mutation::Delete(i) if !bytes.is_empty() => {
                bytes.remove(i % len);
            }
            _ => {}
        }
    }
    bytes
}

/// A node-shaped value with arbitrary contents: prefixes past the
/// previous key, unordered or invalid keys, links that aren't CIDs.
fn node_value() -> impl Strategy<Value = LexValue> {
    let link = prop_oneof![
        any::<u8>().prop_map(|n| LexValue::Cid(Cid::for_cbor(&[n]))),
        Just(LexValue::Null),
        any::<i64>().prop_map(LexValue::Integer),
        Just(LexValue::Bytes(vec![1, 2, 3])),
    ];
    let key = prop_oneof![
        "[a-z.]{1,8}/[a-z0-9]{1,6}".prop_map(String::into_bytes),
        prop::collection::vec(any::<u8>(), 0..12),
    ];
    let entry = (any::<i64>(), key, link.clone(), prop::option::of(link.clone())).prop_map(
        |(p, k, v, t)| {
            let mut m = BTreeMap::new();
            m.insert("p".to_owned(), LexValue::Integer(p % 20));
            m.insert("k".to_owned(), LexValue::Bytes(k));
            m.insert("v".to_owned(), v);
            m.insert("t".to_owned(), t.unwrap_or(LexValue::Null));
            LexValue::Map(m)
        },
    );
    (
        prop::option::of(link),
        prop::collection::vec(entry, 0..6),
        any::<bool>(),
    )
        .prop_map(|(l, e, extra)| {
            let mut m = BTreeMap::new();
            m.insert("l".to_owned(), l.unwrap_or(LexValue::Null));
            m.insert("e".to_owned(), LexValue::Array(e));
            if extra {
                m.insert("x".to_owned(), LexValue::Integer(1));
            }
            LexValue::Map(m)
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn the_car_reader_refuses_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        let r = block_on(read_all(&bytes));
        prop_assert!(r.is_ok() || refused(&r), "{r:?}");
    }

    #[test]
    fn the_car_reader_refuses_mutated_cars(ms in prop::collection::vec(mutation(), 1..4)) {
        let car = mutate(valid_car(&K256Keypair::generate(), 3), &ms);
        let r = block_on(read_all(&car));
        prop_assert!(r.is_ok() || refused(&r), "{r:?}");
    }

    #[test]
    fn the_tree_walk_refuses_arbitrary_nodes(
        node in node_value(),
        raw in prop::collection::vec(any::<u8>(), 0..64),
    ) {
        for bytes in [proto_blue_lex_cbor::encode(&node).unwrap(), raw] {
            let mut walk = TreeWalk::new(Cid::for_cbor(&bytes));
            walk.next_node().unwrap().unwrap();
            match walk.visit(&bytes) {
                // What it accepts is a well-formed node: keys in order.
                Ok(leaves) => prop_assert!(leaves.windows(2).all(|w| w[0].0 < w[1].0)),
                Err(e) => prop_assert!(matches!(e, ImportError::Invalid(_)), "{e:?}"),
            }
        }
    }

    #[test]
    fn record_decoding_refuses_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        match decode_record("app.bsky.feed.post", "x", &bytes) {
            Ok(v) => {
                let _ = import_blob_cids(&v);
            }
            Err(e) => prop_assert!(matches!(e, ImportError::Invalid(_)), "{e:?}"),
        }
    }
}

/// Claimed lengths far past the limits are refused before anything is
/// allocated for them.
#[tokio::test]
async fn huge_claimed_lengths_are_refused_up_front() {
    let huge = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
    assert!(refused(&read_all(&huge).await), "header");
    let mut car = crate::car::header(None);
    car.extend_from_slice(&huge);
    assert!(refused(&read_all(&car).await), "block");
    // Eleven continuation bytes: not a varint.
    assert!(refused(&read_all(&[0x80; 11]).await));
}

/// Imports the CAR `make` builds (signed by the account's key) over an
/// empty repo: it completes, or is refused and rolled back.
async fn import_case(make: impl FnOnce(&K256Keypair) -> Vec<u8>, ms: &dyn std::fmt::Debug) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
        .await
        .unwrap();
    let mut tx = store.begin().await.unwrap();
    tx.insert_account(&locus_store::accounts::NewAccount {
        did: DID,
        handle: "fuzz.test",
        email: None,
        password_hash: None,
        status: locus_store::accounts::Status::Active,
        did_method: "plc",
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let key = K256Keypair::generate();
    let engine = RepoEngine::new(store.clone(), Arc::new(TidClock::new(2)));
    engine.create_repo(DID, &key).await.unwrap();
    let before = root_of(&store).await;
    let car = make(&key);
    let path = dir.path().join("in.car");
    std::fs::write(&path, &car).unwrap();
    let job = engine.start_import(DID, "node-a").await.unwrap();
    match engine
        .run_import(&job, &path, Some(&key.did()), &key, true)
        .await
    {
        // A harmless mutation (say, in the signature, which is noted and
        // re-signed): the import completes.
        Ok(_) => {
            engine.finish_import(&job).await.unwrap();
        }
        Err(e) => {
            assert!(matches!(e, ImportError::Invalid(_)), "{ms:?}: {e:?}");
            assert_eq!(root_of(&store).await, before, "{ms:?}: rolled back");
            let mut tx = store.begin().await.unwrap();
            assert!(!tx.write_blocked(DID).await.unwrap(), "{ms:?}: unblocked");
            tx.rollback().await.unwrap();
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn mutated_imports_are_refused_and_leave_the_repo(ms in prop::collection::vec(mutation(), 1..3)) {
        block_on(import_case(|key| mutate(valid_car(key, 6), &ms), &ms));
    }

    /// Well-formed CARs missing blocks, with blocks repeated, or with a
    /// different root: refused (or, for repeats, imported), never an
    /// internal error.
    #[test]
    fn imports_missing_or_repeating_blocks_are_refused(
        drop in prop::collection::vec(any::<bool>(), 16),
        repeat in prop::option::of(any::<usize>()),
        reroot in any::<bool>(),
    ) {
        block_on(import_case(
            |key| {
                let (root, blocks) = valid_blocks(key, 6);
                let entries: Vec<(Cid, Vec<u8>)> =
                    blocks.iter().map(|(c, b)| (c.clone(), b.to_vec())).collect();
                let root = if reroot { entries[0].0.clone() } else { root };
                let mut car = crate::car::header(Some(&root));
                for (i, (c, b)) in entries.iter().enumerate() {
                    if !drop[i % drop.len()] {
                        car.extend(crate::car::block(c, b));
                    }
                }
                if let Some(r) = repeat {
                    let (c, b) = &entries[r % entries.len()];
                    car.extend(crate::car::block(c, b));
                }
                car
            },
            &(&drop, repeat, reroot),
        ));
    }
}

/// A node whose keys sit on different layers, and one claiming a subtree
/// below layer 0, are refused by the walk.
#[test]
fn the_tree_walk_refuses_nodes_off_their_layer() {
    use proto_blue_repo::mst::leading_zeros_on_hash;
    // Two keys on different layers.
    let keys: Vec<String> = (0..200)
        .map(|i| format!("app.bsky.feed.post/3m{i:011}"))
        .collect();
    let low = keys.iter().find(|k| leading_zeros_on_hash(k) == 0).unwrap();
    let high = keys.iter().find(|k| leading_zeros_on_hash(k) > 0).unwrap();
    let (mut a, mut b) = (low.clone(), high.clone());
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }
    let leaf = record("app.bsky.feed.post", 1).0;
    let entry = |k: &str, t: LexValue| {
        let mut m = BTreeMap::new();
        m.insert("p".to_owned(), LexValue::Integer(0));
        m.insert("k".to_owned(), LexValue::Bytes(k.as_bytes().to_vec()));
        m.insert("v".to_owned(), LexValue::Cid(leaf.clone()));
        m.insert("t".to_owned(), t);
        LexValue::Map(m)
    };
    let node = |e: Vec<LexValue>| {
        let mut m = BTreeMap::new();
        m.insert("l".to_owned(), LexValue::Null);
        m.insert("e".to_owned(), LexValue::Array(e));
        proto_blue_lex_cbor::encode(&LexValue::Map(m)).unwrap()
    };
    let visit = |bytes: &[u8]| {
        let mut walk = TreeWalk::new(Cid::for_cbor(bytes));
        walk.next_node().unwrap();
        walk.visit(bytes)
    };
    let mixed = node(vec![entry(&a, LexValue::Null), entry(&b, LexValue::Null)]);
    assert!(refused(&visit(&mixed)), "keys on two layers");
    let below_zero = node(vec![entry(low, LexValue::Cid(leaf.clone()))]);
    assert!(refused(&visit(&below_zero)), "subtree under layer 0");
}
