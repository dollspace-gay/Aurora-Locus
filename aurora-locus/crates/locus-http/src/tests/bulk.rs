//! Bulk jobs over HTTP: writes blocked through every import phase while
//! handle and status changes go through, uploads during and after an
//! import with the media gate on, and blob checks in record writes.

use proto_blue_lex_data::Cid;
use proto_blue_repo::commit::{UnsignedCommit, sign_commit};
use proto_blue_repo::mst::MstNode;

use super::*;

fn png(tag: &str) -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend(tag.as_bytes());
    v
}

fn blob_of(bytes: &[u8]) -> Value {
    json!({
        "$type": "blob",
        "ref": {"$link": Cid::for_raw(bytes).to_string_base32()},
        "mimeType": "image/png",
        "size": bytes.len(),
    })
}

fn cid_of(bytes: &[u8]) -> String {
    Cid::for_raw(bytes).to_string_base32()
}

fn with_image(mut record: Value, bytes: &[u8]) -> Value {
    record["embed"] =
        json!({"$type": "app.bsky.embed.images", "images": [{"image": blob_of(bytes), "alt": ""}]});
    record
}

fn private_record(text: &str) -> Value {
    json!({"$type": "tools.kryphocron.feed.postPrivate", "text": text, "createdAt": "2026-11-05T00:00:00.000Z"})
}

/// A repository of exactly these records, signed with the account's key.
async fn car_of(h: &Harness, did: &str, records: &[(&str, Value)]) -> Vec<u8> {
    let encoded: Vec<(String, Cid, Vec<u8>)> = records
        .iter()
        .map(|(path, v)| {
            let bytes = proto_blue_lex_cbor::encode(&proto_blue_lex_json::json_to_lex(v)).unwrap();
            ((*path).to_owned(), Cid::for_cbor(&bytes), bytes)
        })
        .collect();
    let mut eager = MstNode::empty();
    for (path, cid, _) in &encoded {
        eager = eager.add(path, cid.clone()).unwrap();
    }
    let (data, mut blocks) = eager.get_all_blocks().unwrap();
    for (_, cid, bytes) in encoded {
        blocks.set(cid, bytes);
    }
    let key = h.state.signing_key(did).await.unwrap();
    let commit = sign_commit(
        &UnsignedCommit::new(did.to_owned(), data, "2222222222222".into(), None),
        &key,
    )
    .unwrap();
    let root = commit.cid().unwrap();
    blocks.set(root.clone(), commit.to_cbor().unwrap());
    proto_blue_repo::car::blocks_to_car(Some(&root), &blocks).unwrap()
}

/// The private media gate on, with a scanner (answering while `down` is
/// false; unscanned uploads accepted while it's down).
async fn gate_on(h: &Harness) -> Arc<std::sync::atomic::AtomicBool> {
    let (url, down) = mock_scanner().await;
    for (k, v) in [
        ("blobs.scanner", json!("webhook")),
        ("blobs.scanner_url", json!(url)),
        ("blobs.scan_unavailable", json!("accept_unscanned")),
        ("kryphocron.private_media_gated", json!(true)),
    ] {
        h.state.settings.set(k, v, "test").await.unwrap();
    }
    assert!(crate::media_gate::gate_on(&h.state));
    down
}

/// The blob is permanent, gated exactly when `private`, its bytes in the
/// store that calls for, and served without credentials exactly when not.
async fn assert_blob(h: &Harness, did: &str, cid: &str, private: bool, what: &str) {
    let r = h.state.store.blob(did, cid).await.unwrap().unwrap();
    assert_eq!(
        (
            r.state.as_str(),
            r.unreferenced_since,
            r.pending_move.as_deref()
        ),
        ("permanent", None, None),
        "{what}: {r:?}"
    );
    assert_eq!(
        h.state.store.blob_private_only(did, cid).await.unwrap(),
        private,
        "{what}: gated"
    );
    let prefix = if private { "gated/" } else { "blobs/" };
    assert!(r.storage_key.starts_with(prefix), "{what}: {r:?}");
    let (st, _, _) = h
        .raw_get(format!(
            "/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}"
        ))
        .await;
    assert_eq!(st == StatusCode::OK, !private, "{what}: served ({st})");
}

#[tokio::test]
async fn writes_are_blocked_through_every_import_phase_but_handle_and_status_changes_go_through() {
    let h = harness(false).await;
    let (did, access) = h.user("ina.pds.example.com").await;
    let s = &h.state;
    let create = |text: &'static str| {
        h.post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record(text)}),
        )
    };
    assert_eq!(create("before").await.0, StatusCode::OK);
    let car = car_of(
        &h,
        &did,
        &[("app.bsky.feed.post/3m5aaaaaaaaaa", post_record("imported"))],
    )
    .await;
    let blocked = |what: &'static str| {
        let (h, access, did, car) = (&h, &access, &did, &car);
        async move {
            let (st, headers, body) = h
                .raw(
                    "POST",
                    "/xrpc/com.atproto.repo.createRecord",
                    Some(access),
                    serde_json::to_vec(&json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record(what)})).unwrap(),
                    &[("content-type", "application/json")],
                )
                .await;
            let v: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                (st, v["error"].as_str()),
                (StatusCode::SERVICE_UNAVAILABLE, Some("RepoWriteBlocked")),
                "{what}: {v}"
            );
            assert_eq!(
                headers.get("retry-after").and_then(|v| v.to_str().ok()),
                Some("5"),
                "{what}"
            );
            let (st, v) = h
                .post(
                    "com.atproto.repo.deleteRecord",
                    Some(access),
                    json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m5aaaaaaaaaa"}),
                )
                .await;
            assert_eq!(
                (st, v["error"].as_str()),
                (StatusCode::SERVICE_UNAVAILABLE, Some("RepoWriteBlocked")),
                "{what}: {v}"
            );
            // Another import, and a deletion, are refused too.
            let (st, v) = h.import(access, car.clone()).await;
            assert_eq!(
                (st, v["error"].as_str()),
                (StatusCode::SERVICE_UNAVAILABLE, Some("RepoWriteBlocked")),
                "{what}: {v}"
            );
            let e = crate::account::start_deletion(&h.state, did)
                .await
                .unwrap_err();
            assert_eq!(e.error, "RepoWriteBlocked", "{what}");
            let account = h.state.store.account_by_did(did).await.unwrap().unwrap();
            assert_ne!(
                account.status,
                locus_store::accounts::Status::Deleted,
                "{what}"
            );
        }
    };
    let ok = |method: &'static str, body: Value| {
        let (h, access) = (&h, &access);
        async move {
            let (st, v) = h.post(method, Some(access), body).await;
            assert_eq!(st, StatusCode::OK, "{method}: {v}");
        }
    };

    // The phases one by one, as the importRepo handler and its background
    // finish run them.
    let job = s.repo.start_import(&did, &s.node_id).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.car");
    std::fs::write(&path, &car).unwrap();
    let signer = s.signing_key(&did).await.unwrap();
    blocked("started").await;
    ok(
        "com.atproto.identity.updateHandle",
        json!({"handle": "ina2.pds.example.com"}),
    )
    .await;
    let staged = s.repo.stage_import(&job, &path, None).await.unwrap();
    blocked("staged").await;
    ok("com.atproto.server.deactivateAccount", json!({})).await;
    s.repo
        .import_increments(&job, &staged.new_rev)
        .await
        .unwrap();
    ok("com.atproto.server.activateAccount", json!({})).await;
    blocked("increments").await;
    s.repo
        .import_swap(&job, &staged.data_cid, &staged.new_rev, &signer, true)
        .await
        .unwrap();
    blocked("swapped").await;
    s.repo.import_reindex(&job, &staged.new_rev).await.unwrap();
    // Where importRepo returns: still blocked until decrements end.
    blocked("after importRepo returned").await;
    ok(
        "com.atproto.identity.updateHandle",
        json!({"handle": "ina3.pds.example.com"}),
    )
    .await;
    s.repo
        .import_decrements(&job, &staged.new_rev)
        .await
        .unwrap();

    assert_eq!(
        s.store.account_by_did(&did).await.unwrap().unwrap().handle,
        "ina3.pds.example.com"
    );
    assert_eq!(
        h.post_rkeys(&did).await,
        vec![("3m5aaaaaaaaaa".to_owned(), "imported".to_owned())]
    );
    assert_eq!(create("after").await.0, StatusCode::OK);
}

#[tokio::test]
async fn uploads_during_and_after_an_import_are_permanent_and_gated_by_the_new_tree() {
    let h = harness(false).await;
    let (did, access) = h.user("una.pds.example.com").await;
    let s = &h.state;
    let down = gate_on(&h).await;
    let (p1, q1, p2, q2) = (png("p1"), png("q1"), png("p2"), png("q2"));
    let car = car_of(
        &h,
        &did,
        &[
            (
                "app.bsky.feed.post/3m5aaaaaaaaa1",
                with_image(post_record("p1"), &p1),
            ),
            (
                "app.bsky.feed.post/3m5aaaaaaaaa2",
                with_image(post_record("p2"), &p2),
            ),
            (
                "tools.kryphocron.feed.postPrivate/3m5aaaaaaaaa3",
                with_image(private_record("q1"), &q1),
            ),
            (
                "tools.kryphocron.feed.postPrivate/3m5aaaaaaaaa4",
                with_image(private_record("q2"), &q2),
            ),
        ],
    )
    .await;
    let job = s.repo.start_import(&did, &s.node_id).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("in.car");
    std::fs::write(&path, &car).unwrap();
    let signer = s.signing_key(&did).await.unwrap();
    let staged = s.repo.stage_import(&job, &path, None).await.unwrap();
    s.repo
        .import_increments(&job, &staged.new_rev)
        .await
        .unwrap();
    // Before the swap: no record of the new tree is indexed, yet each is
    // permanent at once and the private one gated (bytes to the gated
    // store, never public).
    h.upload(&access, &p1, "image/png").await;
    h.upload(&access, &q1, "image/png").await;
    assert_blob(&h, &did, &cid_of(&p1), false, "before the swap").await;
    assert_blob(&h, &did, &cid_of(&q1), true, "before the swap").await;
    s.repo
        .import_swap(&job, &staged.data_cid, &staged.new_rev, &signer, true)
        .await
        .unwrap();
    s.repo.import_reindex(&job, &staged.new_rev).await.unwrap();
    // After importRepo would return, before decrements.
    h.upload(&access, &p2, "image/png").await;
    h.upload(&access, &q2, "image/png").await;
    assert_blob(&h, &did, &cid_of(&p2), false, "in decrements").await;
    assert_blob(&h, &did, &cid_of(&q2), true, "in decrements").await;
    let moves = s
        .repo
        .import_decrements(&job, &staged.new_rev)
        .await
        .unwrap();
    crate::repo::finish_moves(s, &moves).await;
    for (b, private) in [(&p1, false), (&q1, true), (&p2, false), (&q2, true)] {
        assert_blob(&h, &did, &cid_of(b), private, "done").await;
    }

    // An import whose blobs come later (a migration): listed as missing,
    // then each upload is permanent at once. With the scanner down the
    // private-only one is still taken, gated-only while unscanned; the
    // public one is served.
    let (r, u) = (png("r public"), png("u private"));
    let car = car_of(
        &h,
        &did,
        &[
            (
                "app.bsky.feed.post/3m6aaaaaaaaa1",
                with_image(post_record("r"), &r),
            ),
            (
                "tools.kryphocron.feed.postPrivate/3m6aaaaaaaaa2",
                with_image(private_record("u"), &u),
            ),
        ],
    )
    .await;
    let (st, v) = h.import(&access, car).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(h.import_finished(&access).await["phase"], "done");
    let (_, missing) = h
        .call(
            "GET",
            "/xrpc/com.atproto.repo.listMissingBlobs",
            Some(&access),
            None,
        )
        .await;
    let mut listed: Vec<&str> = missing["blobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["cid"].as_str().unwrap())
        .collect();
    listed.sort_unstable();
    let mut want = vec![cid_of(&r), cid_of(&u)];
    want.sort();
    assert_eq!(listed, want);
    down.store(true, std::sync::atomic::Ordering::SeqCst);
    h.upload(&access, &u, "image/png").await;
    h.upload(&access, &r, "image/png").await;
    for b in [&u, &r] {
        assert_eq!(
            s.store
                .scan_status(&did, &cid_of(b))
                .await
                .unwrap()
                .as_deref(),
            Some("unscanned")
        );
    }
    assert_blob(&h, &did, &cid_of(&u), true, "uploaded after the import").await;
    assert_blob(&h, &did, &cid_of(&r), false, "uploaded after the import").await;
    // The owner still gets the gated one.
    let (st, _, body) = h
        .raw(
            "GET",
            &format!(
                "/xrpc/com.atproto.sync.getBlob?did={did}&cid={}",
                cid_of(&u)
            ),
            Some(&access),
            vec![],
            &[],
        )
        .await;
    assert_eq!((st, body), (StatusCode::OK, u.clone()));
}

#[tokio::test]
async fn record_writes_check_netted_out_blobs_and_a_last_public_delete_leaves_a_blob_gated() {
    let h = harness(false).await;
    let (did, access) = h.user("vin.pds.example.com").await;
    let s = &h.state;
    let down = gate_on(&h).await;

    // A blob referenced and dropped in one batch takes no reference, but
    // must still exist.
    let batch = |bytes: Vec<u8>| {
        let (h, did, access) = (&h, &did, &access);
        async move {
            h.post(
                "com.atproto.repo.applyWrites",
                Some(access),
                json!({"repo": did, "writes": [
                    {"$type": "com.atproto.repo.applyWrites#create", "collection": "app.bsky.feed.post",
                     "rkey": "3m7aaaaaaaaaa", "value": with_image(post_record("brief"), &bytes)},
                    {"$type": "com.atproto.repo.applyWrites#delete", "collection": "app.bsky.feed.post",
                     "rkey": "3m7aaaaaaaaaa"},
                ]}),
            )
            .await
        }
    };
    let (st, v) = batch(png("never uploaded")).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("BlobNotFound")),
        "{v}"
    );
    let brief = png("uploaded");
    h.upload(&access, &brief, "image/png").await;
    let (st, v) = batch(brief.clone()).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let r = s.store.blob(&did, &cid_of(&brief)).await.unwrap().unwrap();
    assert_eq!(r.state, "temp", "netted out: no reference");

    // An unscanned blob a public post and a private one use: deleting the
    // public post goes through and leaves it gated-only.
    let shared = png("shared");
    let car = car_of(
        &h,
        &did,
        &[
            (
                "app.bsky.feed.post/3m8aaaaaaaaa1",
                with_image(post_record("public"), &shared),
            ),
            (
                "tools.kryphocron.feed.postPrivate/3m8aaaaaaaaa2",
                with_image(private_record("private"), &shared),
            ),
        ],
    )
    .await;
    let (st, v) = h.import(&access, car).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(h.import_finished(&access).await["phase"], "done");
    down.store(true, std::sync::atomic::Ordering::SeqCst);
    h.upload(&access, &shared, "image/png").await;
    let cid = cid_of(&shared);
    assert_blob(&h, &did, &cid, false, "public post").await;
    let (st, v) = h
        .post(
            "com.atproto.repo.deleteRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "3m8aaaaaaaaa1"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        s.store.scan_status(&did, &cid).await.unwrap().as_deref(),
        Some("unscanned")
    );
    assert_blob(&h, &did, &cid, true, "private only now").await;
}
