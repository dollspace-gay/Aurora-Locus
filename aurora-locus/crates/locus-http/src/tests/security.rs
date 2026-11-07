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
    async fn try_post(&self, did: &str, access: &str, record: Value) -> (StatusCode, Value) {
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
    refused(h.try_post(&did, &access, post_with_image(&fresh)).await);
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
    h.admin_call(
        "POST",
        update,
        &moderator,
        Some(takedown(&fresh_cid, false)),
    )
    .await;
    let (st, v) = h.try_post(&did, &access, post_with_image(&fresh)).await;
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
    refused(h.try_post(&did, &access, post_with_image(&fresh)).await);
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
    assert!(
        plain["value"].get("embed").is_none(),
        "the update didn't land"
    );

    // A scanner match: quarantined and held for review.
    let (url, _) = mock_scanner().await;
    for (k, v) in [
        ("blobs.scanner", "webhook"),
        ("blobs.scanner_url", url.as_str()),
    ] {
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
    refused(h.try_post(&did, &access, post_with_image(&bad_ref)).await);
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
        let t = h
            .operator(&format!("op-{}", role.as_str()), role.as_str())
            .await;
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
                        &[
                            ("authorization", auth),
                            ("content-type", "application/json"),
                        ],
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
    assert_eq!(
        h.state.store.current_generation().await.unwrap(),
        generation
    );
}

/// What a refused admin call could have changed: the account row (handle,
/// email, password, status, invites), the invite codes, the mail sent.
async fn admin_state(
    h: &Harness,
    did: &str,
    moderator: &str,
) -> (Option<locus_store::accounts::Account>, Value, usize) {
    let (_, codes) = h
        .admin_call(
            "GET",
            "/xrpc/com.atproto.admin.getInviteCodes",
            moderator,
            None,
        )
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
        let (st, v) = h
            .admin_call("POST", &x(m), &admin, Some(body.clone()))
            .await;
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
        .call(
            "POST",
            &set_token,
            Some(&good()),
            Some(json!({"token": cb1})),
        )
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
        .call(
            "POST",
            &set_token,
            Some(&good()),
            Some(json!({"token": cb2})),
        )
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
    for iss in [
        "did:web:evil.example.com",
        "did:plc:ewvi7nxzyoun6zhxrhs64oiz",
        "",
    ] {
        let t = signed_assertion(&h, &header(crate::membership::ASSERTION_TYP), &claims(iss));
        assert!(
            !crate::membership::verify_assertion(&h.state, &t, &member, &audience).await,
            "iss {iss:?}"
        );
    }
    // Another type: a service JWT's, an access token's, none.
    for typ in ["JWT", "at+jwt", "dpop+jwt", ""] {
        let t = signed_assertion(&h, &header(typ), &claims("did:web:pds.example.com"));
        assert!(
            !crate::membership::verify_assertion(&h.state, &t, &member, &audience).await,
            "typ {typ:?}"
        );
    }
    let mut no_typ = header(crate::membership::ASSERTION_TYP);
    no_typ.as_object_mut().unwrap().remove("typ");
    let t = signed_assertion(&h, &no_typ, &claims("did:web:pds.example.com"));
    assert!(
        !crate::membership::verify_assertion(&h.state, &t, &member, &audience).await,
        "no typ"
    );
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
    taken(
        create("zed7.example.com", "plc").await,
        "hostname at signup",
    );
    taken(
        create("zed7.example.com", "web").await,
        "hostname as did:web",
    );
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
    taken(
        create("next7.example.com", "web").await,
        "candidate as did:web",
    );
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
        .admin_call(
            "POST",
            &ns("owner.hostname.cancel"),
            &owner,
            Some(json!({})),
        )
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
    taken(
        create("old7.example.com", "plc").await,
        "old host at signup",
    );
    taken(
        create("old7.example.com", "web").await,
        "old host as did:web",
    );
}

// ---------------------------------------------------------------- private tier

/// Another PDS holding one private post: `getRecord` serves it (and nothing
/// else), `listRecords` lists no kryphocron blocks, and assertions for the
/// audience are issued (as `did:web:127.0.0.1`) to whoever asks, signed
/// with `key`.
/// The record key whose threadgate the mock remote PDS fails to serve.
const BROKEN_GATE_RKEY: &str = "3m6brokengate";

async fn mock_remote_pds(
    parent: Value,
    audience: String,
    key: Arc<proto_blue_crypto::K256Keypair>,
) -> String {
    use base64::Engine;
    let app = Router::new().route(
        "/xrpc/{nsid}",
        get(
            move |Path(nsid): Path<String>,
                  axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
                  headers: axum::http::HeaderMap| {
                let (parent, audience, key) = (parent.clone(), audience.clone(), key.clone());
                async move {
                    match nsid.as_str() {
                        "com.atproto.repo.getRecord"
                            if q.get("collection").map(String::as_str) == Some(POST_PRIVATE) =>
                        {
                            axum::Json(json!({"uri": "x", "cid": "x", "value": parent}))
                                .into_response()
                        }
                        // A thread root whose threadgate can't be fetched.
                        "com.atproto.repo.getRecord"
                            if q.get("rkey").map(String::as_str) == Some(BROKEN_GATE_RKEY) =>
                        {
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                axum::Json(json!({"error": "InternalServerError"})),
                            )
                                .into_response()
                        }
                        "com.atproto.repo.listRecords" => {
                            axum::Json(json!({"records": []})).into_response()
                        }
                        n if n == crate::membership::ASSERTION_LXM => {
                            // The caller is the service token's issuer.
                            let bearer = headers["authorization"].to_str().unwrap()[7..].to_owned();
                            let payload: Value = serde_json::from_slice(
                                &base64::engine::general_purpose::URL_SAFE_NO_PAD
                                    .decode(bearer.split('.').nth(1).unwrap())
                                    .unwrap(),
                            )
                            .unwrap();
                            let now = locus_core::time::now_ms() / 1000;
                            let token = es256k_jwt(
                                &key,
                                crate::membership::ASSERTION_TYP,
                                &json!({"iss": "did:web:127.0.0.1", "sub": payload["iss"],
                                    "audienceUri": audience, "aud": q["aud"],
                                    "iat": now, "exp": now + 120, "jti": "a1"}),
                            );
                            axum::Json(json!({"token": token})).into_response()
                        }
                        _ => (
                            StatusCode::BAD_REQUEST,
                            axum::Json(json!({"error": "RecordNotFound"})),
                        )
                            .into_response(),
                    }
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn block_checks_against_a_remote_parent_fail_closed() {
    let h = harness_build(HarnessOpts {
        ssrf_protection: false,
        ..HarnessOpts::default()
    })
    .await;
    let (script, _) = h.with_prism().await;
    // One AppView for both: prism is also the public AppView, so it's the
    // block source.
    let prism_url = h.state.snapshot().text("appview.private_url").to_owned();
    for (k, v) in [
        ("appview.url", prism_url.as_str()),
        ("appview.did", "did:web:prism.test"),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    // The parent's author is on another PDS.
    let author = format!("did:plc:{}", "r".repeat(24));
    let audience = format!("at://{author}/{AUD}/3m6aaaaaaaaaa");
    let parent_uri = format!("at://{author}/{POST_PRIVATE}/3m6parentpare");
    let parent = json!({"$type": POST_PRIVATE, "createdAt": "2024-01-01T00:00:00.000Z",
        "audienceList": audience, "encodedContent": {"$bytes": "AAECAwQFBgc"},
        "encodedContentCodec": "laquna/0.2"});
    let key = Arc::new(proto_blue_crypto::K256Keypair::generate());
    let remote = mock_remote_pds(parent, audience.clone(), key.clone()).await;
    h.plc.lock().unwrap().log.insert(
        author.clone(),
        vec![json!({"type": "plc_operation", "alsoKnownAs": ["at://far.example.net"],
            "verificationMethods": {"atproto": key.did()},
            "services": {"atproto_pds": {"type": "AtprotoPersonalDataServer", "endpoint": remote}}})],
    );
    h.state
        .kr
        .membership
        .seed_issuer_key("127.0.0.1", &key.did());
    let parent_ref = json!({"uri": parent_uri, "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"});
    // Two writers, each a known member of the private tier.
    let mut writers = Vec::new();
    for (handle, rkey) in [
        ("wyn.pds.example.com", "3m6wwwwwwwwww"),
        ("xia.pds.example.com", "3m6xxxxxxxxxx"),
    ] {
        let (did, access) = h.user(handle).await;
        let mine = h.add_audience(&did, rkey).await;
        let (st, v) = h
            .private_post_as(&did, &access, private_post("x", &mine))
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        writers.push((did, access));
    }
    let relationships = |status: u16, body: Value| {
        set_script(&script, "app.bsky.graph.getRelationships", status, body);
    };
    let reply = |w: usize| h.reply_as(&writers[w].0, &writers[w].1, &parent_ref, &audience);
    let forbidden = |(st, v): (StatusCode, Value), why: &str| {
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::FORBIDDEN, Some("Forbidden")),
            "{why}: {v}"
        );
    };

    // The block-source AppView answers 501 or another error: the reply is
    // refused, as if blocked.
    relationships(501, json!({"error": "MethodNotImplemented"}));
    forbidden(reply(0).await, "501");
    relationships(500, json!({"error": "InternalServerError"}));
    forbidden(reply(0).await, "500");
    relationships(400, json!({"error": "InvalidRequest"}));
    forbidden(reply(0).await, "400");
    relationships(429, json!({"error": "RateLimitExceeded"}));
    forbidden(reply(0).await, "429");
    // A real block is enforced.
    relationships(
        200,
        json!({"relationships": [{"did": author, "blockedBy": "at://x/app.bsky.graph.block/1"}]}),
    );
    forbidden(reply(1).await, "blocked");
    // No block: the same reply goes through (so the refusals were the
    // block check's).
    relationships(200, json!({"relationships": [{"did": author}]}));
    let (st, v) = reply(0).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // A thread root whose threadgate can't be fetched: refused (no
    // threadgate at all, as above, allows it).
    let broken = json!({"uri": format!("at://{author}/{POST_PRIVATE}/{BROKEN_GATE_RKEY}"),
        "cid": parent_ref["cid"]});
    forbidden(
        h.reply_as(&writers[0].0, &writers[0].1, &broken, &audience)
            .await,
        "threadgate unreachable",
    );
}

/// The private tier needs the private AppView entered, even when it's the
/// same service as the AppView: the AppView alone never stands in.
#[tokio::test]
async fn the_private_tier_needs_its_own_private_appview_entry() {
    let h = harness(false).await;
    let (_, seen) = h.with_prism().await;
    let prism_url = h.state.snapshot().text("appview.private_url").to_owned();
    let set = |k: &'static str, v: Value| {
        let h = &h;
        async move { h.state.settings.set(k, v, "test").await.unwrap() }
    };
    set("appview.url", json!(prism_url)).await;
    set("appview.did", json!("did:web:prism.test")).await;
    set("appview.private_url", json!("")).await;
    set("appview.private_did", json!("")).await;
    let (did, access) = h.user("pat.pds.example.com").await;
    let mine = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    let (st, v) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("UnsupportedNamespace")),
        "{v}"
    );
    assert!(
        !seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.uri.contains("getMembership")),
        "the AppView wasn't asked as prism"
    );
    // Entered as the private AppView too (the same service): it works.
    set("appview.private_url", json!(prism_url)).await;
    set("appview.private_did", json!("did:web:prism.test")).await;
    let (st, v) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn private_posts_name_only_audiences_the_writers_store_holds() {
    let h = harness(false).await;
    let (script, _) = h.with_prism().await;
    let (did, access) = h.user("ivy.pds.example.com").await;
    let mine = h.add_audience(&did, "3m6aaaaaaaaaa").await;
    let (st, post) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK, "{post}");
    let rkey = post["uri"].as_str().unwrap().rsplit('/').next().unwrap();
    let invalid = |(st, v): (StatusCode, Value), why: &str| {
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
            "{why}: {v}"
        );
    };

    // updatePostPrivate to an audience the store lacks (a complete listing
    // lacks it too), or to someone else's: refused, the post unchanged.
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": []}),
    );
    let (other, _) = h.user("jon.pds.example.com").await;
    let theirs = h.add_audience(&other, "3m6tttttttttt").await;
    for (target, why) in [
        (
            format!("at://{did}/{AUD}/3m6nnnnnnnnnn"),
            "not in the store",
        ),
        (theirs, "another account's"),
    ] {
        invalid(
            h.post(
                "tools.kryphocron.feed.updatePostPrivate",
                Some(&access),
                json!({"repo": did, "rkey": rkey, "audienceList": target}),
            )
            .await,
            why,
        );
    }
    assert_eq!(
        h.record_value(&did, POST_PRIVATE, rkey).await["audienceList"],
        json!(mine)
    );

    // A URI the refresh finds in prism but that is deleted here (a pending
    // delete): InvalidRequest, not adopted.
    let (did2, access2) = h.user("kai.pds.example.com").await;
    h.add_audience(&did2, "3m6cccccccccc").await;
    let mut row = h
        .state
        .store
        .kr_audience(&did2, "3m6cccccccccc")
        .await
        .unwrap()
        .unwrap();
    row.pending = "delete".into();
    h.state.store.put_kr_audience(&row).await.unwrap();
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [prism_audience(&did2, "3m6cccccccccc", "v9", &[])]}),
    );
    let pending = format!("at://{did2}/{AUD}/3m6cccccccccc");
    invalid(
        h.private_post_as(&did2, &access2, private_post("x", &pending))
            .await,
        "pending delete",
    );

    // Or a tombstone: a complete listing deleted it, a later page has it.
    let (did3, access3) = h.user("lev.pds.example.com").await;
    h.add_audience(&did3, "3m6dddddddddd").await;
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": []}),
    );
    let far = std::time::Instant::now() + std::time::Duration::from_secs(30);
    crate::audience::refresh(
        &h.state,
        &did3,
        crate::audience::Trigger::Background,
        None,
        far,
    )
    .await
    .unwrap();
    assert!(
        h.state
            .store
            .audience_tombstone(&did3, "3m6dddddddddd")
            .await
            .unwrap()
            .is_some()
    );
    set_script(
        &script,
        "app.nearhorizon.private.listAudiences",
        200,
        json!({"audiences": [prism_audience(&did3, "3m6dddddddddd", "v1", &[])]}),
    );
    let gone = format!("at://{did3}/{AUD}/3m6dddddddddd");
    invalid(
        h.private_post_as(&did3, &access3, private_post("x", &gone))
            .await,
        "tombstoned",
    );
    assert!(
        h.state
            .store
            .kr_audience(&did3, "3m6dddddddddd")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn audience_values_and_private_posts_follow_the_host_rules() {
    let h = harness(false).await;
    let (_, seen) = h.with_prism().await;
    let (owner, access) = h.user("max.pds.example.com").await;
    let base = || {
        json!({"$type": AUD, "mode": "list", "members": [],
            "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-01-01T00:00:00.000Z"})
    };
    let with = |k: &str, v: Value| {
        let mut r = base();
        r[k] = v;
        r
    };
    let ok = |r: &Value| crate::audience::check_value(&h.state, &owner, r).is_ok();
    let dids = |n: usize| -> Vec<String> { (0..n).map(|i| format!("did:plc:m{i:023}")).collect() };

    // Members: at most 1000; each a DID of at most 256 bytes.
    assert!(ok(&with("members", json!(dids(1000)))));
    assert!(!ok(&with("members", json!(dids(1001)))), "1001 members");
    let long = format!("did:plc:{}", "a".repeat(248));
    assert_eq!(long.len(), 256);
    assert!(ok(&with("members", json!([long]))));
    assert!(
        !ok(&with("members", json!([format!("{long}a")]))),
        "a 257-byte DID"
    );
    assert!(!ok(&with("members", json!(["not a did"]))));
    assert!(!ok(&with("members", json!("did:plc:abc"))), "not a list");

    // Name: 1-64 graphemes, at most 640 bytes.
    assert!(ok(&with("name", json!("a".repeat(64)))));
    assert!(!ok(&with("name", json!("a".repeat(65)))), "65 graphemes");
    assert!(!ok(&with("name", json!("   "))), "empty once trimmed");
    // A 10-byte grapheme: e-acute and four combining acutes.
    let g10 = "\u{e9}\u{301}\u{301}\u{301}\u{301}";
    assert_eq!(g10.len(), 10);
    assert!(ok(&with("name", json!(g10.repeat(64)))), "640 bytes");
    let g11 = "a\u{301}\u{301}\u{301}\u{301}\u{301}";
    assert_eq!(g11.len(), 11);
    let over = format!("{}{g11}", g10.repeat(63));
    assert!(!ok(&with("name", json!(over))), "641 bytes in 64 graphemes");

    // Body: at most 512 KiB.
    let mut padded = with("pad", json!(""));
    let len = serde_json::to_vec(&padded).unwrap().len();
    padded["pad"] = json!("x".repeat(512 * 1024 - len));
    assert!(ok(&padded), "exactly 512 KiB");
    padded["pad"] = json!("x".repeat(512 * 1024 - len + 1));
    assert!(!ok(&padded), "a byte over");

    // Mode: one of the known ones; `members` required for `list` (also
    // when the mode is left out).
    for m in crate::audience::MODES {
        assert!(ok(&with("mode", json!(m))), "{m}");
    }
    assert!(!ok(&with("mode", json!("friends"))), "unknown mode");
    assert!(!ok(&with("mode", json!(3))), "mode not a string");
    let mut no_members = base();
    no_members.as_object_mut().unwrap().remove("members");
    assert!(!ok(&no_members), "list without members");
    no_members.as_object_mut().unwrap().remove("mode");
    assert!(!ok(&no_members), "default list without members");
    no_members["mode"] = json!("everyone");
    assert!(ok(&no_members), "everyone needs no members");

    // Over HTTP: refused before anything reaches prism.
    let posts = || {
        seen.lock()
            .unwrap()
            .iter()
            .filter(|s| s.method == "POST")
            .count()
    };
    let before = posts();
    for bad in [
        with("members", json!(dids(1001))),
        with("mode", json!("friends")),
        with("name", json!("a".repeat(65))),
    ] {
        let (st, v) = h
            .post(
                "tools.kryphocron.policy.manageAudience",
                Some(&access),
                json!({"repo": owner, "record": bad}),
            )
            .await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
            "{v}"
        );
    }
    assert_eq!(posts(), before, "prism was never asked");

    // Pre-encoded content in a codec locus doesn't support. The content is
    // valid for the record (taken from a post locus encoded at that rkey);
    // only the codec named is wrong.
    let mine = h.add_audience(&owner, "3m6aaaaaaaaaa").await;
    let (st, post) = h
        .private_post_as(&owner, &access, private_post("hello", &mine))
        .await;
    assert_eq!(st, StatusCode::OK, "{post}");
    let rkey = post["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let stored = h.record_value(&owner, POST_PRIVATE, &rkey).await;
    let (st, _) = h
        .post(
            "com.atproto.repo.deleteRecord",
            Some(&access),
            json!({"repo": owner, "collection": POST_PRIVATE, "rkey": rkey}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let put = |codec: &str| {
        let mut rec = stored.clone();
        rec["encodedContentCodec"] = json!(codec);
        h.post(
            "com.atproto.repo.putRecord",
            Some(&access),
            json!({"repo": owner, "collection": POST_PRIVATE, "rkey": rkey, "record": rec}),
        )
    };
    for codec in ["laquna/0.1", "laquna/0.3", "rot13"] {
        let (st, v) = put(codec).await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
            "{codec}: {v}"
        );
    }
    let (_, list) = h
        .get(format!(
            "/xrpc/com.atproto.repo.listRecords?repo={owner}&collection={POST_PRIVATE}"
        ))
        .await;
    assert!(list["records"].as_array().unwrap().is_empty(), "{list}");
    // The supported codec takes the same content.
    let (st, v) = put("laquna/0.2").await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

// ---------------------------------------------------------------- consent pages

/// The consent page's security block (client, account, scopes, the allow
/// and deny forms), with the per-request form tokens blanked.
fn security_block(html: &str) -> String {
    let start = html.find("<h1>").expect("a heading");
    let end = html.rfind("</form>").expect("the forms") + "</form>".len();
    let mut block = html[start..end].to_owned();
    for name in ["csrf", "request_id"] {
        block = block.replace(&hidden(html, name), "-");
    }
    block
}

#[tokio::test]
async fn consent_pages_keep_their_security_block_under_every_theme() {
    use locus_web::branding::{FONTS, RADII, SPACINGS, Theme};
    const LB: &str = "http://localhost?scope=atproto+transition%3Ageneric";
    const HOSTILE: &str = ".scopes { display: none } button.primary { visibility: hidden }";
    let h = harness(false).await;
    h.create_account("cas.pds.example.com", "cas@example.com", "hunter22")
        .await;
    let did = h
        .state
        .store
        .account_by_handle("cas.pds.example.com")
        .await
        .unwrap()
        .unwrap()
        .did;

    // Every bundled font, radius and spacing, with colours, images, links
    // and texts; and themes that carry custom CSS and an uploaded font
    // (portal only) aimed at the consent page.
    let mut themes = vec![Theme::default()];
    for font in FONTS {
        for radius in RADII {
            for spacing in SPACINGS {
                let mut t = Theme {
                    font: font.into(),
                    radius: radius.into(),
                    spacing: spacing.into(),
                    service_name: format!("Brand {font} {radius} {spacing}"),
                    logo: "logo-asset".into(),
                    background_light: "bg-light".into(),
                    background_dark: "bg-dark".into(),
                    ..Theme::default()
                };
                t.light.primary = "#1f6feb".into();
                t.dark.primary = "#4d8dff".into();
                t.links.terms = "https://example.com/terms".into();
                t.text.sign_in_heading = "Welcome back".into();
                themes.push(t);
            }
        }
    }
    themes.push(Theme {
        custom_css: HOSTILE.into(),
        uploaded_font: "font-asset".into(),
        service_name: "<b>Totally Safe</b>".into(),
        ..Theme::default()
    });

    let browser = Browser::default();
    let mut signed_in = false;
    let mut first: Option<String> = None;
    for theme in &themes {
        h.state
            .store
            .save_branding_draft(&serde_json::to_string(theme).unwrap(), &[], "test")
            .await
            .unwrap();
        let version = h
            .state
            .store
            .publish_branding_draft("test")
            .await
            .unwrap()
            .unwrap();
        h.state.branding.invalidate();
        crate::branding::published(&h.state).await;

        // A real authorization request, to its consent page.
        let client = DpopClient::new();
        let (_, challenge) = pkce();
        let (_, _, v) = client
            .post(
                &h,
                "/oauth/par",
                &[
                    ("client_id", LB),
                    ("response_type", "code"),
                    ("scope", "atproto transition:generic"),
                    ("redirect_uri", "http://127.0.0.1/"),
                    ("code_challenge", &challenge),
                    ("code_challenge_method", "S256"),
                ],
            )
            .await;
        let path = format!(
            "/oauth/authorize?client_id={}&request_uri={}",
            url::form_urlencoded::byte_serialize(LB.as_bytes()).collect::<String>(),
            url::form_urlencoded::byte_serialize(v["request_uri"].as_str().unwrap().as_bytes())
                .collect::<String>()
        );
        let (_, _, page) = browser.get(&h, &path).await;
        let (csrf, request_id) = (hidden(&page, "csrf"), hidden(&page, "request_id"));
        let (_, _, html) = if signed_in {
            browser
                .post(
                    &h,
                    "/oauth/authorize/select",
                    &[("csrf", &csrf), ("request_id", &request_id), ("did", &did)],
                )
                .await
        } else {
            signed_in = true;
            browser
                .post(
                    &h,
                    "/oauth/authorize/sign-in",
                    &[
                        ("csrf", &csrf),
                        ("request_id", &request_id),
                        ("identifier", "cas.pds.example.com"),
                        ("password", "hunter22"),
                    ],
                )
                .await
        };
        assert!(html.contains("wants access"), "{html}");

        // The block is there, the same under every theme.
        let block = security_block(&html);
        assert!(
            block.contains("@cas.pds.example.com")
                && block.contains("Allow")
                && block.contains("Deny")
                && block.contains(r#"value="transition:generic""#),
            "{block}"
        );
        match &first {
            None => first = Some(block),
            Some(f) => assert_eq!(&block, f, "under {theme:?}"),
        }
        // Only the checked variables reach it: no custom CSS, no uploaded
        // font, inline or linked.
        assert!(
            html.contains(&format!("/branding/theme/{version}/vars.css"))
                && !html.contains("full.css")
                && !html.contains("display: none")
                && !html.contains("<b>Totally"),
            "{html}"
        );
        let (_, _, css) = h
            .raw(
                "GET",
                &format!("/branding/theme/{version}/vars.css"),
                None,
                vec![],
                &[],
            )
            .await;
        let css = String::from_utf8(css).unwrap();
        assert!(
            !css.contains("display") && !css.contains("visibility") && !css.contains("@font-face"),
            "{css}"
        );
    }
}

/// With a block-source AppView set, app.bsky block checks for a remote
/// parent author ask it (as the writer, its DID the token's audience),
/// not prism; it fails closed the same way; unset, the (public) AppView is
/// asked, never prism; with no AppView either, it fails closed.
#[tokio::test]
async fn block_checks_ask_the_block_source_appview_when_one_is_set() {
    use base64::Engine;
    let h = harness_build(HarnessOpts {
        ssrf_protection: false,
        ..HarnessOpts::default()
    })
    .await;
    let (prism, prism_seen) = h.with_prism().await;
    // Prism doesn't answer getRelationships (as it may not).
    set_script(
        &prism,
        "app.bsky.graph.getRelationships",
        501,
        json!({"error": "MethodNotImplemented"}),
    );
    let (bsky_url, bsky, bsky_seen) = scripted_appview().await;
    let author = format!("did:plc:{}", "q".repeat(24));
    let audience = format!("at://{author}/{AUD}/3m6aaaaaaaaaa");
    let parent_uri = format!("at://{author}/{POST_PRIVATE}/3m6parentpare");
    let parent = json!({"$type": POST_PRIVATE, "createdAt": "2024-01-01T00:00:00.000Z",
        "audienceList": audience, "encodedContent": {"$bytes": "AAECAwQFBgc"},
        "encodedContentCodec": "laquna/0.2"});
    let key = Arc::new(proto_blue_crypto::K256Keypair::generate());
    let remote = mock_remote_pds(parent, audience.clone(), key.clone()).await;
    h.plc.lock().unwrap().log.insert(
        author.clone(),
        vec![json!({"type": "plc_operation", "alsoKnownAs": ["at://far.example.net"],
            "verificationMethods": {"atproto": key.did()},
            "services": {"atproto_pds": {"type": "AtprotoPersonalDataServer", "endpoint": remote}}})],
    );
    h.state
        .kr
        .membership
        .seed_issuer_key("127.0.0.1", &key.did());
    let parent_ref = json!({"uri": parent_uri, "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"});
    // One writer per case: block answers are cached per pair.
    let mut writers = Vec::new();
    for (handle, rkey) in [
        ("bya.pds.example.com", "3m6aaaaaaaaab"),
        ("byb.pds.example.com", "3m6aaaaaaaaac"),
        ("byc.pds.example.com", "3m6aaaaaaaaad"),
        ("byd.pds.example.com", "3m6aaaaaaaaae"),
        ("bye.pds.example.com", "3m6aaaaaaaaaf"),
    ] {
        let (did, access) = h.user(handle).await;
        let mine = h.add_audience(&did, rkey).await;
        let (st, v) = h
            .private_post_as(&did, &access, private_post("x", &mine))
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        writers.push((did, access));
    }
    let reply = |w: usize| h.reply_as(&writers[w].0, &writers[w].1, &parent_ref, &audience);
    let set = |k: &'static str, v: Value| {
        let h = &h;
        async move { h.state.settings.set(k, v, "test").await.unwrap() }
    };
    set("kryphocron.block_appview_url", json!(bsky_url)).await;
    set(
        "kryphocron.block_appview_did",
        json!("did:web:api.bsky.test"),
    )
    .await;

    // No block there: the reply goes through though prism can't answer.
    set_script(
        &bsky,
        "app.bsky.graph.getRelationships",
        200,
        json!({"relationships": [{"did": author}]}),
    );
    let (st, v) = reply(0).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let asked = bsky_seen
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.uri.contains("getRelationships"))
        .cloned()
        .unwrap();
    let token = asked.headers["authorization"]
        .to_str()
        .unwrap()
        .trim_start_matches("Bearer ")
        .to_owned();
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(token.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(claims["aud"], "did:web:api.bsky.test");
    assert_eq!(claims["iss"], json!(writers[0].0), "asked as the writer");
    assert!(
        !prism_seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.uri.contains("getRelationships")),
        "prism wasn't asked"
    );
    // A block there is enforced; an error there fails closed.
    set_script(
        &bsky,
        "app.bsky.graph.getRelationships",
        200,
        json!({"relationships": [{"did": author, "blocking": "at://x/app.bsky.graph.block/1"}]}),
    );
    assert_eq!(reply(1).await.0, StatusCode::FORBIDDEN);
    set_script(
        &bsky,
        "app.bsky.graph.getRelationships",
        500,
        json!({"error": "InternalServerError"}),
    );
    assert_eq!(reply(2).await.0, StatusCode::FORBIDDEN);
    // Unset: the public AppView is asked, not prism.
    set("kryphocron.block_appview_url", json!("")).await;
    let (pub_url, public, pub_seen) = scripted_appview().await;
    set_script(
        &public,
        "app.bsky.graph.getRelationships",
        200,
        json!({"relationships": [{"did": author}]}),
    );
    set("appview.url", json!(pub_url)).await;
    set("appview.did", json!("did:web:appview.test")).await;
    let (st, v) = reply(3).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        pub_seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.uri.contains("getRelationships"))
    );
    assert!(
        !prism_seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.uri.contains("getRelationships")),
        "prism is never the block source"
    );
    // No AppView at all: fails closed.
    set("appview.url", json!("")).await;
    assert_eq!(reply(4).await.0, StatusCode::FORBIDDEN);
}
