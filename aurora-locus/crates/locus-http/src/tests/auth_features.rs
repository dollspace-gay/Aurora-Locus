//! Inbound service-auth replay and lifetime rules, hCaptcha on web sign-up,
//! and an explicit service DID (`PDS_SERVICE_DID`).

use super::*;

fn now_s() -> i64 {
    locus_core::time::now_ms() / 1000
}

/// A DID in the mock PLC directory whose `#atproto` key is `key`.
fn plc_did(h: &Harness, did: &str, key: &proto_blue_crypto::K256Keypair) {
    h.plc.lock().unwrap().log.insert(
        did.into(),
        vec![json!({
            "type": "plc_operation",
            "alsoKnownAs": [],
            "verificationMethods": {"atproto": key.did()},
            "services": {},
        })],
    );
}

// ---------------------------------------------------------------- service auth

const MOD_DID: &str = "did:plc:modmodmodmodmodmodmodmodm";
const UPDATE: &str = "com.atproto.admin.updateSubjectStatus";
const STATUS: &str = "com.atproto.admin.getSubjectStatus";

/// A node whose moderation service is `MOD_DID`, signing with the key
/// returned, and an account on it.
async fn moderated(o: HarnessOpts) -> (Harness, proto_blue_crypto::K256Keypair, String) {
    let h = harness_build(o).await;
    let key = proto_blue_crypto::K256Keypair::generate();
    plc_did(&h, MOD_DID, &key);
    h.state
        .settings
        .set("moderation.service_did", json!(MOD_DID), "test")
        .await
        .unwrap();
    let (did, _) = h.user("tara.pds.example.com").await;
    (h, key, did)
}

/// The moderation service's call to `lxm` with `token`.
async fn mod_call(h: &Harness, lxm: &str, did: &str, token: &str) -> (StatusCode, Value) {
    if lxm == UPDATE {
        h.post(
            UPDATE,
            Some(token),
            json!({"subject": {"$type": "com.atproto.admin.defs#repoRef", "did": did},
                   "takedown": {"applied": false}}),
        )
        .await
    } else {
        h.call("GET", &format!("/xrpc/{lxm}?did={did}"), Some(token), None)
            .await
    }
}

fn mod_token(key: &proto_blue_crypto::K256Keypair, lxm: &str) -> String {
    let now = now_s();
    locus_auth::service_jwt::create(
        key,
        MOD_DID,
        "did:web:pds.example.com",
        None,
        Some(lxm),
        now,
    )
    .unwrap()
}

/// The refusal a replayed or `jti`-less procedure token gets.
fn refused(r: &(StatusCode, Value), message: &str) -> bool {
    r.0 == StatusCode::UNAUTHORIZED && r.1["error"] == "BadJwt" && r.1["message"] == message
}

/// Procedures take a service token once; queries take it again.
async fn replay_rules(o: HarnessOpts) {
    let (h, key, did) = moderated(o).await;

    // A procedure (every com.atproto.admin.* procedure): the second use of
    // the same token is refused, a fresh token works.
    let t = mod_token(&key, UPDATE);
    let first = mod_call(&h, UPDATE, &did, &t).await;
    assert_eq!(first.0, StatusCode::OK, "{}", first.1);
    let again = mod_call(&h, UPDATE, &did, &t).await;
    assert!(refused(&again, "jwt replayed"), "{again:?}");
    let fresh = mod_call(&h, UPDATE, &did, &mod_token(&key, UPDATE)).await;
    assert_eq!(fresh.0, StatusCode::OK, "{}", fresh.1);

    // A procedure token without `jti` is refused, even on first use.
    let now = now_s();
    let claims = |jti: Option<&str>, lxm: &str| {
        let mut c = json!({"iss": MOD_DID, "aud": "did:web:pds.example.com", "lxm": lxm,
                           "iat": now, "exp": now + 60});
        if let Some(j) = jti {
            c["jti"] = json!(j);
        }
        c
    };
    let no_jti = es256k_jwt(&key, "JWT", &claims(None, UPDATE));
    let r = mod_call(&h, UPDATE, &did, &no_jti).await;
    assert!(refused(&r, "jwt has no jti"), "{r:?}");

    // A forged token (another key) carrying a real token's jti doesn't use
    // that jti up: only a verified token is recorded.
    let jti = hex::encode(rand::random::<[u8; 8]>());
    let jti = jti.as_str();
    let forged = es256k_jwt(
        &proto_blue_crypto::K256Keypair::generate(),
        "JWT",
        &claims(Some(jti), UPDATE),
    );
    let r = mod_call(&h, UPDATE, &did, &forged).await;
    assert_eq!(r.1["error"], "BadJwtSignature", "{r:?}");
    let real = es256k_jwt(&key, "JWT", &claims(Some(jti), UPDATE));
    let r = mod_call(&h, UPDATE, &did, &real).await;
    assert_eq!(r.0, StatusCode::OK, "{}", r.1);
    let r = mod_call(&h, UPDATE, &did, &real).await;
    assert!(refused(&r, "jwt replayed"), "{r:?}");

    // A query: the same token works again, and needs no jti.
    let q = mod_token(&key, STATUS);
    for n in 0..3 {
        let r = mod_call(&h, STATUS, &did, &q).await;
        assert_eq!(r.0, StatusCode::OK, "use {n}: {}", r.1);
    }
    let q = es256k_jwt(&key, "JWT", &claims(None, STATUS));
    for _ in 0..2 {
        let r = mod_call(&h, STATUS, &did, &q).await;
        assert_eq!(r.0, StatusCode::OK, "{}", r.1);
    }

    // uploadBlob (an idempotent procedure) with user service auth: a retry
    // with the same token works.
    let (_, access) = h.user("ulla.pds.example.com").await;
    let (st, v) = h
        .service_token(
            &access,
            "did:web:pds.example.com",
            "com.atproto.repo.uploadBlob",
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let token = v["token"].as_str().unwrap().to_owned();
    for _ in 0..2 {
        let (st, _, body) = h
            .raw(
                "POST",
                "/xrpc/com.atproto.repo.uploadBlob",
                Some(&token),
                b"same bytes".to_vec(),
                &[("content-type", "text/plain")],
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    }
}

#[tokio::test]
async fn procedure_service_tokens_are_single_use_and_query_tokens_reusable() {
    replay_rules(HarnessOpts::default()).await;
}

/// With Redis, the replay window is there, so a token used on one node is
/// refused on another sharing it.
#[tokio::test]
async fn the_service_auth_replay_window_is_shared_through_redis() {
    let Ok(url) = std::env::var("LOCUS_TEST_REDIS_URL") else {
        return;
    };
    let kv = crate::scratch::Scratch::connect(&url).await.unwrap();
    replay_rules(HarnessOpts {
        kv: kv.clone(),
        ..HarnessOpts::default()
    })
    .await;
    let plc = mock_plc().await;
    let (a, key, did) = moderated(HarnessOpts {
        kv: kv.clone(),
        plc: Some(plc.clone()),
        ..HarnessOpts::default()
    })
    .await;
    let b = harness_build(HarnessOpts {
        kv,
        plc: Some(plc),
        ..HarnessOpts::default()
    })
    .await;
    b.state
        .settings
        .set("moderation.service_did", json!(MOD_DID), "test")
        .await
        .unwrap();
    let t = mod_token(&key, UPDATE);
    let r = mod_call(&a, UPDATE, &did, &t).await;
    assert_eq!(r.0, StatusCode::OK, "{}", r.1);
    let r = mod_call(&b, UPDATE, &did, &t).await;
    assert!(refused(&r, "jwt replayed"), "the other node: {r:?}");
}

/// `exp - iat` is at most an hour plus a minute; without `iat`, `exp - now`
/// at most an hour. On queries too.
#[tokio::test]
async fn inbound_service_tokens_live_at_most_an_hour() {
    let (h, key, did) = moderated(HarnessOpts::default()).await;
    let now = now_s();
    let token = |iat: Option<i64>, exp: i64, lxm: &str| {
        let mut c = json!({"iss": MOD_DID, "aud": "did:web:pds.example.com", "lxm": lxm,
                           "exp": exp, "jti": hex::encode(rand::random::<[u8; 8]>())});
        if let Some(i) = iat {
            c["iat"] = json!(i);
        }
        es256k_jwt(&key, "JWT", &c)
    };
    let too_long = "jwt lifetime is longer than an hour";
    for lxm in [STATUS, UPDATE] {
        // At the limit: accepted.
        let r = mod_call(&h, lxm, &did, &token(Some(now - 3000), now + 660, lxm)).await;
        assert_eq!(r.0, StatusCode::OK, "{lxm} 3660 s: {}", r.1);
        let r = mod_call(&h, lxm, &did, &token(None, now + 3500, lxm)).await;
        assert_eq!(r.0, StatusCode::OK, "{lxm} no iat: {}", r.1);
        // With `iat`, it's `exp - iat` that counts, not `exp - now`.
        let r = mod_call(&h, lxm, &did, &token(Some(now + 50), now + 3700, lxm)).await;
        assert_eq!(r.0, StatusCode::OK, "{lxm} iat ahead: {}", r.1);
        // Past it: refused.
        let r = mod_call(&h, lxm, &did, &token(Some(now), now + 3661, lxm)).await;
        assert!(refused(&r, too_long), "{lxm} 3661 s: {r:?}");
        let r = mod_call(&h, lxm, &did, &token(None, now + 3700, lxm)).await;
        assert!(refused(&r, too_long), "{lxm} no iat: {r:?}");
    }
}

// ---------------------------------------------------------------- hCaptcha

/// What the mock siteverify was sent.
type Verifies = Arc<Mutex<Vec<HashMap<String, String>>>>;

/// A mock hCaptcha siteverify: `good` passes, anything else fails;
/// `down` answers 500.
async fn siteverify() -> (String, Verifies) {
    let seen: Verifies = Arc::default();
    let app = Router::new()
        .route(
            "/siteverify",
            post(
                |State(seen): State<Verifies>, body: axum::body::Bytes| async move {
                    let form: HashMap<String, String> =
                        url::form_urlencoded::parse(&body).into_owned().collect();
                    let token = form.get("response").cloned().unwrap_or_default();
                    seen.lock().unwrap().push(form);
                    if token == "down" {
                        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                    }
                    axum::Json(json!({
                        "success": token == "good",
                        "challenge_ts": "2026-01-01T00:00:00Z",
                        "hostname": "pds.example.com",
                    }))
                    .into_response()
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/siteverify"), seen)
}

/// A node with hCaptcha configured: the secret from env on first boot, the
/// site key and the mock siteverify as settings.
async fn hcaptcha_harness() -> (Harness, Verifies) {
    let env: HashMap<String, String> = [
        ("PDS_HCAPTCHA_SECRET_KEY", "the-hcaptcha-secret"),
        ("PDS_HCAPTCHA_TOKEN_SALT", "the-salt"),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    let h = harness_build(HarnessOpts {
        env: Some(env),
        ..HarnessOpts::default()
    })
    .await;
    let (url, seen) = siteverify().await;
    for (k, v) in [
        ("accounts.hcaptcha_site_key", "site-key-1"),
        ("accounts.hcaptcha_verify_url", url.as_str()),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    (h, seen)
}

async fn portal_sign_up(
    h: &Harness,
    handle: &str,
    captcha: Option<&str>,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let b = Browser::default();
    let (_, _, page) = b.get(h, "/account/sign-up").await;
    let csrf = hidden(&page, "csrf");
    let email = format!("{handle}@example.com");
    let mut form = vec![
        ("csrf", csrf.as_str()),
        ("handle", handle),
        ("email", email.as_str()),
        ("password", "a fine password"),
    ];
    if let Some(c) = captcha {
        form.push(("h-captcha-response", c));
    }
    b.post(h, "/account/sign-up", &form).await
}

async fn exists(h: &Harness, handle: &str) -> bool {
    h.state
        .store
        .account_by_handle(&format!("{handle}.pds.example.com"))
        .await
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn web_sign_up_needs_a_valid_hcaptcha_token_when_configured() {
    let (h, seen) = hcaptcha_harness().await;

    // The portal's form carries the widget, and its CSP lets it in.
    let (_, headers, page) = Browser::default().get(&h, "/account/sign-up").await;
    assert!(
        page.contains(r#"class="h-captcha" data-sitekey="site-key-1""#)
            && page.contains(crate::hcaptcha::SCRIPT),
        "{page}"
    );
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(
        csp.contains("script-src https://hcaptcha.com https://*.hcaptcha.com")
            && csp.contains("frame-src https://hcaptcha.com"),
        "{csp}"
    );

    // Missing, invalid, and unverifiable tokens: refused with the
    // reference's messages, no account made.
    let (st, _, page) = portal_sign_up(&h, "nia", None).await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("hCaptcha token is required"),
        "{st} {page}"
    );
    assert!(seen.lock().unwrap().is_empty(), "nothing to verify");
    for bad in ["wrong", "down"] {
        let (st, _, page) = portal_sign_up(&h, "nia", Some(bad)).await;
        assert!(
            st == StatusCode::BAD_REQUEST && page.contains("hCaptcha verification failed"),
            "{bad}: {st} {page}"
        );
    }
    assert!(!exists(&h, "nia").await);

    // A valid one: signed up, and siteverify got the reference's fields.
    let (st, headers, page) = portal_sign_up(&h, "nia", Some("good")).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{page}");
    assert_eq!(headers["location"], "/account");
    assert!(exists(&h, "nia").await);
    let sent = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(sent["secret"], "the-hcaptcha-secret");
    assert_eq!(sent["sitekey"], "site-key-1");
    assert_eq!(sent["behavior_type"], "signup");
    assert_eq!(sent["response"], "good");
    let tokens: Value = serde_json::from_str(&sent["client_tokens"]).unwrap();
    // scrypt(handle, salt, N = 1024, r = 8, p = 1, 32 bytes), as the reference.
    assert_eq!(
        tokens["hashedHandle"],
        "jokLkoBAmWuLXoue3kIqNbFfpHVGMAyVPAGJkmrhvhc="
    );
    assert!(tokens["hashedIp"].is_string());

    // createAccount itself isn't gated, as the reference.
    h.create_account("omar.pds.example.com", "omar@example.com", "pw")
        .await;
}

#[tokio::test]
async fn oauth_sign_up_needs_a_valid_hcaptcha_token_when_configured() {
    const LB: &str = "http://localhost?scope=atproto+transition%3Ageneric";
    let (h, seen) = hcaptcha_harness().await;
    let client = DpopClient::new();
    let (_, challenge) = pkce();
    let form = [
        ("client_id", LB),
        ("response_type", "code"),
        ("scope", "atproto transition:generic"),
        ("redirect_uri", "http://127.0.0.1/"),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("prompt", "create"),
    ];
    let (_, _, v) = client.post(&h, "/oauth/par", &form).await;
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let path = format!(
        "/oauth/authorize?client_id={}&request_uri={}",
        enc(LB),
        enc(v["request_uri"].as_str().unwrap())
    );
    let b = Browser::default();
    let (st, headers, html) = b.get(&h, &path).await;
    assert!(
        st == StatusCode::OK
            && html.contains(r#"data-sitekey="site-key-1""#)
            && html.contains(crate::hcaptcha::SCRIPT),
        "{html}"
    );
    let csp = headers["content-security-policy"].to_str().unwrap();
    assert!(
        csp.contains("script-src 'self' https://hcaptcha.com")
            && csp.contains("frame-src https://hcaptcha.com"),
        "{csp}"
    );
    let (csrf, rid) = (hidden(&html, "csrf"), hidden(&html, "request_id"));
    let sign_up = |captcha: Option<&'static str>| {
        let (b, h, csrf, rid) = (&b, &h, csrf.clone(), rid.clone());
        async move {
            let mut form = vec![
                ("csrf", csrf.as_str()),
                ("request_id", rid.as_str()),
                ("handle", "pia"),
                ("email", "pia@example.com"),
                ("password", "pias password"),
            ];
            if let Some(c) = captcha {
                form.push(("h-captcha-response", c));
            }
            b.post(h, "/oauth/authorize/sign-up", &form).await
        }
    };
    let (st, _, html) = sign_up(None).await;
    assert!(
        st == StatusCode::BAD_REQUEST && html.contains("hCaptcha token is required"),
        "{st} {html}"
    );
    let (st, _, html) = sign_up(Some("wrong")).await;
    assert!(
        st == StatusCode::BAD_REQUEST && html.contains("hCaptcha verification failed"),
        "{st} {html}"
    );
    assert!(!exists(&h, "pia").await);
    let (st, _, html) = sign_up(Some("good")).await;
    assert_eq!(st, StatusCode::OK, "{html}");
    assert!(exists(&h, "pia").await);
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn without_a_site_key_and_secret_web_sign_up_has_no_captcha() {
    // A site key but no secret: off.
    let h = harness(false).await;
    let (url, seen) = siteverify().await;
    for (k, v) in [
        ("accounts.hcaptcha_site_key", "site-key-1"),
        ("accounts.hcaptcha_verify_url", url.as_str()),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    let (_, headers, page) = Browser::default().get(&h, "/account/sign-up").await;
    assert!(!page.contains("h-captcha"), "{page}");
    assert!(
        !headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("hcaptcha"),
    );
    let (st, _, page) = portal_sign_up(&h, "quin", None).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{page}");

    // A secret but no site key: off too.
    let (h, seen2) = hcaptcha_harness().await;
    h.state
        .settings
        .set("accounts.hcaptcha_site_key", json!(""), "test")
        .await
        .unwrap();
    let (st, _, page) = portal_sign_up(&h, "rae", None).await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{page}");
    assert!(seen.lock().unwrap().is_empty() && seen2.lock().unwrap().is_empty());
}

/// Marks setup complete, as a finished install.
async fn complete_setup(h: &Harness) {
    assert!(h.state.store.create_setup_token("t").await.unwrap());
    let mut tx = h.state.store.begin().await.unwrap();
    assert!(tx.complete_setup("pds.example.com", "test").await.unwrap());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn the_hcaptcha_secret_comes_from_env_once_and_changes_with_keys_set() {
    let env = |v: &str| -> HashMap<String, String> {
        [("PDS_HCAPTCHA_SECRET_KEY".to_owned(), v.to_owned())].into()
    };
    let (h, _) = hcaptcha_harness().await;
    let load = |e: HashMap<String, String>| {
        let h = &h;
        async move {
            ServerSecrets::load_or_create_from(&h.state.store, &h.state.master_key, &e)
                .await
                .unwrap()
                .hcaptcha_secret
                .map(|s| String::from_utf8(s.to_vec()).unwrap())
        }
    };
    assert_eq!(
        h.state
            .secrets
            .hcaptcha_secret
            .as_deref()
            .map(Vec::as_slice),
        Some(b"the-hcaptcha-secret".as_slice()),
        "imported on the first boot"
    );
    complete_setup(&h).await;
    // A changed env value is refused: the stored secret stays.
    assert_eq!(
        load(env("another-secret")).await.as_deref(),
        Some("the-hcaptcha-secret")
    );

    // keys set hcaptcha-secret: owner, re-auth, audited by fingerprint.
    let owner = h.owner_with_password("hana", "hana-password-123").await;
    let set = format!("/xrpc/{}.owner.keys.setSecret", locus_core::LOCUS_NS);
    let call = |body: Value| h.admin_call("POST", &set, &owner, Some(body));
    let (st, _) = call(json!({"name": "hcaptcha-secret", "value": "new-secret",
                              "password": "wrong-password"}))
    .await;
    assert!(st.is_client_error(), "re-auth needed");
    let (st, _) = call(json!({"name": "hcaptcha-secret", "value": "  ",
                              "password": "hana-password-123"}))
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "empty");
    let (st, v) = call(json!({"name": "hcaptcha-secret", "value": "new-secret",
                              "password": "hana-password-123"}))
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        load(env("the-hcaptcha-secret")).await.as_deref(),
        Some("new-secret"),
        "the env value doesn't win back"
    );
    let audit = h.state.store.audit_entries(None, 20).await.unwrap();
    let row = audit
        .iter()
        .find(|a| a.action == "keys.setSecret")
        .expect("audited");
    assert!(
        !row.detail_json
            .clone()
            .unwrap_or_default()
            .contains("new-secret")
    );

    // An install without one doesn't take it from env after setup.
    let h2 = harness(false).await;
    complete_setup(&h2).await;
    let got = ServerSecrets::load_or_create_from(&h2.state.store, &h2.state.master_key, &env("x"))
        .await
        .unwrap()
        .hcaptcha_secret;
    assert!(got.is_none());
}

// ---------------------------------------------------------------- service DID

const EXPLICIT: &str = "did:plc:ewvi7nxzyoun6zhxrhs64oiz";

fn claims(jwt: &str) -> Value {
    use base64::Engine;
    let p = jwt.split('.').nth(1).unwrap();
    serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(p)
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn an_explicit_service_did_is_the_service_identity() {
    let h = harness_build(HarnessOpts {
        service_did: Some(EXPLICIT.into()),
        ..HarnessOpts::default()
    })
    .await;
    assert_eq!(h.state.service_did, EXPLICIT);

    // describeServer names it; legacy tokens are minted for it.
    let (_, v) = h
        .call("GET", "/xrpc/com.atproto.server.describeServer", None, None)
        .await;
    assert_eq!(v["did"], EXPLICIT);
    let (did, access) = h.user("sid.pds.example.com").await;
    assert_eq!(claims(&access)["aud"], EXPLICIT);
    let (st, _) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // The host's did.json stays did:web:<host>.
    let (st, v) = h.call("GET", "/.well-known/did.json", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["id"], "did:web:pds.example.com");

    // Inbound service auth names the explicit DID; did:web:<host> is
    // refused there.
    let key = proto_blue_crypto::K256Keypair::generate();
    plc_did(&h, MOD_DID, &key);
    h.state
        .settings
        .set("moderation.service_did", json!(MOD_DID), "test")
        .await
        .unwrap();
    let now = now_s();
    let token = |aud: &str, lxm: &str| {
        locus_auth::service_jwt::create(&key, MOD_DID, aud, None, Some(lxm), now).unwrap()
    };
    let r = mod_call(&h, STATUS, &did, &token(EXPLICIT, STATUS)).await;
    assert_eq!(r.0, StatusCode::OK, "{}", r.1);
    let r = mod_call(&h, STATUS, &did, &token("did:web:pds.example.com", STATUS)).await;
    assert_eq!(
        (r.0, r.1["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("BadJwtAudience")),
        "{}",
        r.1
    );

    // The membership-assertion query also takes did:web:<host> (assertion
    // identities are always the host's): past the audience check, the
    // answer is the plain refusal; another audience is refused there.
    let lxm = crate::membership::ASSERTION_LXM;
    let path = format!(
        "/xrpc/{lxm}?audienceUri=at%3A%2F%2Fdid%3Aplc%3Aowner%2Fx%2Fy&aud=did:web:other.test"
    );
    for (aud, want) in [
        ("did:web:pds.example.com", "Forbidden"),
        (EXPLICIT, "Forbidden"),
        ("did:web:other.test", "BadJwtAudience"),
    ] {
        let (_, v) = h.call("GET", &path, Some(&token(aud, lxm)), None).await;
        assert_eq!(v["error"], want, "{aud}: {v}");
    }

    // No change path: the setting is refused through the admin API.
    let owner = format!("Bearer {}", h.operator("own", "owner").await);
    let (st, v) = h
        .admin_call(
            "POST",
            &format!("/xrpc/{}.admin.setSetting", locus_core::LOCUS_NS),
            &owner,
            Some(json!({"key": "identity.service_did", "value": "did:web:pds.example.com"})),
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && v["message"].as_str().unwrap().contains("0.1"),
        "{v}"
    );
}

#[tokio::test]
async fn pds_service_did_is_recorded_before_setup_and_refused_after() {
    let h = harness(false).await;
    let store = &h.state.store;
    let stored = || async {
        store
            .load_snapshot()
            .await
            .unwrap()
            .text("identity.service_did")
            .to_owned()
    };
    let apply = |v: Option<&'static str>, done: bool| async move {
        crate::identity::apply_service_did_env(store, v, done).await
    };
    // Before setup: recorded; a bad value fails the boot; unset keeps it.
    assert!(apply(Some("not a did"), false).await.is_err());
    assert!(!apply(Some(EXPLICIT), false).await.unwrap());
    assert_eq!(stored().await, EXPLICIT);
    assert!(!apply(None, false).await.unwrap());
    assert_eq!(stored().await, EXPLICIT);
    // After: the same value is fine; another, or none, is refused and
    // changes nothing.
    assert!(!apply(Some(EXPLICIT), true).await.unwrap());
    assert!(apply(Some("did:plc:another"), true).await.unwrap());
    assert!(apply(None, true).await.unwrap());
    assert_eq!(stored().await, EXPLICIT);
    // An install that never set one refuses a later one.
    let h2 = harness(false).await;
    assert!(
        crate::identity::apply_service_did_env(&h2.state.store, Some(EXPLICIT), true)
            .await
            .unwrap()
    );
    assert_eq!(
        h2.state
            .store
            .load_snapshot()
            .await
            .unwrap()
            .text("identity.service_did"),
        ""
    );
}

#[tokio::test]
async fn the_console_warns_about_a_refused_pds_service_did() {
    use locus_store::env::CLOCK_SLOT_FAMILY;
    let h = harness(false).await;
    h.console_operator("cass", "viewer", "cass-password-1234")
        .await;
    let b = Browser::default();
    h.console_sign_in(&b, "cass", "cass-password-1234").await;
    let warning = "PDS_SERVICE_DID on node-b differs from this install";
    let (_, lease) = h
        .state
        .store
        .claim_slot(
            CLOCK_SLOT_FAMILY,
            8,
            "node-b",
            std::time::Duration::from_secs(30),
        )
        .await
        .unwrap()
        .unwrap();
    h.state
        .store
        .report_bootstrap("node-b", &[("service_did_env", "ok")])
        .await
        .unwrap();
    assert!(!b.get(&h, "/admin").await.2.contains(warning));
    h.state
        .store
        .report_bootstrap("node-b", &[("service_did_env", "refused")])
        .await
        .unwrap();
    let page = b.get(&h, "/admin").await.2;
    assert!(page.contains(warning), "{page}");
    assert!(page.contains(EXPLICIT) || page.contains("did:web:pds.example.com"));
    h.state.store.release_lease(&lease).await.unwrap();
    assert!(!b.get(&h, "/admin").await.2.contains(warning));
}
