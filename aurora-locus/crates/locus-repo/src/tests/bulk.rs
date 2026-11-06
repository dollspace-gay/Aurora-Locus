//! Import and deletion jobs against uploads, crashes between batches and
//! the record index, on both backends. Batches are small in tests (see
//! `import::BATCH`), so every phase runs several of them.

use std::future::Future;
use std::time::Duration;

use locus_core::CoreError;
use proto_blue_repo::commit::{UnsignedCommit, sign_commit};

use super::*;
use crate::import::{ImportError, fault};

type Model = BTreeMap<String, (Cid, Vec<u8>)>;

const POST: &str = "app.bsky.feed.post";
const LIKE: &str = "app.bsky.feed.like";
const PRIVATE: &str = "tools.kryphocron.feed.postPrivate";
const PUBLIC: &str = "tools.kryphocron.feed.postPublic";
const COLLS: [&str; 4] = [POST, LIKE, PRIVATE, PUBLIC];
const HOUR: i64 = 3_600_000;

fn blob_cid(name: &str) -> String {
    Cid::for_raw(name.as_bytes()).to_string_base32()
}

fn lex(pairs: Vec<(&str, LexValue)>) -> LexValue {
    LexValue::Map(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn encode(v: &LexValue) -> (Cid, Vec<u8>) {
    let bytes = proto_blue_lex_cbor::encode(v).unwrap();
    (Cid::for_cbor(&bytes), bytes)
}

/// A record of `collection` (value `n`) using these blobs.
fn rec(collection: &str, n: i64, blobs: &[&str]) -> (Cid, Vec<u8>) {
    let media = blobs
        .iter()
        .map(|c| {
            lex(vec![
                ("$type", LexValue::String("blob".into())),
                (
                    "ref",
                    LexValue::Cid(Cid::from_str_multibase(c).unwrap()),
                ),
                ("mimeType", LexValue::String("image/png".into())),
                ("size", LexValue::Integer(10)),
            ])
        })
        .collect();
    encode(&lex(vec![
        ("$type", LexValue::String(collection.into())),
        ("n", LexValue::Integer(n)),
        ("media", LexValue::Array(media)),
    ]))
}

/// A like of `uri`.
fn like_of(uri: &str) -> (Cid, Vec<u8>) {
    encode(&lex(vec![
        ("$type", LexValue::String(LIKE.into())),
        (
            "subject",
            lex(vec![
                ("uri", LexValue::String(uri.into())),
                (
                    "cid",
                    LexValue::String(blob_cid("subject")),
                ),
            ]),
        ),
    ]))
}

async fn setup(b: &Backend) -> (RepoEngine, K256Keypair) {
    let mut tx = b.store.begin().await.unwrap();
    tx.insert_account(&locus_store::accounts::NewAccount {
        did: DID,
        handle: "bulk.test",
        email: None,
        password_hash: None,
        status: locus_store::accounts::Status::Active,
        did_method: "plc",
    })
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let key = K256Keypair::generate();
    // The media gate on: gating is checked throughout.
    let engine =
        RepoEngine::new(b.store.clone(), Arc::new(TidClock::new(9))).with_media_gate(|| true);
    engine.create_repo(DID, &key).await.unwrap();
    (engine, key)
}

/// `model` as a CAR file signed by `key`.
fn car_file(model: &Model, key: &K256Keypair) -> (tempfile::TempDir, std::path::PathBuf) {
    let mut eager = MstNode::empty();
    for (path, (cid, _)) in model {
        eager = eager.add(path, cid.clone()).unwrap();
    }
    let (data, mut blocks) = eager.get_all_blocks().unwrap();
    for (cid, bytes) in model.values() {
        blocks.set(cid.clone(), bytes.clone());
    }
    let commit = sign_commit(
        &UnsignedCommit::new(DID.to_owned(), data, "2222222222222".into(), None),
        key,
    )
    .unwrap();
    let root = commit.cid().unwrap();
    blocks.set(root.clone(), commit.to_cbor().unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.car");
    std::fs::write(
        &path,
        proto_blue_repo::car::blocks_to_car(Some(&root), &blocks).unwrap(),
    )
    .unwrap();
    (dir, path)
}

async fn import_all(engine: &RepoEngine, model: &Model, key: &K256Keypair) {
    let (_dir, path) = car_file(model, key);
    let job = engine.start_import(DID, "node-a").await.unwrap();
    engine.run_import(&job, &path, None, key, true).await.unwrap();
    engine.finish_import(&job).await.unwrap();
}

/// What `uploadBlob` does under the repo lock once the bytes are stored.
async fn upload(engine: &RepoEngine, cid: &str) {
    let (_guard, mut tx) = engine.locked_tx(DID).await.unwrap();
    let backend = tx.blob_backend().await.unwrap();
    let temp = locus_store::blob::temp_key();
    let fresh = match tx.blob_for_update(DID, cid).await.unwrap() {
        None => {
            tx.insert_temp_blob(DID, cid, "image/png", 10, &temp)
                .await
                .unwrap();
            true
        }
        Some(r) if r.state == "temp" => {
            tx.repoint_temp_blob(DID, cid, &temp).await.unwrap();
            true
        }
        Some(_) => {
            tx.restart_unreferenced(DID, cid).await.unwrap();
            false
        }
    };
    if fresh {
        tx.promote_upload(DID, cid, engine.media_gate_on(), backend.epoch)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
}

/// The mover job: finishes every pending move.
async fn settle(store: &Store) {
    for m in store.pending_blob_moves(1000).await.unwrap() {
        let mut tx = store.begin().await.unwrap();
        let epoch = tx.blob_backend().await.unwrap().epoch;
        tx.finish_blob_move(&m.did, &m.cid, &m.to, epoch)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
}

/// Blob GC with every age past its limit: whatever it may collect, it does.
async fn gc(store: &Store) -> u64 {
    settle(store).await;
    let mut tx = store.begin().await.unwrap();
    let epoch = tx.blob_backend().await.unwrap().epoch;
    let n = tx.gc_blob_batch(-HOUR, -HOUR, 1000, epoch).await.unwrap();
    tx.commit().await.unwrap();
    n
}

async fn row(store: &Store, cid: &str) -> Option<locus_store::blob::BlobRow> {
    store.blob(DID, cid).await.unwrap()
}

async fn has_records(store: &Store, cid: &str) -> bool {
    let mut tx = store.begin().await.unwrap();
    let r = tx.blob_has_records(DID, cid).await.unwrap();
    tx.rollback().await.unwrap();
    r
}

/// The blob is in use: permanent, referenced, gated exactly when
/// `private`, and its bytes in the store that calls for once moved.
async fn assert_in_use(store: &Store, cid: &str, private: bool, what: &str) {
    settle(store).await;
    let r = row(store, cid)
        .await
        .unwrap_or_else(|| panic!("{what}: {cid} is gone"));
    assert_eq!(
        (r.state.as_str(), r.unreferenced_since),
        ("permanent", None),
        "{what}: {r:?}"
    );
    assert_eq!(
        store.blob_private_only(DID, cid).await.unwrap(),
        private,
        "{what}: gated"
    );
    let store_prefix = if private { "gated/" } else { "blobs/" };
    assert!(r.storage_key.starts_with(store_prefix), "{what}: {r:?}");
}

/// The record index holds exactly `model` (taken-down records aside).
async fn check_index(store: &Store, model: &Model, what: &str) {
    assert_eq!(index(store).await, flat(model), "{what}: record index");
}

async fn index(store: &Store) -> BTreeMap<String, String> {
    let mut got = BTreeMap::new();
    for c in COLLS {
        for (rkey, cid) in store.list_records(DID, c, 10_000, None, false).await.unwrap() {
            got.insert(format!("{c}/{rkey}"), cid);
        }
    }
    got
}

fn flat(model: &Model) -> BTreeMap<String, String> {
    model
        .iter()
        .map(|(p, (c, _))| (p.clone(), c.to_string_base32()))
        .collect()
}

async fn count(store: &Store, cid: &Cid) -> i64 {
    store
        .repo_block_counts(DID)
        .await
        .unwrap()
        .get(&cid.to_string_base32())
        .copied()
        .unwrap_or(0)
}

async fn set_phase(store: &Store, job: &str, phase: &str) {
    let mut tx = store.begin().await.unwrap();
    tx.set_bulk_progress(job, phase, None).await.unwrap();
    tx.commit().await.unwrap();
}

async fn write_blocked(store: &Store) -> bool {
    let mut tx = store.begin().await.unwrap();
    let r = tx.write_blocked(DID).await.unwrap();
    tx.rollback().await.unwrap();
    r
}

async fn reached(p: &fault::Pause) {
    tokio::time::timeout(Duration::from_secs(20), p.reached.notified())
        .await
        .expect("the job never reached the fault point");
}

/// Runs `fut` (a job phase), pausing it at the `skip`-th next batch of
/// `point` while `act` runs.
async fn during<T>(
    fut: impl Future<Output = T>,
    point: &'static str,
    skip: usize,
    act: impl Future<Output = ()>,
) -> T {
    let p = fault::pause(point, skip, false);
    let (out, ()) = tokio::join!(fut, async {
        reached(&p).await;
        act.await;
        p.resume.notify_one();
    });
    out
}

fn crashed<T: std::fmt::Debug>(r: Result<T, ImportError>, what: &str) {
    match r {
        Err(ImportError::Core(CoreError::Storage(m))) if m.contains("injected crash") => {}
        other => panic!("{what}: expected the injected crash, got {other:?}"),
    }
    assert!(!fault::armed(), "{what}: fired");
}

fn base() -> Model {
    (0..6)
        .map(|i| (format!("{POST}/3m{i:011}"), record(POST, i)))
        .collect()
}

#[tokio::test]
async fn uploads_during_each_import_phase_end_permanent_and_gated_per_the_new_tree() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let mut current = base();
        import_all(&engine, &current, &key).await;
        let points = [
            "before-staging",
            "staging",
            "increments",
            "after-increments",
            "reindex",
            "decrements",
            "decrements-blobs",
            "done",
        ];
        let mut round = 0;
        for point in points {
            let before_swap = matches!(
                point,
                "before-staging" | "staging" | "increments" | "after-increments"
            );
            for rollback in [false, true] {
                if rollback && !before_swap {
                    continue;
                }
                round += 1;
                let what = format!("{} {point}{}", b.name, if rollback { " rolled back" } else { "" });
                let pubc = blob_cid(&format!("public {what}"));
                let privc = blob_cid(&format!("private {what}"));
                // The new tree: the base, a public post and a private one
                // using one blob each, and fillers so each phase has
                // several batches.
                let mut new = base();
                new.insert(format!("{PUBLIC}/r{round}p"), rec(PUBLIC, round, &[&pubc]));
                new.insert(format!("{PRIVATE}/r{round}q"), rec(PRIVATE, round, &[&privc]));
                for i in 0..6 {
                    new.insert(format!("{LIKE}/r{round}f{i}"), record(LIKE, (round * 10 + i) as u64));
                }
                let (_dir, path) = car_file(&new, &key);
                let job = engine.start_import(DID, "node-a").await.unwrap();

                // The upload, then checks right after it: with the import's
                // staged row in place, it's permanent at once, gated per
                // the new tree, and temp GC leaves it.
                let up = || async {
                    upload(&engine, &pubc).await;
                    upload(&engine, &privc).await;
                    if point != "before-staging" {
                        gc(s).await;
                        assert_in_use(s, &pubc, false, &format!("{what}: at upload")).await;
                        assert_in_use(s, &privc, true, &format!("{what}: at upload")).await;
                    }
                };
                if point == "before-staging" {
                    up().await;
                }
                let staged = engine.stage_import(&job, &path, None).await.unwrap();
                if point == "before-staging" {
                    // Still temp (no staged row when it came), but the
                    // import's CID now: temp GC leaves it.
                    gc(s).await;
                    assert_eq!(row(s, &pubc).await.unwrap().state, "temp", "{what}");
                    assert_eq!(row(s, &privc).await.unwrap().state, "temp", "{what}");
                }
                if point == "staging" {
                    // Staging is past writing its blob rows.
                    set_phase(s, &job.id, "staging").await;
                    up().await;
                    set_phase(s, &job.id, "increments").await;
                }
                if point == "increments" {
                    // Between the block pass and the blob pass.
                    during(
                        engine.import_increments(&job, &staged.new_rev),
                        "increments-blobs",
                        0,
                        up(),
                    )
                    .await
                    .unwrap();
                } else {
                    engine
                        .import_increments(&job, &staged.new_rev)
                        .await
                        .unwrap();
                }
                if point == "after-increments" {
                    up().await;
                }
                if rollback {
                    engine.import_rollback(&job, "stopped").await.unwrap();
                    check_against_model(&b, &current, round as usize, &key).await;
                    check_index(s, &current, &what).await;
                    for c in [&pubc, &privc] {
                        assert!(!has_records(s, c).await, "{what}");
                        assert!(row(s, c).await.is_some(), "{what}: uploaded");
                    }
                    // Unreferenced, so collected.
                    gc(s).await;
                    assert!(row(s, &pubc).await.is_none(), "{what}: collected");
                    assert!(row(s, &privc).await.is_none(), "{what}: collected");
                    continue;
                }
                engine
                    .import_swap(&job, &staged.data_cid, &staged.new_rev, &key, true)
                    .await
                    .unwrap();
                if point == "reindex" {
                    during(engine.import_reindex(&job, &staged.new_rev), "reindex", 1, up())
                        .await
                        .unwrap();
                } else {
                    engine.import_reindex(&job, &staged.new_rev).await.unwrap();
                }
                if point == "decrements" || point == "decrements-blobs" {
                    let skip = usize::from(point == "decrements");
                    during(engine.import_decrements(&job, &staged.new_rev), point, skip, up())
                        .await
                        .unwrap();
                } else {
                    engine
                        .import_decrements(&job, &staged.new_rev)
                        .await
                        .unwrap();
                }
                if point == "done" {
                    up().await;
                }
                current = new;
                check_against_model(&b, &current, round as usize, &key).await;
                check_index(s, &current, &what).await;
                gc(s).await;
                for (c, private) in [(&pubc, false), (&privc, true)] {
                    assert!(has_records(s, c).await, "{what}");
                    assert_in_use(s, c, private, &format!("{what}: at the end")).await;
                }
            }
        }
    }
}

#[tokio::test]
async fn uploads_during_a_rollback_stay_temp_and_promoted_blobs_end_unreferenced() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let n = b.name;
        let old = base();
        import_all(&engine, &old, &key).await;

        // Six blobs uploaded before the import (promoted by increments), x
        // just before the rollback, and y, z, w during it: after it set
        // `rolling_back`, during its blob pass and after it.
        let early: Vec<String> = (0..6).map(|i| blob_cid(&format!("early {n} {i}"))).collect();
        let [x, y, z, w, v] = ["x", "y", "z", "w", "v"].map(|t| blob_cid(&format!("{t} {n}")));
        let mut new = old.clone();
        for (i, c) in early.iter().chain([&x, &y, &z, &w, &v]).enumerate() {
            new.insert(format!("{POST}/b{i:02}"), rec(POST, i as i64, &[c]));
        }
        let (_dir, path) = car_file(&new, &key);
        for c in &early {
            upload(&engine, c).await;
        }
        let job = engine.start_import(DID, "node-a").await.unwrap();
        let staged = engine.stage_import(&job, &path, None).await.unwrap();
        engine
            .import_increments(&job, &staged.new_rev)
            .await
            .unwrap();
        for c in &early {
            assert_eq!(row(s, c).await.unwrap().state, "permanent", "{n}: promoted");
        }
        upload(&engine, &x).await;
        assert_eq!(row(s, &x).await.unwrap().state, "permanent", "{n}");
        {
            let mut tx = s.begin().await.unwrap();
            let page = tx.import_blobs_page(&job.id, "", 100).await.unwrap();
            tx.rollback().await.unwrap();
            assert!(
                page.iter().any(|r| r.cid == x && r.applied),
                "{n}: x marked applied"
            );
        }

        let temp_and_collected = |c: String, what: &'static str| {
            let engine = &engine;
            async move {
                upload(engine, &c).await;
                let r = row(s, &c).await.unwrap();
                assert_eq!(r.state, "temp", "{n} {what}: not promoted");
                gc(s).await;
                assert!(row(s, &c).await.is_none(), "{n} {what}: collected");
            }
        };
        let p1 = fault::pause("rollback", 0, false);
        let (res, ()) = tokio::join!(engine.import_rollback(&job, "stopped"), async {
            reached(&p1).await;
            assert_eq!(s.bulk_job(&job.id).await.unwrap().unwrap().phase, "rolling_back");
            temp_and_collected(y.clone(), "after rolling_back").await;
            // Not unreferenced yet: the rollback's blob pass hasn't run.
            assert!(row(s, &x).await.is_some(), "{n}");
            let p2 = fault::pause("rollback-blobs", 1, false);
            p1.resume.notify_one();
            reached(&p2).await;
            temp_and_collected(z.clone(), "during the blob pass").await;
            let p3 = fault::pause("drop", 0, false);
            p2.resume.notify_one();
            reached(&p3).await;
            temp_and_collected(w.clone(), "after the blob pass").await;
            // A failed job's CIDs are collected too (its rows may outlive it).
            set_phase(s, &job.id, "failed").await;
            temp_and_collected(v.clone(), "failed").await;
            gc(s).await;
            for c in early.iter().chain([&x]) {
                assert!(row(s, c).await.is_none(), "{n}: promoted, then collected");
            }
            set_phase(s, &job.id, "rolling_back").await;
            p3.resume.notify_one();
        });
        res.unwrap();
        let st = s.latest_import(DID).await.unwrap().unwrap();
        assert_eq!(
            (st.phase.as_str(), st.error.as_deref()),
            ("failed", Some("stopped")),
            "{n}"
        );
        check_against_model(&b, &old, 1, &key).await;
        check_index(s, &old, n).await;
        assert!(!write_blocked(s).await, "{n}");

        // A staging failure after an upload promoted one of its CIDs: the
        // rollback runs, and the blob ends unreferenced, not leaked.
        let late = blob_cid(&format!("late {n}"));
        let mut new = old.clone();
        new.insert(format!("{POST}/late"), rec(POST, 77, &[&late]));
        let (_dir, path) = car_file(&new, &key);
        let job = engine.start_import(DID, "node-a").await.unwrap();
        let p = fault::pause("stage-end", 0, true);
        let (res, ()) = tokio::join!(engine.run_import(&job, &path, None, &key, true), async {
            reached(&p).await;
            upload(&engine, &late).await;
            assert_eq!(row(s, &late).await.unwrap().state, "permanent", "{n}");
            p.resume.notify_one();
        });
        crashed(res, n);
        let st = s.latest_import(DID).await.unwrap().unwrap();
        assert_eq!(st.phase, "failed", "{n}");
        assert!(st.error.unwrap().contains("injected crash"), "{n}");
        let r = row(s, &late).await.unwrap();
        assert!(r.unreferenced_since.is_some(), "{n}: unreferenced {r:?}");
        gc(s).await;
        assert!(row(s, &late).await.is_none(), "{n}: collected");
        check_against_model(&b, &old, 2, &key).await;
    }
}

#[tokio::test]
async fn crashes_between_batches_roll_back_before_the_swap_and_resume_after_it() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let n = b.name;
        let ob = blob_cid(&format!("old {n}"));
        let nb = blob_cid(&format!("new {n}"));
        // D is referenced three times by the old tree and once by the new;
        // E once by the old and three times by the new.
        let d = record(LIKE, 500);
        let e = record(LIKE, 600);
        let mut old = base();
        for i in 6..12 {
            old.insert(format!("{POST}/3m{i:011}"), record(POST, i));
        }
        for i in 0..3 {
            old.insert(format!("{LIKE}/d{i}"), d.clone());
        }
        old.insert(format!("{LIKE}/e0"), e.clone());
        old.insert(format!("{POST}/ob"), rec(POST, 700, &[&ob]));
        let mut new = old.clone();
        for i in 6..12 {
            new.insert(format!("{POST}/3m{i:011}"), record(POST, i + 100));
        }
        new.remove(&format!("{LIKE}/d1"));
        new.remove(&format!("{LIKE}/d2"));
        for i in 1..3 {
            new.insert(format!("{LIKE}/e{i}"), e.clone());
        }
        new.remove(&format!("{POST}/ob"));
        new.insert(format!("{POST}/nb"), rec(POST, 800, &[&nb]));
        for i in 0..12 {
            new.insert(format!("{LIKE}/f{i:02}"), record(LIKE, 900 + i));
        }
        let (_dir, path) = car_file(&new, &key);
        import_all(&engine, &old, &key).await;
        upload(&engine, &ob).await;
        let ob_ref = &ob;
        let old_blob_intact = |what: String| async move {
            assert!(has_records(s, ob_ref).await, "{what}: record_blob rows intact");
            let r = row(s, ob_ref).await.unwrap();
            assert_eq!((r.state.as_str(), r.unreferenced_since), ("permanent", None), "{what}");
        };

        // Before the swap: each crash rolls back (some crash again in the
        // middle of the rollback, which resumes and undoes each row once).
        let before = [
            ("car", 1, None),
            ("stage", 1, None),
            ("stage-end", 0, Some(("drop", 0))),
            ("increments", 1, Some(("rollback-blobs", 0))),
            ("increments-blobs", 0, Some(("rollback", 2))),
        ];
        for (round, (point, skip, again)) in before.into_iter().enumerate() {
            let what = format!("{n} crash at {point}");
            upload(&engine, &nb).await;
            let job = engine.start_import(DID, "node-a").await.unwrap();
            fault::crash(point, skip);
            let r = async {
                let st = engine.stage_import(&job, &path, None).await?;
                engine.import_increments(&job, &st.new_rev).await?;
                Ok::<_, ImportError>(st)
            }
            .await;
            crashed(r, &what);
            if point == "increments-blobs" {
                // The block pass applied +2 and -0 with multiplicity.
                assert_eq!(count(s, &e.0).await, 3, "{what}");
                assert_eq!(count(s, &d.0).await, 3, "{what}");
            }
            let taken = s.bulk_job(&job.id).await.unwrap().unwrap();
            assert!(write_blocked(s).await, "{what}: still blocked");
            if let Some((p, k)) = again {
                fault::crash(p, k);
                crashed(engine.resume_import(&taken).await, &what);
                assert_eq!(
                    s.bulk_job(&job.id).await.unwrap().unwrap().phase,
                    "rolling_back",
                    "{what}"
                );
            }
            let taken = s.bulk_job(&job.id).await.unwrap().unwrap();
            engine.resume_import(&taken).await.unwrap();
            check_against_model(&b, &old, round, &key).await;
            check_index(s, &old, &what).await;
            assert_eq!(count(s, &e.0).await, 1, "{what}");
            assert_eq!(count(s, &d.0).await, 3, "{what}");
            let st = s.latest_import(DID).await.unwrap().unwrap();
            assert_eq!(st.phase, "failed", "{what}");
            assert!(
                st.error.unwrap().contains("interrupted"),
                "{what}: the resumer's error"
            );
            assert!(!write_blocked(s).await, "{what}");
            old_blob_intact(what.clone()).await;
            // The promoted new blob is left for GC.
            assert!(!has_records(s, &nb).await, "{what}");
            gc(s).await;
            assert!(row(s, &nb).await.is_none(), "{what}: collected");
        }

        // After the swap: each crash resumes forward to the same rev.
        let after = [
            ("reindex", 1),
            ("decrements", 1),
            ("decrements-blobs", 0),
            ("drop", 0),
            ("finish", 0),
        ];
        for (round, (point, skip)) in after.into_iter().enumerate() {
            let what = format!("{n} crash at {point}");
            upload(&engine, &nb).await;
            let job = engine.start_import(DID, "node-a").await.unwrap();
            let staged = engine.stage_import(&job, &path, None).await.unwrap();
            engine
                .import_increments(&job, &staged.new_rev)
                .await
                .unwrap();
            engine
                .import_swap(&job, &staged.data_cid, &staged.new_rev, &key, true)
                .await
                .unwrap();
            fault::crash(point, skip);
            crashed(engine.finish_import(&job).await, &what);
            if point == "decrements" {
                // reindex removed the old record's rows; its blob still
                // gets its decrement from the staged rows.
                assert!(!has_records(s, &ob).await, "{what}");
                assert_eq!(row(s, &ob).await.unwrap().unreferenced_since, None, "{what}");
            }
            let taken = s.bulk_job(&job.id).await.unwrap().unwrap();
            engine.resume_import(&taken).await.unwrap();
            let root = root_of(s).await;
            assert_eq!(root.rev, staged.new_rev, "{what}: the staged rev");
            assert_eq!(root.data_cid, staged.data_cid.to_string_base32(), "{what}");
            check_against_model(&b, &new, 10 + round, &key).await;
            check_index(s, &new, &what).await;
            assert_eq!(count(s, &e.0).await, 3, "{what}");
            assert_eq!(count(s, &d.0).await, 1, "{what}");
            assert_eq!(s.latest_import(DID).await.unwrap().unwrap().phase, "done");
            assert!(row(s, &ob).await.unwrap().unreferenced_since.is_some(), "{what}");
            assert_in_use(s, &nb, false, &what).await;
            // Back to the old repo for the next round.
            import_all(&engine, &old, &key).await;
            upload(&engine, &ob).await;
            old_blob_intact(what.clone()).await;
        }
    }
}

#[tokio::test]
async fn the_index_serves_the_old_repo_then_lags_the_new_root_and_keeps_takedowns() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let n = b.name;
        let (a_uri, b_uri) = (
            "at://did:plc:someone0000000000000000/app.bsky.feed.post/3maaaaaaaaaaa",
            "at://did:plc:someone0000000000000000/app.bsky.feed.post/3mbbbbbbbbbbb",
        );
        let (tb_old, tb_new) = (blob_cid(&format!("t old {n}")), blob_cid(&format!("t new {n}")));
        let mut old = base();
        for i in 6..14 {
            old.insert(format!("{POST}/3m{i:011}"), record(POST, i));
        }
        old.insert(format!("{LIKE}/l0"), like_of(a_uri));
        old.insert(format!("{POST}/t"), rec(POST, 1, &[&tb_old]));
        // Every key changes.
        let mut new: Model = old
            .keys()
            .enumerate()
            .map(|(i, p)| (p.clone(), record(POST, 1000 + i as u64)))
            .collect();
        new.insert(format!("{LIKE}/l0"), like_of(b_uri));
        new.insert(format!("{POST}/t"), rec(POST, 2, &[&tb_new]));
        import_all(&engine, &old, &key).await;
        assert!(
            s.set_record_takedown(DID, POST, "t", Some("takedown-1"))
                .await
                .unwrap()
        );
        let visible = |m: &Model| {
            let mut m = m.clone();
            m.remove(&format!("{POST}/t"));
            m
        };

        let (_dir, path) = car_file(&new, &key);
        let job = engine.start_import(DID, "node-a").await.unwrap();
        let staged = engine.stage_import(&job, &path, None).await.unwrap();
        // During increments the index is exactly the old repo.
        during(
            engine.import_increments(&job, &staged.new_rev),
            "increments",
            1,
            check_index(s, &visible(&old), &format!("{n} during increments")),
        )
        .await
        .unwrap();
        engine
            .import_swap(&job, &staged.data_cid, &staged.new_rev, &key, true)
            .await
            .unwrap();
        // During reindex it lags the new root: some keys already new, the
        // rest still old, none from anywhere else.
        let (old_flat, new_flat) = (flat(&old), flat(&new));
        during(engine.import_reindex(&job, &staged.new_rev), "reindex", 2, async {
            assert_eq!(root_of(s).await.data_cid, staged.data_cid.to_string_base32());
            let got = index(s).await;
            let mut moved = 0;
            for (p, c) in &got {
                assert!(
                    Some(c) == old_flat.get(p) || Some(c) == new_flat.get(p),
                    "{n}: {p} -> {c} is in neither tree"
                );
                moved += usize::from(Some(c) != old_flat.get(p));
            }
            assert!(moved > 0 && moved < got.len(), "{n}: mid-reindex ({moved})");
        })
        .await
        .unwrap();
        engine
            .import_decrements(&job, &staged.new_rev)
            .await
            .unwrap();
        check_index(s, &visible(&new), &format!("{n} done")).await;
        // The changed record keeps its takedown, and its old blob and
        // backlink rows are gone.
        let (cid, takedown) = s.record_entry(DID, POST, "t").await.unwrap().unwrap();
        assert_eq!(cid, new[&format!("{POST}/t")].0.to_string_base32());
        assert_eq!(takedown.as_deref(), Some("takedown-1"), "{n}");
        let mut tx = s.begin().await.unwrap();
        let blobs = tx
            .record_blob_cids(DID, &[(POST.into(), "t".into())])
            .await
            .unwrap();
        assert_eq!(
            blobs.get(&(POST.to_owned(), "t".to_owned())),
            Some(&vec![tb_new.clone()]),
            "{n}"
        );
        assert!(
            tx.backlinked_records(DID, LIKE, "subject.uri", a_uri)
                .await
                .unwrap()
                .is_empty(),
            "{n}: old backlink gone"
        );
        assert_eq!(
            tx.backlinked_records(DID, LIKE, "subject.uri", b_uri)
                .await
                .unwrap(),
            vec!["l0".to_owned()],
            "{n}"
        );
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn reindex_gates_a_blob_when_its_last_public_record_goes_and_not_before_a_later_one() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let n = b.name;
        let (g, k) = (blob_cid(&format!("g {n}")), blob_cid(&format!("k {n}")));
        // g: a public and a private post; the import removes the public
        // one. k: one private post now; the import adds a public one that
        // sorts after it (postPrivate < postPublic), batches later.
        let mut old = base();
        old.insert(format!("{POST}/gpub"), rec(POST, 1, &[&g]));
        old.insert(format!("{PRIVATE}/gpriv"), rec(PRIVATE, 1, &[&g]));
        old.insert(format!("{PRIVATE}/a0"), rec(PRIVATE, 2, &[&k]));
        import_all(&engine, &old, &key).await;
        upload(&engine, &g).await;
        upload(&engine, &k).await;
        assert_in_use(s, &g, false, n).await;
        assert_in_use(s, &k, true, n).await;
        let mut new = old.clone();
        new.remove(&format!("{POST}/gpub"));
        new.insert(format!("{PRIVATE}/a0"), rec(PRIVATE, 3, &[&k]));
        for i in 0..8 {
            new.insert(format!("{PRIVATE}/a{}", i + 1), record(PRIVATE, 10 + i));
        }
        new.insert(format!("{PUBLIC}/z"), rec(PUBLIC, 4, &[&k]));
        let (_dir, path) = car_file(&new, &key);
        let job = engine.start_import(DID, "node-a").await.unwrap();
        let staged = engine.stage_import(&job, &path, None).await.unwrap();
        engine
            .import_increments(&job, &staged.new_rev)
            .await
            .unwrap();
        engine
            .import_swap(&job, &staged.data_cid, &staged.new_rev, &key, true)
            .await
            .unwrap();
        // The first batch (app.bsky.feed.post/gpub's delete first) has run.
        during(engine.import_reindex(&job, &staged.new_rev), "reindex", 1, async {
            let mut tx = s.begin().await.unwrap();
            let gpub = tx
                .record_blob_cids(DID, &[(POST.into(), "gpub".into())])
                .await
                .unwrap();
            tx.rollback().await.unwrap();
            assert!(gpub.is_empty(), "{n}: the public record left the index");
            assert_in_use(s, &g, true, &format!("{n}: gated in that batch")).await;
        })
        .await
        .unwrap();
        engine
            .import_decrements(&job, &staged.new_rev)
            .await
            .unwrap();
        assert_in_use(s, &g, true, n).await;
        assert_in_use(s, &k, false, n).await;
    }
}

#[tokio::test]
async fn reindex_never_gates_a_blob_a_later_batch_makes_public() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let n = b.name;
        let k = blob_cid(&format!("k {n}"));
        let old = base();
        import_all(&engine, &old, &key).await;
        upload(&engine, &k).await;
        // k: a private post early in the key order and a public one much
        // later; increments promotes it (public in the new tree).
        let mut new = old.clone();
        new.insert(format!("{PRIVATE}/a0"), rec(PRIVATE, 3, &[&k]));
        for i in 0..8 {
            new.insert(format!("{PRIVATE}/a{}", i + 1), record(PRIVATE, 10 + i));
        }
        new.insert(format!("{PUBLIC}/z"), rec(PUBLIC, 4, &[&k]));
        let (_dir, path) = car_file(&new, &key);
        let job = engine.start_import(DID, "node-a").await.unwrap();
        let staged = engine.stage_import(&job, &path, None).await.unwrap();
        engine
            .import_increments(&job, &staged.new_rev)
            .await
            .unwrap();
        assert_in_use(s, &k, false, &format!("{n}: promoted")).await;
        engine
            .import_swap(&job, &staged.data_cid, &staged.new_rev, &key, true)
            .await
            .unwrap();
        // After each of the three reindex batches (ten rows, z last): never
        // gated, though only the private post is indexed until the last.
        let first = fault::pause("reindex", 1, false);
        let (res, ()) = tokio::join!(engine.import_reindex(&job, &staged.new_rev), async {
            let mut p = first;
            for batch in 1..=3 {
                reached(&p).await;
                assert!(has_records(s, &k).await, "{n}: indexed");
                assert!(
                    !s.blob_private_only(DID, &k).await.unwrap(),
                    "{n}: gated after reindex batch {batch}"
                );
                // The next call: the next batch, or (after the third) the
                // one that ends the phase.
                let next = (batch < 3).then(|| fault::pause("reindex", 0, false));
                p.resume.notify_one();
                match next {
                    Some(x) => p = x,
                    None => break,
                }
            }
        });
        res.unwrap();
        assert!(!fault::armed());
        engine
            .import_decrements(&job, &staged.new_rev)
            .await
            .unwrap();
        assert_in_use(s, &k, false, n).await;
    }
}

#[tokio::test]
async fn import_and_deletion_exclude_each_other_and_deletion_resumes_after_crashes() {
    for b in backends().await {
        let (engine, key) = setup(&b).await;
        let s = &b.store;
        let n = b.name;
        let blob = blob_cid(&format!("del {n}"));
        let mut old = base();
        old.insert(format!("{POST}/img"), rec(POST, 1, &[&blob]));
        import_all(&engine, &old, &key).await;
        upload(&engine, &blob).await;

        // An import holds the account: no deletion, no writes.
        let job = engine.start_import(DID, "node-a").await.unwrap();
        let mut tx = s.begin().await.unwrap();
        assert!(
            matches!(
                tx.start_bulk_job("del-1", DID, "delete", "deleting", "node-a")
                    .await,
                Err(CoreError::Conflict(_))
            ),
            "{n}"
        );
        tx.rollback().await.unwrap();
        let commit = |rkey: &'static str| {
            engine.commit(crate::engine::CommitRequest {
                did: DID,
                writes: vec![write(Action::Create, POST, rkey, Some(5))],
                swap_commit: None,
                signer: &key,
                dedupe_backlinks: false,
            })
        };
        assert!(
            matches!(commit("w1").await, Err(CommitError::WriteBlocked)),
            "{n}"
        );
        engine.import_rollback(&job, "stopped").await.unwrap();

        // A deletion holds it: no import.
        let mut tx = s.begin().await.unwrap();
        let del = tx
            .start_bulk_job("del-1", DID, "delete", "deleting", "node-a")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(
            matches!(
                engine.start_import(DID, "node-a").await,
                Err(ImportError::Core(CoreError::Conflict(_)))
            ),
            "{n}"
        );
        assert!(
            matches!(commit("w2").await, Err(CommitError::WriteBlocked)),
            "{n}"
        );

        // Crashes between batches, then just before the final step: each
        // time the lease runs out and another node takes the job over.
        let batch = |job: locus_store::bulk::BulkJob| async move {
            assert!(s.renew_bulk_job(&job).await.unwrap());
            let mut tx = s.begin().await.unwrap();
            tx.fence_bulk_job(&job).await.unwrap();
            let left = tx.delete_account_batch(DID, 2).await.unwrap();
            if left == 0 {
                tx.rollback().await.unwrap(); // the crash, before the final step
            } else {
                tx.set_bulk_progress(&job.id, "deleting", None).await.unwrap();
                tx.commit().await.unwrap();
            }
            left
        };
        let take_over = |holder: &'static str| async move {
            let expire = "UPDATE bulk_job SET expires_at = 0 WHERE id = 'del-1'";
            match s {
                Store::Sqlite(p) => sqlx::query(expire).execute(p).await.map(|_| ()),
                Store::Postgres(p) => sqlx::query(expire).execute(p).await.map(|_| ()),
            }
            .unwrap();
            s.take_over_bulk_job(holder).await.unwrap().expect("taken over")
        };
        for _ in 0..3 {
            assert!(batch(del.clone()).await > 0, "{n}");
        }
        let second = take_over("node-b").await;
        assert_eq!(second.epoch, 2, "{n}");
        assert!(
            matches!(s.run_delete_job(&del, 2).await, Err(CoreError::LeaseLost(_))),
            "{n}: the first holder is fenced out"
        );
        while batch(second.clone()).await > 0 {}
        assert!(s.account_by_did(DID).await.unwrap().is_some(), "{n}: not yet");
        let third = take_over("node-c").await;
        s.run_delete_job(&third, 2).await.unwrap();
        assert!(s.account_by_did(DID).await.unwrap().is_none(), "{n}");
        assert!(s.repo_root_of(DID).await.unwrap().is_none(), "{n}");
        assert!(s.repo_block_counts(DID).await.unwrap().is_empty(), "{n}");
        assert!(check_empty_index(s).await, "{n}");
        assert!(row(s, &blob).await.is_none(), "{n}: blob rows gone");
        assert!(!has_records(s, &blob).await, "{n}");
        assert_eq!(s.bulk_job("del-1").await.unwrap().unwrap().phase, "done", "{n}");
    }
}

async fn check_empty_index(store: &Store) -> bool {
    index(store).await.is_empty()
}
