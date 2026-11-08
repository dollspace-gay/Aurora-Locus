//! A shard's entryway keys: genesis ops under each accepted PLC rotation
//! key, verify keys with real tokens, and the entryway's token-key
//! rotation across two shards.

use super::*;
use proto_blue_crypto::K256Keypair;

const EW_DID: &str = "did:web:entryway.test";

fn ns(m: &str) -> String {
    format!("/xrpc/{}.{m}", locus_core::LOCUS_NS)
}

/// A shard whose stored entryway values are verify key `v` and PLC
/// rotation key `p`, as a node's first boot leaves them.
async fn shard_with(v: &K256Keypair, p: &K256Keypair) -> Harness {
    let (url, _) = mock_entryway().await;
    let h = shard_harness(crate::shard::Entryway::for_test(
        &url,
        EW_DID,
        &[v.did()],
        &[p.did()],
    ))
    .await;
    let store = &h.state.store;
    store
        .set_setting("entryway.url", json!(url), "test")
        .await
        .unwrap();
    store
        .set_setting("entryway.did", json!(EW_DID), "test")
        .await
        .unwrap();
    store
        .set_setting("entryway.plc_rotation_keys", json!([p.did()]), "test")
        .await
        .unwrap();
    let mut tx = store.begin().await.unwrap();
    tx.replace_shard_verify_keys(&[v.did()]).await.unwrap();
    tx.commit().await.unwrap();
    h
}

/// What a node does after a key change: loads its entryway values again.
async fn reload(h: Harness) -> Harness {
    let Harness {
        app,
        state,
        plc,
        mail,
        _dir,
    } = h;
    drop(app);
    let Ok(mut st) = Arc::try_unwrap(state) else {
        panic!("the state is still shared");
    };
    let snap = st.store.load_snapshot().await.unwrap();
    st.entryway = Some(Arc::new(
        crate::shard::Entryway::load(
            &st.store,
            &st.master_key,
            &HashMap::new(),
            &snap,
            &st.secrets.admin_password,
        )
        .await
        .unwrap(),
    ));
    let state = Arc::new(st);
    Harness {
        app: crate::router(state.clone()),
        state,
        plc,
        mail,
        _dir,
    }
}

/// createAccount on the shard with a genesis op listing `rotation`,
/// signed by `signer`: (status, body, DID).
async fn genesis(
    h: &Harness,
    signer: &K256Keypair,
    rotation: &[&K256Keypair],
    handle: &str,
) -> (StatusCode, Value, String) {
    let (_, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.reserveSigningKey",
            None,
            Some(json!({})),
        )
        .await;
    let reserved = v["signingKey"].as_str().unwrap().to_owned();
    let op = locus_identity::plc::Operation {
        rotation_keys: rotation.iter().map(|k| k.did()).collect(),
        verification_methods: [("atproto".to_owned(), reserved)].into(),
        also_known_as: vec![format!("at://{handle}")],
        services: [(
            "atproto_pds".to_owned(),
            locus_identity::plc::Service {
                kind: "AtprotoPersonalDataServer".into(),
                endpoint: "https://pds.example.com".into(),
            },
        )]
        .into(),
        prev: None,
    }
    .sign(signer)
    .unwrap();
    let did = op.genesis_did().unwrap();
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": handle, "did": did, "plcOp": op.to_json()})),
        )
        .await;
    (st, v, did)
}

/// An entryway access token for `did` on the shard, signed by `key`.
fn access(key: &K256Keypair, did: &str) -> String {
    let now = locus_core::time::now_ms() / 1000;
    es256k_jwt(
        key,
        "at+jwt",
        &json!({"scope": "com.atproto.access", "aud": "did:web:pds.example.com",
            "sub": did, "iat": now, "exp": now + 600}),
    )
}

/// `keys set <name>` with a did:key value, as `auth`.
async fn set_key(
    h: &Harness,
    auth: &str,
    name: &str,
    key: &K256Keypair,
    revoke: bool,
    password: &str,
) -> (StatusCode, Value) {
    h.admin_call(
        "POST",
        &ns("owner.keys.setSecret"),
        auth,
        Some(
            json!({"name": name, "value": key.did(), "revokePrevious": revoke,
            "password": password}),
        ),
    )
    .await
}

async fn stored_verify_keys(h: &Harness) -> Vec<String> {
    let mut k: Vec<String> = h
        .state
        .store
        .shard_verify_keys()
        .await
        .unwrap()
        .into_iter()
        .map(|k| k.did_key)
        .collect();
    k.sort();
    k
}

async fn status_with(h: &Harness, token: &str) -> StatusCode {
    h.call(
        "GET",
        "/xrpc/com.atproto.server.checkAccountStatus",
        Some(token),
        None,
    )
    .await
    .0
}

#[tokio::test]
async fn a_shard_takes_genesis_ops_from_each_entryway_key_until_revoked() {
    let v = K256Keypair::generate();
    let (c, p, p2) = (
        K256Keypair::generate(),
        K256Keypair::generate(),
        K256Keypair::generate(),
    );
    let h = shard_with(&v, &c).await;
    let owner = h.owner_with_password("sara", "sara-password-123").await;
    const PLC: &str = "entryway-plc-rotation-key";
    const PW: &str = "sara-password-123";

    // The entryway replaced a pending key before confirming it: P, then
    // P', in quick succession (the node hasn't reloaded in between).
    let (st, r) = set_key(&h, &owner, PLC, &p, false, PW).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let (st, r) = set_key(&h, &owner, PLC, &p2, false, PW).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(
        r["accepted"],
        json!([p2.did(), p.did(), c.did()]),
        "every key since the last revoke is kept"
    );
    let h = reload(h).await;

    // Genesis ops signed by each of C, P and P' are accepted.
    let mut dids = Vec::new();
    for (i, k) in [&c, &p, &p2].into_iter().enumerate() {
        let (st, r, did) = genesis(&h, k, &[k], &format!("k{i}.test")).await;
        assert_eq!(st, StatusCode::OK, "key {i}: {r}");
        assert!(h.plc.lock().unwrap().last(&did).is_some());
        dids.push(did);
    }
    // P' is the one recommended.
    let (st, r) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            Some(&access(&v, &dids[0])),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["rotationKeys"], json!([p2.did()]));

    // An op naming P' but signed by another key it lists: refused, and
    // nothing reaches PLC.
    let other = K256Keypair::generate();
    let (st, r, did) = genesis(&h, &other, &[&other, &p2], "forged.test").await;
    assert_eq!(
        (st, r["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("IncompatibleDidDoc")),
        "{r}"
    );
    assert!(h.plc.lock().unwrap().last(&did).is_none());
    assert!(h.state.store.account_by_did(&did).await.unwrap().is_none());

    // --revoke-previous: only P' from now on.
    let (st, r) = set_key(&h, &owner, PLC, &p2, true, PW).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["accepted"], json!([p2.did()]));
    let h = reload(h).await;
    for (i, k) in [&c, &p].into_iter().enumerate() {
        // Signed by the revoked key, listing it (and P' too).
        let (st, r, did) = genesis(&h, k, &[k, &p2], &format!("late{i}.test")).await;
        assert_eq!(
            (st, r["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("IncompatibleDidDoc")),
            "revoked key {i}: {r}"
        );
        assert!(
            h.plc.lock().unwrap().last(&did).is_none(),
            "nothing submitted on refusal"
        );
    }
    let (st, r, _) = genesis(&h, &p2, &[&p2], "after.test").await;
    assert_eq!(st, StatusCode::OK, "{r}");
}

#[tokio::test]
async fn a_shard_keeps_a_previous_verify_key_for_its_overlap_only() {
    let (v0, p) = (K256Keypair::generate(), K256Keypair::generate());
    let h = shard_with(&v0, &p).await;
    let (st, r, did) = genesis(&h, &p, &[&p], "vera.test").await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let owner = h.owner_with_password("sara", "sara-password-123").await;
    let admin = format!("Bearer {}", h.operator("adam", "admin").await);
    let v1 = K256Keypair::generate();
    const VERIFY: &str = "entryway-verify-key";
    const PW: &str = "sara-password-123";

    // Not the owner, or a wrong password: refused, nothing changes.
    let (st, r) = set_key(&h, &admin, VERIFY, &v1, false, PW).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{r}");
    let (st, r) = set_key(&h, &owner, VERIFY, &v1, false, "wrong-password").await;
    assert_ne!(st, StatusCode::OK, "{r}");
    assert_eq!(stored_verify_keys(&h).await, [v0.did()]);

    // A new key: tokens under both are accepted.
    let (st, r) = set_key(&h, &owner, VERIFY, &v1, false, PW).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    // As if the overlap were nearly over.
    let Some(pool) = sqlite_pool(&h.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };
    sqlx::query("UPDATE shard_verify_key SET expires_at = ?1 WHERE did_key = ?2")
        .bind(locus_core::time::now_ms() + 1500)
        .bind(v0.did())
        .execute(pool)
        .await
        .unwrap();
    let h = reload(h).await;
    assert_eq!(status_with(&h, &access(&v0, &did)).await, StatusCode::OK);
    assert_eq!(status_with(&h, &access(&v1, &did)).await, StatusCode::OK);

    // Once the overlap ends the old key is refused, without a reload.
    tokio::time::sleep(std::time::Duration::from_millis(1700)).await;
    assert_eq!(
        status_with(&h, &access(&v0, &did)).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(status_with(&h, &access(&v1, &did)).await, StatusCode::OK);
    let h = reload(h).await;
    assert_eq!(
        status_with(&h, &access(&v0, &did)).await,
        StatusCode::BAD_REQUEST
    );

    // --revoke-previous: the previous key is refused at once.
    let v2 = K256Keypair::generate();
    let (st, r) = set_key(&h, &owner, VERIFY, &v2, true, PW).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let h = reload(h).await;
    assert_eq!(stored_verify_keys(&h).await, [v2.did()]);
    assert_eq!(
        status_with(&h, &access(&v1, &did)).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(status_with(&h, &access(&v2, &did)).await, StatusCode::OK);
}

/// A locus shard at `hostname` behind the entryway `e`, served over TCP.
async fn served_shard(e: &Harness, e_url: &str, hostname: &str) -> (Harness, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let token = crate::entryway::token_key(&e.state).await.unwrap().did();
    let s = harness_build(HarnessOpts {
        hostname: hostname.into(),
        entryway: Some(Arc::new(crate::shard::Entryway::for_test(
            e_url,
            "did:web:entryway.example.com",
            &[token],
            &[e.state.secrets.plc_keys.current_did().unwrap()],
        ))),
        plc: Some((
            e.state.snapshot().text("identity.plc_url").to_owned(),
            e.plc.clone(),
        )),
        public_url: Some(url.clone()),
        ..HarnessOpts::default()
    })
    .await;
    serve_on(&s, l);
    (s, url)
}

#[tokio::test]
async fn an_entryway_key_rotation_waits_for_an_offline_shard_and_keeps_the_old_key_two_hours() {
    let (e, s1, s1_url) = entryway_and_shard().await;
    let e_url = e.state.public_url.clone();
    let (s2, s2_url) = served_shard(&e, &e_url, "pds2.example.com").await;
    let owner = e.owner_with_password("ena", "ena-password-123").await;
    for (name, s, url) in [("s1", &s1, &s1_url), ("s2", &s2, &s2_url)] {
        let pw = String::from_utf8(s.state.secrets.admin_password.to_vec()).unwrap();
        let (st, v) = e
            .admin_call(
                "POST",
                &ns("owner.shard.add"),
                &owner,
                Some(json!({"url": url, "adminPassword": pw, "name": name})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    // A token the entryway issued under its first key.
    let (st, acct) = e
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(
                json!({"handle": "olga.entryway.example.com", "email": "olga@example.com",
                "password": "olga-password"}),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{acct}");
    let old_token = acct["accessJwt"].as_str().unwrap().to_owned();
    // The entryway's own sessions, as a node loads them after each change.
    let es = &e;
    let entryway_accepts = |t: String| async move {
        let keys = crate::entryway::entryway_keys(&es.state.store, &es.state.master_key)
            .await
            .unwrap();
        LegacySessions::new(
            es.state.store.clone(),
            es.state.secrets.jwt.clone(),
            es.state.service_did.clone(),
        )
        .with_entryway(keys)
        .verify_access(&t)
        .await
        .is_ok()
    };
    assert!(entryway_accepts(old_token.clone()).await);

    // s2 is offline when the rotation starts.
    let Some(pool) = sqlite_pool(&e.state.store) else {
        return; // SQLite-only: it reads or sets rows directly
    };
    let dead = format!("http://127.0.0.1:{}", {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    });
    sqlx::query("UPDATE shard SET url = ?1 WHERE name = 's2'")
        .bind(&dead)
        .execute(pool)
        .await
        .unwrap();
    let (st, v) = e
        .admin_call(
            "POST",
            &ns("owner.keys.rotateEntrywayKey"),
            &owner,
            Some(json!({"password": "ena-password-123"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (old, new) = (
        v["oldKey"].as_str().unwrap().to_owned(),
        v["newKey"].as_str().unwrap().to_owned(),
    );
    let mut both = vec![old.clone(), new.clone()];
    both.sort();
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    crate::entryway::rotation_step(&e.state).await.unwrap();
    assert_eq!(stored_verify_keys(&s1).await, both);
    assert_eq!(stored_verify_keys(&s2).await, [old.clone()], "s2 missed it");
    let rotation = || async { e.state.store.key_rotation().await.unwrap().unwrap() };
    assert!(
        rotation().await.switched_at.is_none(),
        "no switch while a shard lacks the new key"
    );
    assert_eq!(
        crate::entryway::token_key(&e.state).await.unwrap().did(),
        old
    );
    // Both keys are accepted on the entryway already.
    assert!(entryway_accepts(old_token.clone()).await);

    // s2 comes back and catches up from the queue; then signing switches.
    sqlx::query("UPDATE shard SET url = ?1 WHERE name = 's2'")
        .bind(&s2_url)
        .execute(pool)
        .await
        .unwrap();
    // The failed push waits for its retry; let the backoff pass.
    let tried: i64 =
        sqlx::query_scalar("SELECT attempts FROM shard_outbox WHERE shard = 's2' ORDER BY id")
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(tried >= 1);
    sqlx::query("UPDATE shard_outbox SET next_at = 0 WHERE shard = 's2'")
        .execute(pool)
        .await
        .unwrap();
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    assert_eq!(stored_verify_keys(&s2).await, both);
    crate::entryway::rotation_step(&e.state).await.unwrap();
    assert!(rotation().await.switched_at.is_some());
    assert_eq!(
        crate::entryway::token_key(&e.state).await.unwrap().did(),
        new
    );
    // Within two hours of the switch the entryway takes old-key tokens.
    sqlx::query("UPDATE entryway_key_rotation SET switched_at = switched_at - 7000000")
        .execute(pool)
        .await
        .unwrap();
    crate::entryway::rotation_step(&e.state).await.unwrap();
    assert!(rotation().await.finished_at.is_none());
    assert!(entryway_accepts(old_token.clone()).await);

    // After two hours: finished, both shards keep only the new key, and
    // the entryway refuses the old key's tokens.
    sqlx::query("UPDATE entryway_key_rotation SET switched_at = switched_at - 300000")
        .execute(pool)
        .await
        .unwrap();
    crate::entryway::rotation_step(&e.state).await.unwrap();
    assert!(rotation().await.finished_at.is_some());
    crate::entryway::deliver_pushes(&e.state).await.unwrap();
    assert_eq!(stored_verify_keys(&s1).await, [new.clone()]);
    assert_eq!(stored_verify_keys(&s2).await, [new.clone()]);
    assert!(!entryway_accepts(old_token).await);
}
