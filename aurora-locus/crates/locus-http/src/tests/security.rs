//! Security checks: client IPs behind proxies, blob references, operator
//! roles, shard callbacks, membership assertions, hostname reservations and
//! the private tier's fail-closed paths.

use super::*;

// ---------------------------------------------------------------- client IPs

async fn hit(h: &Harness, peer: &str, xff: &str) -> StatusCode {
    h.call_from(
        peer,
        "GET",
        "/xrpc/com.atproto.server.describeServer",
        None,
        None,
        &[("x-forwarded-for", xff)],
    )
    .await
    .0
}

#[tokio::test]
async fn forwarded_headers_count_only_from_trusted_proxies() {
    let h = harness(false).await;
    h.state
        .settings
        .set("rate_limits.global_points", json!(1), "test")
        .await
        .unwrap();

    // Under the defaults (loopback only), a private peer's X-Forwarded-For
    // is ignored: a second request naming another client still counts
    // against the peer.
    for (i, peer) in ["10.1.2.3", "172.16.4.4", "192.168.1.9", "172.30.53.2"]
        .into_iter()
        .enumerate()
    {
        let a = format!("198.51.100.{}", 10 + i * 2);
        let b = format!("198.51.100.{}", 11 + i * 2);
        assert_eq!(hit(&h, peer, &a).await, StatusCode::OK, "{peer}");
        assert_eq!(
            hit(&h, peer, &b).await,
            StatusCode::TOO_MANY_REQUESTS,
            "{peer}: keyed by the peer"
        );
        // The forwarded client's own bucket wasn't touched.
        assert_eq!(hit(&h, "127.0.0.1", &a).await, StatusCode::OK, "{peer}");
    }

    // Caddy's static address, configured: its forwarded client counts, one
    // bucket per client; the address next to it still doesn't.
    h.state
        .settings
        .set(
            "http.trusted_proxies",
            json!(["127.0.0.0/8", "::1/128", "172.30.53.2/32"]),
            "test",
        )
        .await
        .unwrap();
    let caddy = "172.30.53.2";
    assert_eq!(hit(&h, caddy, "203.0.113.1").await, StatusCode::OK);
    assert_eq!(hit(&h, caddy, "203.0.113.2").await, StatusCode::OK);
    assert_eq!(
        hit(&h, caddy, "203.0.113.1").await,
        StatusCode::TOO_MANY_REQUESTS,
        "keyed by the forwarded client"
    );
    assert_eq!(hit(&h, "172.30.53.3", "203.0.113.3").await, StatusCode::OK);
    assert_eq!(
        hit(&h, "172.30.53.3", "203.0.113.4").await,
        StatusCode::TOO_MANY_REQUESTS,
        "a neighbour isn't trusted"
    );
}

// ---------------------------------------------------------------- blob references

fn post_with_image(blob: &Value) -> Value {
    json!({"$type": "app.bsky.feed.post", "text": "a picture", "createdAt": "2026-11-05T00:00:00.000Z",
        "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "", "image": blob}]}})
}

impl Harness {
    async fn create_post(&self, did: &str, access: &str, record: Value) -> (StatusCode, Value) {
        self.post(
            "com.atproto.repo.createRecord",
            Some(access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": record}),
        )
        .await
    }

    async fn post_count(&self, did: &str) -> usize {
        let (_, v) = self
            .get(format!(
                "/xrpc/com.atproto.repo.listRecords?repo={did}&collection=app.bsky.feed.post"
            ))
            .await;
        v["records"].as_array().unwrap().len()
    }
}

#[tokio::test]
async fn record_writes_naming_taken_down_quarantined_or_held_blobs_are_refused() {
    let h = harness(false).await;
    let (did, access) = h.user("bob.pds.example.com").await;
    let moderator = format!("Bearer {}", h.operator("mo", "moderator").await);
    let update = "/xrpc/com.atproto.admin.updateSubjectStatus";
    let takedown = |cid: &str, applied: bool| {
        json!({"subject": {"$type": "com.atproto.admin.defs#repoBlobRef", "did": did, "cid": cid},
            "takedown": {"applied": applied, "ref": "case-9"}})
    };
    let refused = |(st, v): (StatusCode, Value)| {
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("BlobNotFound")),
            "{v}"
        );
    };

    // A fresh (temp) blob, taken down: no record may take it.
    let fresh = h.upload(&access, b"fresh bytes", "image/png").await;
    let fresh_cid = fresh["ref"]["$link"].as_str().unwrap().to_owned();
    let (st, _) = h
        .admin_call("POST", update, &moderator, Some(takedown(&fresh_cid, true)))
        .await;
    assert_eq!(st, StatusCode::OK);
    refused(h.create_post(&did, &access, post_with_image(&fresh)).await);
    // Nor through applyWrites.
    let (st, v) = h
        .post(
            "com.atproto.repo.applyWrites",
            Some(&access),
            json!({"repo": did, "writes": [{"$type": "com.atproto.repo.applyWrites#create",
                "collection": "app.bsky.feed.post", "value": post_with_image(&fresh)}]}),
        )
        .await;
    refused((st, v));
    // Nor by a write the same batch then supersedes.
    let (st, v) = h
        .post(
            "com.atproto.repo.applyWrites",
            Some(&access),
            json!({"repo": did, "writes": [
                {"$type": "com.atproto.repo.applyWrites#create", "collection": "app.bsky.feed.post",
                 "rkey": "3m6supersede2", "value": post_with_image(&fresh)},
                {"$type": "com.atproto.repo.applyWrites#update", "collection": "app.bsky.feed.post",
                 "rkey": "3m6supersede2", "value": post_record("plain after all")},
            ]}),
        )
        .await;
    refused((st, v));
    assert_eq!(h.post_count(&did).await, 0, "nothing written");
    let row = h.state.store.blob(&did, &fresh_cid).await.unwrap().unwrap();
    assert_eq!(row.state, "temp", "not made permanent");
    // Reversed: the same record now goes through.
    h.admin_call("POST", update, &moderator, Some(takedown(&fresh_cid, false)))
        .await;
    let (st, v) = h.create_post(&did, &access, post_with_image(&fresh)).await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // A blob already referenced (permanent), then taken down: a second
    // record can't reference it, and an update can't add it.
    let (st, first) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("plain")}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = h
        .admin_call("POST", update, &moderator, Some(takedown(&fresh_cid, true)))
        .await;
    assert_eq!(st, StatusCode::OK);
    let before = h.post_count(&did).await;
    refused(h.create_post(&did, &access, post_with_image(&fresh)).await);
    let rkey = first["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    refused(
        h.post(
            "com.atproto.repo.putRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": rkey,
                "record": post_with_image(&fresh)}),
        )
        .await,
    );
    assert_eq!(h.post_count(&did).await, before);
    let (_, plain) = h
        .get(format!(
            "/xrpc/com.atproto.repo.getRecord?repo={did}&collection=app.bsky.feed.post&rkey={rkey}"
        ))
        .await;
    assert!(plain["value"].get("embed").is_none(), "the update didn't land");

    // A scanner match: quarantined and held for review.
    let (url, _) = mock_scanner().await;
    for (k, v) in [("blobs.scanner", "webhook"), ("blobs.scanner_url", url.as_str())] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    let bad = b"a BAD picture";
    let bad_cid = proto_blue_lex_data::Cid::for_raw(bad).to_string_base32();
    let (st, _, _) = h
        .raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(&access),
            bad.to_vec(),
            &[("content-type", "image/png")],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let row = h.state.store.blob(&did, &bad_cid).await.unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.review_hold), ("quarantined", true));
    let bad_ref = json!({"$type": "blob", "ref": {"$link": bad_cid}, "mimeType": "image/png", "size": bad.len()});
    let before = h.post_count(&did).await;
    refused(h.create_post(&did, &access, post_with_image(&bad_ref)).await);
    assert_eq!(h.post_count(&did).await, before);
    let row = h.state.store.blob(&did, &bad_cid).await.unwrap().unwrap();
    assert_eq!(row.state, "quarantined", "still quarantined");
}

// ---------------------------------------------------------------- operator roles

/// Every grouped locus method the router serves, from the route tables.
fn grouped_locus_methods() -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    for src in [
        include_str!("../locus_admin.rs"),
        include_str!("../entryway.rs"),
        include_str!("../repair.rs"),
        include_str!("../lib.rs"),
    ] {
        for marker in ["at!(\"", "locus_nsid!(\""] {
            for piece in src.split(marker).skip(1) {
                let name = piece.split('"').next().unwrap();
                let lxm = format!("{}.{name}", locus_core::LOCUS_NS);
                if crate::admin_auth::group_min(&lxm).is_some() {
                    out.insert(lxm);
                }
            }
        }
    }
    out.into_iter().collect()
}

#[tokio::test]
async fn no_grouped_locus_method_takes_a_lower_role() {
    use crate::admin_auth::Role;
    let h = harness(false).await;
    let (_, user_access) = h.user("una.pds.example.com").await;
    let mut tokens = Vec::new();
    for role in [Role::Viewer, Role::Moderator, Role::Admin, Role::Owner] {
        let t = h.operator(&format!("op-{}", role.as_str()), role.as_str()).await;
        tokens.push((role, format!("Bearer {t}")));
    }
    let methods = grouped_locus_methods();
    assert!(methods.len() > 40, "{methods:?}");
    for lxm in &methods {
        let min = crate::admin_auth::group_min(lxm).unwrap();
        let path = format!("/xrpc/{lxm}");
        for (role, auth) in &tokens {
            // PUT matches no route's method: the gate answers first, or
            // the method router's 405 shows the gate let it through.
            let (st, _) = h.admin_call("PUT", &path, auth, None).await;
            let want = if *role < min {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::METHOD_NOT_ALLOWED
            };
            assert_eq!(st, want, "{lxm} as {}", role.as_str());
            // A body that doesn't parse is still refused on the role, not
            // the body.
            if *role < min {
                let (st, _, _) = h
                    .raw(
                        "POST",
                        &path,
                        None,
                        b"{not json".to_vec(),
                        &[("authorization", auth), ("content-type", "application/json")],
                    )
                    .await;
                assert_eq!(st, StatusCode::FORBIDDEN, "{lxm} as {}", role.as_str());
            }
        }
        // No operator credentials at all.
        for auth in [
            None,
            Some(format!("Bearer {user_access}")),
            Some(h.basic_admin()),
        ] {
            let (st, _, _) = h
                .raw(
                    "POST",
                    &path,
                    None,
                    b"{}".to_vec(),
                    &auth
                        .as_deref()
                        .map(|a| vec![("authorization", a)])
                        .unwrap_or_default(),
                )
                .await;
            assert_eq!(st, StatusCode::UNAUTHORIZED, "{lxm} with {auth:?}");
        }
    }

    // A viewer can't rotate the kryphocron generation; nothing changes.
    let generation = h.state.store.current_generation().await.unwrap();
    let rotate = format!("/xrpc/{}.admin.kryphocron.rotateNow", locus_core::LOCUS_NS);
    let (st, _) = h
        .admin_call("POST", &rotate, &tokens[0].1, Some(json!({})))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(h.state.store.current_generation().await.unwrap(), generation);
}

/// What a refused admin call could have changed: the account row (handle,
/// email, password, status, invites), the invite codes, the mail sent.
async fn admin_state(
    h: &Harness,
    did: &str,
    moderator: &str,
) -> (Option<locus_store::accounts::Account>, Value, usize) {
    let (_, codes) = h
        .admin_call("GET", "/xrpc/com.atproto.admin.getInviteCodes", moderator, None)
        .await;
    (
        h.state.store.account_by_did(did).await.unwrap(),
        codes["codes"].clone(),
        h.mail_count(),
    )
}

#[tokio::test]
async fn reference_admin_methods_refuse_lower_roles_and_change_nothing() {
    let h = harness(false).await;
    let (did, _) = h.user("vic.pds.example.com").await;
    let viewer = format!("Bearer {}", h.operator("vee", "viewer").await);
    let moderator = format!("Bearer {}", h.operator("mo", "moderator").await);
    let admin = format!("Bearer {}", h.operator("ad", "admin").await);
    let x = |m: &str| format!("/xrpc/com.atproto.{m}");
    let (_, code) = h
        .admin_call(
            "POST",
            &x("server.createInviteCode"),
            &moderator,
            Some(json!({"useCount": 1})),
        )
        .await;
    let code = code["code"].as_str().unwrap().to_owned();

    let before = admin_state(&h, &did, &moderator).await;
    assert!(before.0.is_some());

    let takedown = json!({"subject": {"$type": "com.atproto.admin.defs#repoRef", "did": did},
        "takedown": {"applied": true, "ref": "x"}});
    let moderator_methods = [
        ("admin.updateSubjectStatus", takedown),
        (
            "admin.sendEmail",
            json!({"recipientDid": did, "content": "hi", "senderDid": "did:web:pds.example.com"}),
        ),
        ("admin.disableAccountInvites", json!({"account": did})),
        ("admin.enableAccountInvites", json!({"account": did})),
        ("admin.disableInviteCodes", json!({"codes": [code]})),
        ("server.createInviteCode", json!({"useCount": 3})),
        (
            "server.createInviteCodes",
            json!({"codeCount": 2, "useCount": 1}),
        ),
    ];
    let admin_methods = [
        ("admin.deleteAccount", json!({"did": did})),
        (
            "admin.updateAccountPassword",
            json!({"did": did, "password": "a brand new password"}),
        ),
        (
            "admin.updateAccountEmail",
            json!({"account": did, "email": "new@example.com"}),
        ),
        (
            "admin.updateAccountHandle",
            json!({"did": did, "handle": "victor.pds.example.com"}),
        ),
    ];
    for (m, body) in moderator_methods.iter().chain(&admin_methods) {
        let (st, v) = h
            .admin_call("POST", &x(m), &viewer, Some(body.clone()))
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{m} as viewer: {v}");
    }
    let (st, _) = h
        .admin_call("GET", &x("admin.getInviteCodes"), &viewer, None)
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "getInviteCodes as viewer");
    for (m, body) in &admin_methods {
        let (st, v) = h
            .admin_call("POST", &x(m), &moderator, Some(body.clone()))
            .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{m} as moderator: {v}");
    }
    assert_eq!(
        admin_state(&h, &did, &moderator).await,
        before,
        "refused calls change nothing"
    );

    // The same bodies work with the role (so the refusals were the role's).
    for (m, body) in &admin_methods[1..] {
        let (st, v) = h.admin_call("POST", &x(m), &admin, Some(body.clone())).await;
        assert_eq!(st, StatusCode::OK, "{m} as admin: {v}");
    }
    for (m, body) in &moderator_methods[2..] {
        let (st, v) = h
            .admin_call("POST", &x(m), &moderator, Some(body.clone()))
            .await;
        assert_eq!(st, StatusCode::OK, "{m} as moderator: {v}");
    }
    assert_ne!(admin_state(&h, &did, &moderator).await, before);
}

// ---------------------------------------------------------------- shard callbacks

#[tokio::test]
async fn set_callback_token_takes_only_the_entryways_jwt_and_replaces_the_token() {
    use proto_blue_crypto::K256Keypair;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let token_key = K256Keypair::generate();
    let plc_key = K256Keypair::generate();
    let (url, _) = mock_entryway().await;
    let ew_did = "did:web:entryway.test";
    let h = shard_harness(crate::shard::Entryway::for_test(
        &url,
        ew_did,
        &[token_key.did()],
        &[plc_key.did()],
    ))
    .await;
    let did = shard_account(&h, &plc_key, "cy.test").await;
    let now = locus_core::time::now_ms() / 1000;
    let set_token = ns("shard.setCallbackToken");
    let lxm = format!("{}.shard.setCallbackToken", locus_core::LOCUS_NS);
    let good = || {
        locus_auth::service_jwt::create(
            &token_key,
            ew_did,
            "did:web:pds.example.com",
            Some(now + 60),
            Some(&lxm),
            now,
        )
        .unwrap()
    };
    let (cb1, cb2) = (
        "the-first-callback-token-of-enough-length-01",
        "the-second-callback-token-of-enough-length-2",
    );
    let (st, v) = h
        .call("POST", &set_token, Some(&good()), Some(json!({"token": cb1})))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // Whether a callback with `token` is taken.
    let callback = |token: &'static str| {
        let (h, did) = (&h, did.clone());
        async move {
            h.call(
                "POST",
                &ns("shard.revokeSessions"),
                Some(token),
                Some(json!({"did": did, "sid": "s-x"})),
            )
            .await
            .0
        }
    };
    assert_eq!(callback(cb1).await, StatusCode::OK);
    assert_eq!(callback(cb2).await, StatusCode::UNAUTHORIZED);

    // Other credentials never install a token.
    let owner = h.operator("own", "owner").await;
    let access = es256k_jwt(
        &token_key,
        "at+jwt",
        &json!({"scope": "com.atproto.access", "aud": "did:web:pds.example.com",
                "sub": did, "sid": "s1", "iat": now, "exp": now + 600}),
    );
    let user_key = K256Keypair::generate();
    let user_jwt = locus_auth::service_jwt::create(
        &user_key,
        &did,
        "did:web:pds.example.com",
        Some(now + 60),
        Some(&lxm),
        now,
    )
    .unwrap();
    for (bad, why) in [
        (cb1.to_owned(), "the callback token itself"),
        (owner, "an operator token"),
        (access, "an entryway access token"),
        (user_jwt, "an account's service JWT"),
        ("entryway-admin".to_owned(), "the entryway admin token"),
    ] {
        let (st, v) = h
            .call("POST", &set_token, Some(&bad), Some(json!({"token": cb2})))
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{why}: {v}");
        assert_eq!(callback(cb1).await, StatusCode::OK, "{why}");
        assert_eq!(callback(cb2).await, StatusCode::UNAUTHORIZED, "{why}");
    }

    // A second call replaces it: the old token is refused from then on.
    let (st, v) = h
        .call("POST", &set_token, Some(&good()), Some(json!({"token": cb2})))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(callback(cb2).await, StatusCode::OK);
    assert_eq!(callback(cb1).await, StatusCode::UNAUTHORIZED);
    let keys_before = h.state.store.shard_verify_keys().await.unwrap();
    let (st, _) = h
        .call(
            "POST",
            &ns("shard.setVerifyKeys"),
            Some(cb1),
            Some(json!({"keys": [K256Keypair::generate().did()]})),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "the old token sets nothing");
    assert_eq!(
        h.state.store.shard_verify_keys().await.unwrap(),
        keys_before,
        "verify keys unchanged"
    );
}

// ---------------------------------------------------------------- assertions

/// An assertion with `header` and `claims`, signed with this host's
/// assertion key.
fn signed_assertion(h: &Harness, header: &Value, claims: &Value) -> String {
    use base64::Engine;
    use proto_blue_crypto::Signer;
    let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
    let input = format!(
        "{}.{}",
        b64(header.to_string().as_bytes()),
        b64(claims.to_string().as_bytes())
    );
    let sig = h.state.secrets.audience_key.sign(input.as_bytes()).unwrap();
    format!("{input}.{}", b64(&sig))
}

#[tokio::test]
async fn membership_assertions_with_another_issuer_or_type_are_rejected() {
    let h = harness(false).await;
    let (owner, _) = h.user("ova.pds.example.com").await;
    let (member, _) = h.user("mae.pds.example.com").await;
    let audience = format!("at://{owner}/tools.kryphocron.policy.audience/3m6aaaaaaaaaa");
    let (_, doc) = h.call("GET", "/.well-known/did.json", None, None).await;
    let key = doc["verificationMethod"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str().unwrap().ends_with("#kryphocron_audience"))
        .unwrap()["publicKeyMultibase"]
        .as_str()
        .unwrap()
        .to_owned();
    for host in ["pds.example.com", "evil.example.com"] {
        h.state
            .kr
            .membership
            .seed_issuer_key(host, &format!("did:key:{key}"));
    }
    let now = locus_core::time::now_ms() / 1000;
    let alg = "ES256K";
    let header = |typ: &str| json!({"typ": typ, "alg": alg});
    let claims = |iss: &str| {
        json!({"iss": iss, "sub": member, "audienceUri": audience, "aud": "did:web:pds.example.com",
            "iat": now, "exp": now + 120, "jti": "j1"})
    };

    // The control: the right issuer and type verify.
    let ok = signed_assertion(
        &h,
        &header(crate::membership::ASSERTION_TYP),
        &claims("did:web:pds.example.com"),
    );
    assert!(crate::membership::verify_assertion(&h.state, &ok, &member, &audience).await);
    // Another issuer (even one whose key is known and signed it), or no
    // issuer.
    for iss in ["did:web:evil.example.com", "did:plc:ewvi7nxzyoun6zhxrhs64oiz", ""] {
        let t = signed_assertion(&h, &header(crate::membership::ASSERTION_TYP), &claims(iss));
        assert!(
            !crate::membership::verify_assertion(&h.state, &t, &member, &audience).await, "iss {iss:?}");
    }
    // Another type: a service JWT's, an access token's, none.
    for typ in ["JWT", "at+jwt", "dpop+jwt", ""] {
        let t = signed_assertion(&h, &header(typ), &claims("did:web:pds.example.com"));
        assert!(
            !crate::membership::verify_assertion(&h.state, &t, &member, &audience).await, "typ {typ:?}");
    }
    let mut no_typ = header(crate::membership::ASSERTION_TYP);
    no_typ.as_object_mut().unwrap().remove("typ");
    let t = signed_assertion(&h, &no_typ, &claims("did:web:pds.example.com"));
    assert!(
            !crate::membership::verify_assertion(&h.state, &t, &member, &audience).await, "no typ");
}

// ---------------------------------------------------------------- hostnames

#[tokio::test]
async fn pds_hostnames_old_and_prepared_are_never_handles_or_did_web_hosts() {
    let h = harness_at("zed7.example.com", false, true).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    h.state
        .settings
        .set("identity.handle_domains", json!([".example.com"]), "test")
        .await
        .unwrap();
    let (did, access) = h.user("ann.example.com").await;
    let taken = |(st, v): (StatusCode, Value), why: &str| {
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("HandleNotAvailable")),
            "{why}: {v}"
        );
    };
    let create = |handle: &str, method: &str| {
        let body = json!({"handle": handle, "email": format!("{method}-{handle}@x.com"),
            "password": "pw", "locusDidMethod": method});
        h.call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(body),
        )
    };

    // The PDS hostname, under a handle domain, is no handle and no did:web
    // host; a free name next to it is.
    taken(
        h.update_handle(&access, "zed7.example.com").await,
        "hostname as a handle",
    );
    taken(create("zed7.example.com", "plc").await, "hostname at signup");
    taken(create("zed7.example.com", "web").await, "hostname as did:web");
    let (st, v) = h.update_handle(&access, "free7.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // A prepared candidate: reserved as both.
    let owner = h.owner_with_password("oto", "oto-password-1234").await;
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.prepare"),
            &owner,
            Some(json!({"host": "next7.example.com"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    taken(
        h.update_handle(&access, "next7.example.com").await,
        "candidate as a handle",
    );
    taken(create("next7.example.com", "web").await, "candidate as did:web");
    assert!(
        h.state
            .store
            .hosted_did_web_by_host("next7.example.com")
            .await
            .unwrap()
            .is_none()
    );
    // Cancelled: released, and a handle can then really take it.
    let (st, _) = h
        .admin_call("POST", &ns("owner.hostname.cancel"), &owner, Some(json!({})))
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = h.update_handle(&access, "next7.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, r) = h
        .get("/xrpc/com.atproto.identity.resolveHandle?handle=next7.example.com".into())
        .await;
    assert_eq!(r["did"], json!(did));

    // After a migration from an old host, the old host stays reserved.
    h.state
        .store
        .prepare_hostname_migration("m1", "old7.example.com", &h.state.hostname, "test")
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .confirm_hostname_migration("m1", 0)
            .await
            .unwrap()
    );
    taken(
        h.update_handle(&access, "old7.example.com").await,
        "old host as a handle",
    );
    taken(create("old7.example.com", "plc").await, "old host at signup");
    taken(create("old7.example.com", "web").await, "old host as did:web");
}
