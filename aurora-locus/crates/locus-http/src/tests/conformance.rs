//! Answers the differential conformance run found apart from the
//! reference, pinned here so they stay matched.

use super::*;

#[tokio::test]
async fn procedures_without_input_refuse_a_body_after_auth() {
    let h = harness(false).await;
    let acct = h
        .create_account("nib.pds.example.com", "nib@example.com", "pw")
        .await;
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let refresh = acct["refreshJwt"].as_str().unwrap().to_owned();
    for nsid in NO_INPUT {
        let token = if nsid.ends_with("refreshSession") || nsid.ends_with("deleteSession") {
            &refresh
        } else {
            &access
        };
        let (st, v) = h
            .call(
                "POST",
                &format!("/xrpc/{nsid}"),
                Some(token),
                Some(json!({})),
            )
            .await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
            "{nsid}: {v}"
        );
        assert!(
            v["message"].as_str().unwrap().contains("none was expected"),
            "{nsid}: {v}"
        );
        // Without credentials, auth is what fails.
        let (st, _) = h
            .call("POST", &format!("/xrpc/{nsid}"), None, Some(json!({})))
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{nsid}");
    }
    // A bad refresh token with a body is an auth error, not a body error.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.refreshSession",
            Some("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.bm90LWEtc2lnbmF0dXJl"),
            Some(json!({})),
        )
        .await;
    assert_ne!(v["error"], "InvalidRequest", "{st} {v}");
    // The refused calls used nothing: the refresh token still works, and
    // the session is still there.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.refreshSession",
            Some(&refresh),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn migration_reads_and_writes_match_the_reference() {
    let h = harness(false).await;
    let acct = h
        .create_account("mia.pds.example.com", "mia@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let status = || async {
        h.call(
            "GET",
            "/xrpc/com.atproto.server.checkAccountStatus",
            Some(&access),
            None,
        )
        .await
        .1
    };
    let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 9, 9];
    let blob = h.upload(&access, &png, "image/png").await;
    let mut record = post_record("pic");
    record["embed"] =
        json!({"$type": "app.bsky.embed.images", "images": [{"image": blob, "alt": ""}]});
    h.post(
        "com.atproto.repo.createRecord",
        Some(&access),
        json!({"repo": did, "collection": "app.bsky.feed.post", "record": record}),
    )
    .await;
    // The blob counts are the blob rows and the blobs records reference.
    let v = status().await;
    assert_eq!(
        (v["importedBlobs"].as_i64(), v["expectedBlobs"].as_i64()),
        (Some(1), Some(1)),
        "{v}"
    );

    // A deactivated account (one migrating in) can still upload blobs.
    h.post(
        "com.atproto.server.deactivateAccount",
        Some(&access),
        json!({}),
    )
    .await;
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    let (st, _, body) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            None,
            gif.to_vec(),
            &[
                ("authorization", &format!("Bearer {access}")),
                ("content-type", "image/gif"),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let v = status().await;
    assert_eq!(
        (v["importedBlobs"].as_i64(), v["expectedBlobs"].as_i64()),
        (Some(2), Some(1)),
        "{v}"
    );

    // createAccount for a DID that has an account here: AlreadyExists.
    let (_, svc) = h
        .service_token(
            &access,
            "did:web:pds.example.com",
            "com.atproto.server.createAccount",
        )
        .await;
    let (st, v) = h
        .post(
            "com.atproto.server.createAccount",
            svc["token"].as_str(),
            json!({"did": did, "handle": "mia2.pds.example.com", "email": "mia2@example.com", "password": "pw"}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("AlreadyExists")),
        "{v}"
    );
}

#[tokio::test]
async fn sync_params_are_checked_and_admin_methods_without_output_answer_empty() {
    let h = harness(false).await;
    let acct = h
        .create_account("sid.pds.example.com", "sid@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    for q in [
        "com.atproto.sync.getRepo?did=not-a-did".to_owned(),
        format!("com.atproto.sync.getRepo?did={did}&since=nope"),
        format!("com.atproto.sync.getBlocks?did={did}"),
        format!("com.atproto.sync.getRecord?did={did}&collection=nope&rkey=self"),
        "com.atproto.sync.getRepoStatus?did=nope".to_owned(),
        "com.atproto.sync.getLatestCommit?did=nope".to_owned(),
        format!("com.atproto.sync.listBlobs?did={did}&since=nope"),
    ] {
        let (st, _, body) = h
            .raw("GET", &format!("/xrpc/{q}"), None, Vec::new(), &[])
            .await;
        let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
            "{q}: {v}"
        );
    }

    let admin = h.basic_admin();
    for (nsid, body) in [
        ("disableAccountInvites", json!({"account": did})),
        ("enableAccountInvites", json!({"account": did})),
        (
            "disableInviteCodes",
            json!({"codes": ["nope"], "accounts": []}),
        ),
        (
            "updateAccountEmail",
            json!({"account": did, "email": "sid2@example.com"}),
        ),
        (
            "updateAccountPassword",
            json!({"did": did, "password": "pw2"}),
        ),
        (
            "updateAccountHandle",
            json!({"did": did, "handle": "sid2.pds.example.com"}),
        ),
        ("deleteAccount", json!({"did": did})),
    ] {
        let (st, _, out) = h
            .raw(
                "POST",
                &format!("/xrpc/com.atproto.admin.{nsid}"),
                None,
                serde_json::to_vec(&body).unwrap(),
                &[
                    ("authorization", &admin),
                    ("content-type", "application/json"),
                ],
            )
            .await;
        assert_eq!((st, out.as_slice()), (StatusCode::OK, &b""[..]), "{nsid}");
    }
}

#[tokio::test]
async fn a_cursor_that_is_not_an_integer_gets_an_error_frame() {
    use futures::StreamExt;
    let h = harness(false).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    let (mut ws, _) = tokio_tungstenite::connect_async(format!(
        "ws://{addr}/xrpc/com.atproto.sync.subscribeRepos?cursor=abc"
    ))
    .await
    .expect("the upgrade is accepted");
    let next = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await;
    let Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b)))) = next else {
        panic!("an error frame");
    };
    match proto_blue_ws::frame::Frame::decode(&b).unwrap() {
        proto_blue_ws::frame::Frame::Error(e) => assert_eq!(e.error, "InvalidRequest"),
        proto_blue_ws::frame::Frame::Message(_) => panic!("an error frame"),
    }
}

#[tokio::test]
async fn oauth_errors_keep_the_reference_names() {
    const LB: &str = "http://localhost?scope=atproto+transition%3Ageneric";
    let h = harness(false).await;
    let client = DpopClient::new();
    let (_, challenge) = pkce();
    // PAR keeps unsupported_response_type.
    let (st, _, v) = client
        .post(
            &h,
            "/oauth/par",
            &[
                ("client_id", LB),
                ("response_type", "token"),
                ("scope", "atproto transition:generic"),
                ("redirect_uri", "http://127.0.0.1:5173/"),
                ("code_challenge_method", "S256"),
                ("code_challenge", &challenge),
            ],
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("unsupported_response_type")),
        "{v}"
    );
    // A DPoP proof is checked before the token: a proof without `ath` for
    // a token no server issued is a proof error.
    let url = "https://pds.example.com/xrpc/com.atproto.server.getSession";
    let proof = client.proof("GET", url, None);
    let (st, _, body) = h
        .raw(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            None,
            Vec::new(),
            &[
                (
                    "authorization",
                    "DPoP eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJ4In0.bm90LWEtc2lnbmF0dXJl",
                ),
                ("dpop", &proof),
            ],
        )
        .await;
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("invalid_dpop_proof")),
        "{v}"
    );
}
