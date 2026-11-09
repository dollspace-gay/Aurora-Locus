//! `getRepo` while the repo changes: a commit landing mid-stream must not
//! leave the CAR missing blocks the root it was started at needs.

use super::*;

const POSTS: usize = 1200;

#[tokio::test]
async fn a_commit_mid_stream_leaves_get_repo_complete() {
    let h = harness(false).await;
    let acct = h
        .create_account("race.pds.example.com", "race@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    // Enough records that the CAR takes more than one page.
    let tids = TidClock::new(1);
    let rkeys: Vec<String> = (0..POSTS).map(|_| tids.next()).collect();
    for batch in rkeys.chunks(100) {
        let writes: Vec<Value> = batch
            .iter()
            .map(|rkey| {
                json!({
                    "$type": "com.atproto.repo.applyWrites#create",
                    "collection": "app.bsky.feed.post",
                    "rkey": rkey,
                    "value": post_record(rkey),
                })
            })
            .collect();
        let (st, v) = h
            .post(
                "com.atproto.repo.applyWrites",
                Some(&access),
                json!({"repo": did, "writes": writes}),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    let signing = h.state.signing_key(&did).await.unwrap().did();
    let path = format!("/xrpc/com.atproto.sync.getRepo?did={did}");

    // Quiet: the CAR verifies.
    let (_, _, car) = h.raw_get(path.clone()).await;
    proto_blue_repo::sync::verify_repo_car(&car, Some(&did), Some(&signing)).unwrap();

    // Start a stream and read the header and the first page.
    let req = Request::builder()
        .uri(&path)
        .header("host", h.state.hostname.clone())
        .body(Body::empty())
        .unwrap();
    let res = h.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let mut body = res.into_body();
    let mut car = Vec::new();
    for _ in 0..2 {
        let frame = body.frame().await.unwrap().unwrap();
        car.extend_from_slice(frame.data_ref().unwrap());
    }

    // A commit lands: it drops the old commit block and much of the tree.
    let deletes: Vec<Value> = rkeys
        .iter()
        .step_by(POSTS / 200)
        .map(|rkey| {
            json!({
                "$type": "com.atproto.repo.applyWrites#delete",
                "collection": "app.bsky.feed.post",
                "rkey": rkey,
            })
        })
        .collect();
    let (st, v) = h
        .post(
            "com.atproto.repo.applyWrites",
            Some(&access),
            json!({"repo": did, "writes": deletes}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // The rest of the stream still completes the CAR it started.
    car.extend_from_slice(&body.collect().await.unwrap().to_bytes());
    let repo = proto_blue_repo::sync::verify_repo_car(&car, Some(&did), Some(&signing))
        .expect("the CAR has every block its root needs");
    let (_, latest) = h
        .get(format!("/xrpc/com.atproto.sync.getLatestCommit?did={did}"))
        .await;
    assert_ne!(
        repo.commit_cid.to_string_base32(),
        latest["cid"].as_str().unwrap(),
        "rooted at the commit before the deletes"
    );

    // A new stream sends the new repo.
    let (_, _, now) = h.raw_get(path).await;
    let repo = proto_blue_repo::sync::verify_repo_car(&now, Some(&did), Some(&signing)).unwrap();
    assert_eq!(
        repo.commit_cid.to_string_base32(),
        latest["cid"].as_str().unwrap()
    );
}

/// Block GC keeps dropped blocks longer than any stream may last, which the
/// fix above relies on.
#[test]
fn block_gc_grace_outlasts_the_longest_get_repo_stream() {
    let def = locus_core::settings::def("sync.get_repo_timeout_ms").unwrap();
    let locus_core::settings::Kind::Int { max, .. } = def.kind else {
        panic!("an int setting");
    };
    assert!(locus_store::repo::BLOCK_GC_GRACE_MS > max);
}
