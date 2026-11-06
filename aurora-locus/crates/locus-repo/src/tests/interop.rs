//! The atproto interop fixtures (`interop-test-files/` at the workspace
//! root) run through locus's own code: record keys and key types, record
//! JSON parsing and preparation, and the commit-proof trees through the
//! commit pipeline, its `#commit` event, the bounded tree walk and the
//! streaming CAR reader.

use base64::Engine as _;
use serde_json::Value;

use super::props::verify_event_opts;
use proto_blue_repo::car::{ReadCarOpts, read_car_opts};
use super::*;
use crate::import::{CarReader, TreeWalk};
use crate::records::{check_key_type, check_rkey, parse_json, prepare_record};

fn fixture(path: &str) -> String {
    let path = format!(
        "{}/../../interop-test-files/{path}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The non-comment lines of a syntax file, untrimmed.
fn lines(file: &str) -> Vec<String> {
    fixture(&format!("syntax/{file}"))
        .split('\n')
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

fn json(path: &str) -> Value {
    serde_json::from_str(&fixture(path)).unwrap()
}

#[test]
fn record_key_and_key_type_vectors() {
    for k in lines("recordkey_syntax_valid.txt") {
        assert_eq!(check_rkey(&k), Ok(()), "{k:?}");
        assert!(check_key_type("c", Some("any"), &k).is_ok());
    }
    for k in lines("recordkey_syntax_invalid.txt") {
        assert!(check_rkey(&k).is_err(), "{k:?} accepted");
    }
    for t in lines("tid_syntax_valid.txt") {
        assert!(check_key_type("c", Some("tid"), &t).is_ok(), "{t:?}");
    }
    for t in lines("tid_syntax_invalid.txt") {
        assert!(check_key_type("c", Some("tid"), &t).is_err(), "{t:?}");
    }
    for n in lines("nsid_syntax_valid.txt") {
        assert!(check_key_type("c", Some("nsid"), &n).is_ok(), "{n:?}");
    }
    for n in lines("nsid_syntax_invalid.txt") {
        assert!(check_key_type("c", Some("nsid"), &n).is_err(), "{n:?}");
    }
}

#[test]
fn data_model_fixtures_encode_to_their_cbor_and_cid() {
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD;
    let fixtures = json("data-model/data-model-fixtures.json");
    let fixtures = fixtures.as_array().unwrap();
    assert!(fixtures.len() >= 3);
    for f in fixtures {
        let value = parse_json(&f["json"]).unwrap();
        let want = b64.decode(f["cbor_base64"].as_str().unwrap()).unwrap();
        let bytes = proto_blue_lex_cbor::encode(&value).unwrap();
        assert_eq!(bytes, want, "{}", f["cid"]);
        assert_eq!(
            Cid::for_cbor(&bytes).to_string_base32(),
            f["cid"].as_str().unwrap()
        );
        // And the record path: its prepared bytes decode to the same value.
        let p = prepare_record("com.example.blah", "x", value.clone(), Some(false), 1 << 20)
            .unwrap();
        let mut with_type = value.as_map().unwrap().clone();
        with_type.insert("$type".into(), LexValue::String("com.example.blah".into()));
        assert_eq!(
            proto_blue_lex_cbor::decode(&p.bytes).unwrap(),
            LexValue::Map(with_type)
        );
    }
}

#[test]
fn data_model_valid_and_invalid_records() {
    let prep = |v: &Value| {
        prepare_record("com.example.blah", "x", parse_json(v)?, None, 1 << 20).map(|_| ())
    };
    for f in json("data-model/data-model-valid.json").as_array().unwrap() {
        assert_eq!(prep(&f["json"]), Ok(()), "{}", f["note"]);
    }
    for f in json("data-model/data-model-invalid.json").as_array().unwrap() {
        assert!(prep(&f["json"]).is_err(), "{} accepted", f["note"]);
    }
}

fn cid(v: &Value) -> Cid {
    Cid::from_str_multibase(v.as_str().unwrap()).unwrap()
}

async fn root_for(store: &Store, did: &str) -> locus_store::repo::RepoRoot {
    let mut tx = store.begin().await.unwrap();
    let r = tx.repo_root(did).await.unwrap().unwrap();
    tx.rollback().await.unwrap();
    r
}

fn leaf_write(action: Action, key: &str, leaf: &Cid) -> PreparedWrite {
    let (collection, rkey) = key.split_once('/').unwrap();
    PreparedWrite {
        action,
        collection: collection.to_owned(),
        rkey: rkey.to_owned(),
        // Only the CID enters the tree; the bytes are a stand-in.
        record: (action != Action::Delete).then(|| (leaf.clone(), b"leaf".to_vec())),
        swap_record: None,
        blobs: Vec::new(),
        backlinks: Vec::new(),
        lenient_blobs: false,
    }
}

/// Each commit-proof case: the keys committed give `rootBeforeCommit`; the
/// adds and deletes give `rootAfterCommit`; the `#commit` event carries
/// every block of `blocksInProof` and verifies from its blocks alone; the
/// bounded walk over the stored tree yields exactly the final keys; and a
/// CAR of the commit and tree reads back through the streaming reader.
#[tokio::test]
async fn commit_proof_fixtures_through_the_commit_pipeline() {
    let cases = json("firehose/commit-proof-fixtures.json");
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 6);
    for b in backends().await {
        for (i, case) in cases.iter().enumerate() {
            let name = format!("{} {}", b.name, case["comment"]);
            let did = format!("did:plc:proof{i:021}");
            let key = K256Keypair::generate();
            let engine = RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(1)));
            engine.create_repo(&did, &key).await.unwrap();
            let leaf = cid(&case["leafValue"]);
            let strs = |k: &str| -> Vec<String> {
                case[k]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect()
            };
            let (keys, adds, dels) = (strs("keys"), strs("adds"), strs("dels"));
            let commit = |writes: Vec<PreparedWrite>| {
                engine.commit(CommitRequest {
                    did: &did,
                    writes,
                    swap_commit: None,
                    signer: &key,
                    dedupe_backlinks: false,
                })
            };
            commit(
                keys.iter()
                    .map(|k| leaf_write(Action::Create, k, &leaf))
                    .collect(),
            )
            .await
            .unwrap();
            assert_eq!(
                root_for(&b.store, &did).await.data_cid,
                case["rootBeforeCommit"].as_str().unwrap(),
                "{name}: root before"
            );

            let mut writes: Vec<PreparedWrite> = adds
                .iter()
                .map(|k| leaf_write(Action::Create, k, &leaf))
                .collect();
            writes.extend(dels.iter().map(|k| leaf_write(Action::Delete, k, &leaf)));
            commit(writes).await.unwrap();
            let root = root_for(&b.store, &did).await;
            assert_eq!(
                root.data_cid,
                case["rootAfterCommit"].as_str().unwrap(),
                "{name}: root after"
            );

            // The event carries the proof and verifies on its own.
            let events = b.store.outbox_for(&did).await.unwrap();
            let body = proto_blue_lex_cbor::decode(&events.last().unwrap().1).unwrap();
            let LexValue::Bytes(car) = &body.as_map().unwrap()["blocks"] else {
                panic!("{name}: blocks")
            };
            let skip = ReadCarOpts {
                skip_cid_verification: true,
            };
            let (_, blocks) = read_car_opts(car, skip).unwrap();
            for c in case["blocksInProof"].as_array().unwrap() {
                assert!(blocks.has(&cid(c)), "{name}: proof block {c} missing");
            }
            verify_event_opts(&body, &key, skip);

            // The bounded walk over the stored tree.
            let mut want: Vec<String> = keys
                .iter()
                .filter(|k| !dels.contains(k))
                .chain(&adds)
                .cloned()
                .collect();
            want.sort();
            let mut walk = TreeWalk::new(Cid::from_str_multibase(&root.data_cid).unwrap());
            let mut got = Vec::new();
            let mut nodes = Vec::new();
            while let Some(n) = walk.next_node().unwrap() {
                let bytes = b
                    .store
                    .block(&did, &n.to_string_base32())
                    .await
                    .unwrap()
                    .unwrap();
                for (k, v) in walk.visit(&bytes).unwrap() {
                    assert_eq!(v, leaf);
                    got.push(k);
                }
                nodes.push((n, bytes));
            }
            got.sort();
            assert_eq!(got, want, "{name}: walked leaves");

            // A CAR of the commit and the tree, written and read by locus.
            let commit_cid = Cid::from_str_multibase(&root.commit_cid).unwrap();
            let commit_bytes = b
                .store
                .block(&did, &root.commit_cid)
                .await
                .unwrap()
                .unwrap();
            nodes.push((commit_cid.clone(), commit_bytes));
            let mut bytes = crate::car::header(Some(&commit_cid));
            for (c, data) in &nodes {
                bytes.extend(crate::car::block(c, data));
            }
            let (mut reader, roots) = CarReader::open(bytes.as_slice()).await.unwrap();
            assert_eq!(roots, vec![commit_cid]);
            let mut read = Vec::new();
            while let Some(block) = reader.next_block().await.unwrap() {
                read.push(block);
            }
            assert_eq!(read, nodes, "{name}: CAR round trip");
        }
    }
}
