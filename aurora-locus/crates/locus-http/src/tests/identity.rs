//! Host routing, outbound request rules (SSRF, dev mode), hostname
//! reservations and hostname migrations.

use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

// ---------------------------------------------------------------- helpers

/// Paths a local service was asked for.
type Hits = Arc<Mutex<Vec<String>>>;

/// A service on 127.0.0.1 over plain HTTP: it serves its own did:web
/// document (`did:web:localhost%3A<port>`, service `#svc` at
/// `http://localhost:<port>`) and records every path asked for.
async fn local_service() -> (u16, Hits) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits: Hits = Arc::default();
    let seen = hits.clone();
    let app = Router::new().fallback(move |req: Request<Body>| {
        let seen = seen.clone();
        async move {
            let path = req.uri().path().to_owned();
            seen.lock().unwrap().push(path.clone());
            if path == "/.well-known/did.json" {
                let did = format!("did:web:localhost%3A{port}");
                axum::Json(json!({
                    "@context": ["https://www.w3.org/ns/did/v1"],
                    "id": did,
                    "service": [{"id": "#svc", "type": "LocalService",
                        "serviceEndpoint": format!("http://localhost:{port}")}],
                }))
                .into_response()
            } else {
                axum::Json(json!({"ok": true})).into_response()
            }
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, hits)
}

/// A listener that only counts the connections it gets (a TLS handshake
/// to it fails, but the connection shows a request was attempted).
async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = Arc::clone(&hits);
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            h.fetch_add(1, Ordering::SeqCst);
            drop(sock);
        }
    });
    (port, hits)
}

/// A did:plc in the mock directory with these services (`tag`: letters).
fn fake_did(h: &Harness, tag: &str, services: Value) -> String {
    let did = format!("did:plc:{tag:a<24}");
    h.plc.lock().unwrap().log.insert(
        did.clone(),
        vec![json!({
            "type": "plc_operation",
            "alsoKnownAs": [],
            "rotationKeys": [],
            "verificationMethods": {"atproto": proto_blue_crypto::K256Keypair::generate().did()},
            "services": services,
        })],
    );
    did
}

fn svc(id: &str, ty: &str, endpoint: &str) -> Value {
    json!({ id: {"type": ty, "endpoint": endpoint} })
}

fn hit(hits: &Hits, path: &str) -> bool {
    hits.lock().unwrap().iter().any(|p| p == path)
}

impl Harness {
    /// A call with `atproto-proxy` set (GET, or POST with a JSON body).
    async fn via_proxy(
        &self,
        access: &str,
        proxy: &str,
        lxm: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (method, bytes) = match &body {
            Some(b) => ("POST", b.to_string().into_bytes()),
            None => ("GET", vec![]),
        };
        let (st, _, out) = self
            .raw(
                method,
                &format!("/xrpc/{lxm}"),
                Some(access),
                bytes,
                &[
                    ("atproto-proxy", proxy),
                    ("content-type", "application/json"),
                ],
            )
            .await;
        (st, serde_json::from_slice(&out).unwrap_or(Value::Null))
    }

    async fn register_push(&self, access: &str, service_did: &str) -> (StatusCode, Value) {
        self.post(
            "app.bsky.notification.registerPush",
            Some(access),
            json!({"serviceDid": service_did, "token": "t", "platform": "ios", "appId": "x"}),
        )
        .await
    }
}

// ---------------------------------------------------------------- SSRF

/// What a user-influenced target may be, with SSRF protection on (the
/// default) or off (`PDS_DISABLE_SSRF_PROTECTION=true` on any hostname, or
/// `PDS_DEV_MODE` on a local one; both reach the node as this flag).
async fn outbound_rules(protection: bool) {
    let h = harness_build(HarnessOpts {
        ssrf_protection: protection,
        ..HarnessOpts::default()
    })
    .await;
    let (_, access) = h.user("sam.pds.example.com").await;
    let (appview, appview_seen) = mock_service().await;
    for (k, v) in [
        ("appview.url", appview.as_str()),
        ("appview.did", "did:web:appview.test"),
    ] {
        h.state.settings.set(k, json!(v), "test").await.unwrap();
    }
    h.state
        .settings
        .set("proxy.headers_timeout_ms", json!(2000), "test")
        .await
        .unwrap();
    let off = !protection;
    let (port, hits) = local_service().await;
    let local = format!("http://localhost:{port}");
    let literal = format!("http://127.0.0.1:{port}");
    let ping = "com.example.ssrf.ping";
    let ping_path = format!("/xrpc/{ping}");

    // A did:web:localhost service: its document over plain HTTP from
    // loopback, then the call proxied there. Only with protection off.
    let (st, v) = h
        .via_proxy(&access, &format!("did:web:localhost%3A{port}#svc"), ping, None)
        .await;
    assert_eq!(st.is_success(), off, "did:web:localhost: {st} {v}");
    assert_eq!(hit(&hits, "/.well-known/did.json"), off);
    assert_eq!(hit(&hits, &ping_path), off);

    // A local feed generator (a private address over plain HTTP).
    let fg = fake_did(&h, "feedgen", svc("bsky_fg", "BskyFeedGenerator", &local));
    let (st, v) = h
        .via_proxy(
            &access,
            &format!("{fg}#bsky_fg"),
            "app.bsky.feed.getFeedSkeleton",
            None,
        )
        .await;
    assert_eq!(st.is_success(), off, "feed generator: {st} {v}");
    assert_eq!(hit(&hits, "/xrpc/app.bsky.feed.getFeedSkeleton"), off);

    // Pipethrough may target an IP literal, as the reference's proxy agent
    // (public unicast over HTTPS with protection on; anything with it off).
    hits.lock().unwrap().clear();
    let lit = fake_did(&h, "literal", svc("svc", "LocalService", &literal));
    let (st, v) = h
        .via_proxy(&access, &format!("{lit}#svc"), ping, None)
        .await;
    assert_eq!(st.is_success(), off, "IP literal: {st} {v}");
    assert_eq!(hit(&hits, &ping_path), off);
    if protection {
        // HTTPS to loopback, CGNAT, private, mapped and link-local literals:
        // refused before connecting.
        let (tls_port, connections) = counting_listener().await;
        for (tag, endpoint) in [
            ("loopback", format!("https://127.0.0.1:{tls_port}")),
            ("mapped", format!("https://[::ffff:127.0.0.1]:{tls_port}")),
            ("cgnat", "https://100.64.0.1".to_owned()),
            ("private", "https://10.0.0.1".to_owned()),
            ("linklocal", "https://169.254.169.254".to_owned()),
            ("ula", "https://[fd00::1]".to_owned()),
        ] {
            let did = fake_did(&h, tag, svc("svc", "LocalService", &endpoint));
            let (st, v) = h
                .via_proxy(&access, &format!("{did}#svc"), ping, None)
                .await;
            assert_eq!(
                (st, v["error"].as_str()),
                (StatusCode::BAD_GATEWAY, Some("UpstreamFailure")),
                "{tag}: {v}"
            );
        }
        assert_eq!(connections.load(Ordering::SeqCst), 0, "nothing connected");
    }

    // createReport keeps the stricter rule: never an IP literal, in any
    // mode; a name is reached only with protection off.
    hits.lock().unwrap().clear();
    let report = json!({"reasonType": "com.atproto.moderation.defs#reasonSpam",
        "subject": {"$type": "com.atproto.admin.defs#repoRef", "did": "did:plc:abc"}});
    let report_path = "/xrpc/com.atproto.moderation.createReport";
    let (st, v) = h
        .via_proxy(
            &access,
            &format!("{lit}#svc"),
            "com.atproto.moderation.createReport",
            Some(report.clone()),
        )
        .await;
    assert!(!st.is_success(), "report to an IP literal: {v}");
    assert!(!hit(&hits, report_path));
    let named = fake_did(&h, "named", svc("svc", "LocalService", &local));
    let (st, v) = h
        .via_proxy(
            &access,
            &format!("{named}#svc"),
            "com.atproto.moderation.createReport",
            Some(report),
        )
        .await;
    assert_eq!(st.is_success(), off, "report to a name: {st} {v}");
    assert_eq!(hit(&hits, report_path), off);

    // registerPush: a push service named by DID keeps the stricter rule;
    // the configured AppView is reached as configured.
    let push_path = "/xrpc/app.bsky.notification.registerPush";
    let notif = |tag: &str, endpoint: &str| {
        fake_did(
            &h,
            tag,
            svc("bsky_notif", "BskyNotificationService", endpoint),
        )
    };
    let push_literal = notif("pushlit", &literal);
    let (st, v) = h.register_push(&access, &push_literal).await;
    assert!(!st.is_success(), "push to an IP literal: {v}");
    assert!(!hit(&hits, push_path));
    let push_named = notif("pushnamed", &local);
    let (st, v) = h.register_push(&access, &push_named).await;
    assert_eq!(st.is_success(), off, "push to a name: {st} {v}");
    assert_eq!(hit(&hits, push_path), off);
    let (st, v) = h.register_push(&access, "did:web:appview.test").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(
        appview_seen
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.uri.starts_with(push_path))
    );

    // DID documents and OAuth client metadata on an IP-literal host are
    // refused before connecting, in every mode.
    let (tls_port, connections) = counting_listener().await;
    let (st, v) = h
        .via_proxy(
            &access,
            &format!("did:web:127.0.0.1%3A{tls_port}#svc"),
            ping,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        crate::oauth::get_client(
            &h.state,
            &format!("https://127.0.0.1:{tls_port}/client-metadata.json")
        )
        .await
        .is_err()
    );
    assert_eq!(connections.load(Ordering::SeqCst), 0, "nothing connected");

    // include: resolution reads the permission set from the authority's
    // PDS under the stricter rule too.
    hits.lock().unwrap().clear();
    let get_record = "/xrpc/com.atproto.sync.getRecord";
    for (tag, endpoint, reached) in [
        ("lexlit", literal.as_str(), false),
        ("lexnamed", local.as_str(), off),
    ] {
        let did = fake_did(
            &h,
            tag,
            svc("atproto_pds", "AtprotoPersonalDataServer", endpoint),
        );
        h.state
            .settings
            .set("oauth.lexicon_authority_did", json!(did), "test")
            .await
            .unwrap();
        let scope = format!("atproto include:com.example.{tag}.auth");
        assert!(crate::lexicon::token_scope(&h.state, &scope).await.is_err());
        assert_eq!(hit(&hits, get_record), reached, "{tag}");
        hits.lock().unwrap().clear();
    }
}

#[tokio::test]
async fn with_ssrf_protection_on_private_targets_and_ip_literals_are_refused() {
    outbound_rules(true).await;
}

#[tokio::test]
async fn with_ssrf_protection_off_private_targets_are_reached_but_ip_literal_documents_are_not() {
    outbound_rules(false).await;
}

#[tokio::test]
async fn the_console_names_nodes_without_ssrf_protection_and_nodes_that_disagree() {
    use locus_store::env::CLOCK_SLOT_FAMILY;
    let h = harness(false).await;
    h.console_operator("cora", "viewer", "cora-password-1234")
        .await;
    let b = Browser::default();
    h.console_sign_in(&b, "cora", "cora-password-1234").await;
    let ssrf_off = "SSRF protection is off on node-b";
    let dev_ignored = "PDS_DEV_MODE is ignored on node-b";
    let disagree = "disagree on ssrf_protection";
    let page = || async { b.get(&h, "/admin").await.2 };

    // Two live nodes; node-b boots with SSRF protection off and dev mode
    // asked for on a public hostname.
    let ttl = std::time::Duration::from_secs(30);
    let mut leases = Vec::new();
    for n in ["node-a", "node-b"] {
        let (_, lease) = h
            .state
            .store
            .claim_slot(CLOCK_SLOT_FAMILY, 8, n, ttl)
            .await
            .unwrap()
            .unwrap();
        leases.push(lease);
    }
    let on = [("role", "Pds"), ("dev_mode", "off"), ("ssrf_protection", "on")];
    h.state.store.report_bootstrap("node-a", &on).await.unwrap();
    h.state.store.report_bootstrap("node-b", &on).await.unwrap();
    let p = page().await;
    assert!(
        !p.contains(ssrf_off) && !p.contains(dev_ignored) && !p.contains("disagree on"),
        "nothing to warn about"
    );
    let off = [
        ("role", "Pds"),
        ("dev_mode", "ignored"),
        ("ssrf_protection", "off"),
    ];
    let differ = h.state.store.report_bootstrap("node-b", &off).await.unwrap();
    assert_eq!(differ, ["dev_mode", "ssrf_protection"]);
    let p = page().await;
    assert!(p.contains(ssrf_off), "{p}");
    assert!(p.contains(dev_ignored));
    assert!(p.contains(disagree));
    // On every page, until node-b stops.
    let (_, _, other) = b.get(&h, "/admin/security").await;
    assert!(other.contains(ssrf_off));
    h.state.store.release_lease(&leases[1]).await.unwrap();
    let p = page().await;
    assert!(!p.contains(ssrf_off) && !p.contains(disagree), "node-b stopped");
}

// ---------------------------------------------------------------- host routing

impl Harness {
    /// A request to `host` with these headers; status and JSON (or text).
    async fn at_host(
        &self,
        host: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", host);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let body = if method == "POST" { "{}" } else { "" };
        let res = self
            .app
            .clone()
            .oneshot(
                req.header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, v)
    }

    async fn create_web_account(&self, handle: &str) -> Value {
        let (st, v) = self
            .call(
                "POST",
                "/xrpc/com.atproto.server.createAccount",
                None,
                Some(json!({"handle": handle, "email": format!("{handle}@x.com"),
                    "password": "pw", "locusDidMethod": "web"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v
    }
}

#[tokio::test]
async fn handle_hosts_get_only_their_documents_and_internal_hosts_only_the_pds_identity() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, access) = h.user("yan.pds.example.com").await;
    let web = h.create_web_account("wes.pds.example.com").await;
    let viewer = format!("Bearer {}", h.operator("vic", "viewer").await);
    let bearer = format!("Bearer {access}");

    // A user's host gets its identity documents and health, nothing else,
    // with or without credentials: no XRPC, console, OAuth, portal, setup
    // or operator method.
    let swept = [
        ("GET", "/xrpc/com.atproto.server.describeServer".to_owned()),
        ("POST", "/xrpc/com.atproto.server.createSession".to_owned()),
        ("GET", "/xrpc/com.atproto.server.getSession".to_owned()),
        ("GET", "/xrpc/app.bsky.feed.getTimeline".to_owned()),
        ("GET", "/xrpc/com.atproto.sync.subscribeRepos".to_owned()),
        ("GET", "/admin".to_owned()),
        ("GET", "/admin/sign-in".to_owned()),
        ("GET", "/oauth/authorize".to_owned()),
        ("POST", "/oauth/par".to_owned()),
        ("GET", "/.well-known/oauth-authorization-server".to_owned()),
        ("GET", "/.well-known/oauth-protected-resource".to_owned()),
        ("GET", "/account".to_owned()),
        ("GET", "/account/sign-in".to_owned()),
        ("GET", "/setup".to_owned()),
        ("GET", "/metrics".to_owned()),
        ("GET", ns("viewer.hostname.status")),
    ];
    for host in ["yan.pds.example.com", "wes.pds.example.com"] {
        for (method, path) in &swept {
            for auth in [None, Some(bearer.as_str()), Some(viewer.as_str())] {
                let hs: Vec<(&str, &str)> =
                    auth.map(|a| ("authorization", a)).into_iter().collect();
                let (st, _) = h.at_host(host, method, path, &hs).await;
                assert_eq!(st, StatusCode::NOT_FOUND, "{method} {path} on {host}");
            }
        }
        let (st, _) = h.at_host(host, "GET", "/xrpc/_health", &[]).await;
        assert_eq!(st, StatusCode::OK);
    }
    let (st, v) = h
        .at_host("yan.pds.example.com", "GET", "/.well-known/atproto-did", &[])
        .await;
    assert_eq!((st, v.as_str()), (StatusCode::OK, Some(did.as_str())));
    let (st, doc) = h
        .at_host("wes.pds.example.com", "GET", "/.well-known/did.json", &[])
        .await;
    assert_eq!((st, &doc["id"]), (StatusCode::OK, &web["did"]));

    // Internal names get the routes but only the PDS's identity, and the
    // privileged checks answer exactly as on the PDS hostname.
    let status = ns("viewer.hostname.status");
    let owner_only = ns("owner.listOperators");
    for host in ["pds.example.com", "localhost", "127.0.0.1", "[::1]", "locus"] {
        let (_, v) = h
            .at_host(host, "GET", "/xrpc/com.atproto.server.describeServer", &[])
            .await;
        assert_eq!(v["did"], json!("did:web:pds.example.com"), "{host}");
        // No identity of its own: the service document, if any, is the PDS's.
        let (st, doc) = h.at_host(host, "GET", "/.well-known/did.json", &[]).await;
        assert!(
            st != StatusCode::OK || doc["id"] == json!("did:web:pds.example.com"),
            "{host}: {doc}"
        );
        let (_, pr) = h
            .at_host(host, "GET", "/.well-known/oauth-protected-resource", &[])
            .await;
        assert_eq!(pr["resource"], json!("https://pds.example.com"), "{host}");
        assert_eq!(
            pr["authorization_servers"],
            json!(["https://pds.example.com"])
        );
        for (path, auth, expected) in [
            (&status, None, StatusCode::UNAUTHORIZED),
            (&status, Some(viewer.as_str()), StatusCode::OK),
            (&status, Some(bearer.as_str()), StatusCode::UNAUTHORIZED),
            (&owner_only, Some(viewer.as_str()), StatusCode::FORBIDDEN),
        ] {
            let hs: Vec<(&str, &str)> = auth.map(|a| ("authorization", a)).into_iter().collect();
            let (st, v) = h.at_host(host, "GET", path, &hs).await;
            assert_eq!(st, expected, "{path} on {host}: {v}");
        }
        let (st, _) = h
            .at_host(
                host,
                "GET",
                &owner_only,
                &[(crate::socket::MARKER, "guess")],
            )
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{host}");
        // The console sends a visitor without a session to sign in.
        let (st, _) = h.at_host(host, "GET", "/admin", &[]).await;
        assert_eq!(st, StatusCode::SEE_OTHER, "{host}");
    }
    for name in ["localhost", "locus", "127.0.0.1"] {
        assert!(!h.state.accepts_aud(&format!("did:web:{name}")));
    }

    // Over TCP the socket marker is stripped whatever the Host says.
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    tokio::spawn(crate::serve(h.state.clone(), tcp, cancel.clone()));
    for host in ["localhost", "pds.example.com"] {
        let res = reqwest::Client::new()
            .get(format!("http://{addr}{owner_only}"))
            .header("host", host)
            .header(crate::socket::MARKER, &h.state.socket_secret)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 401, "{host}");
    }
    cancel.cancel();
}

// ---------------------------------------------------------------- reservations

#[tokio::test]
async fn internal_hosts_and_user_hosts_are_reserved_against_each_other() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let admin = format!("Bearer {}", h.operator("ada", "admin").await);
    let owner = h.owner_with_password("oto", "oto-password-1234").await;
    let (_, access) = h.user("yan.pds.example.com").await;
    h.create_web_account("wes.pds.example.com").await;
    // This PDS moved here from old.example.net (now a former hostname).
    h.state
        .store
        .prepare_hostname_migration("m1", "old.example.net", "pds.example.com", "test")
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .confirm_hostname_migration("m1", 0)
            .await
            .unwrap()
    );
    h.state
        .store
        .finish_hostname_migration("m1", true)
        .await
        .unwrap();
    let set_path = ns("admin.setSetting");
    let set = |v: Value| {
        h.admin_call(
            "POST",
            &set_path,
            &admin,
            Some(json!({"key": "http.internal_hosts", "value": v})),
        )
    };
    let taken = |(st, v): (StatusCode, Value), why: &str| {
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("HandleNotAvailable")),
            "{why}: {v}"
        );
    };

    // An internal name can't then be taken as a handle or a did:web host.
    let (st, v) = set(json!(["int.pds.example.com"])).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    taken(
        h.update_handle(&access, "int.pds.example.com").await,
        "internal name as a handle",
    );
    let body = json!({"handle": "int.pds.example.com", "email": "i@x.com", "password": "pw",
        "locusDidMethod": "web"});
    taken(
        h.call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(body),
        )
        .await,
        "internal name as did:web",
    );

    // Saving an entry that is a handle, a did:web host or a PDS hostname
    // (current, prepared or old) is refused and changes nothing.
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.prepare"),
            &owner,
            Some(json!({"host": "next.example.net"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    for entry in [
        "yan.pds.example.com",
        "WES.pds.example.com",
        "pds.example.com",
        "next.example.net",
        "old.example.net",
    ] {
        let (st, v) = set(json!(["lan.internal", entry])).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{entry}: {v}");
    }
    assert_eq!(
        h.state.snapshot().list("http.internal_hosts"),
        ["int.pds.example.com"]
    );
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.cancel"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = set(json!(["next.example.net"])).await;
    assert_eq!(st, StatusCode::OK, "a cancelled candidate is free: {v}");

    // `migrate hostname` refuses a user's host: a local handle, an external
    // handle, a hosted did:web host (also once its account is deleted).
    let (ext_did, _) = h.user("eve.pds.example.com").await;
    let mut tx = h.state.store.begin().await.unwrap();
    tx.set_account_handle(&ext_did, "eve.example.org")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let gone = h.create_web_account("gil.pds.example.com").await;
    let gone_did = gone["did"].as_str().unwrap();
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.admin.deleteAccount",
            None,
            Some(json!({"did": gone_did})),
        )
        .await;
    assert_ne!(st, StatusCode::OK, "needs the admin password: {v}");
    let (st, v) = h
        .admin_call(
            "POST",
            "/xrpc/com.atproto.admin.deleteAccount",
            &h.basic_admin(),
            Some(json!({"did": gone_did})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state.store.account_by_did(gone_did).await.unwrap().is_some() {
        assert!(tokio::time::Instant::now() < deadline, "deletion never finished");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let prepare_path = ns("owner.hostname.prepare");
    let prepare = |host: &str| {
        h.admin_call(
            "POST",
            &prepare_path,
            &owner,
            Some(json!({"host": host})),
        )
    };
    for host in [
        "yan.pds.example.com",
        "eve.example.org",
        "wes.pds.example.com",
        "gil.pds.example.com",
    ] {
        let (st, v) = prepare(host).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{host}: {v}");
        assert!(h.state.store.open_hostname_migration().await.unwrap().is_none());
    }

    // An old PDS hostname is accepted (migrating back).
    let (st, v) = prepare("old.example.net").await;
    assert_eq!(st, StatusCode::OK, "an old PDS hostname: {v}");
}

// ---------------------------------------------------------------- migrations

impl Harness {
    /// A running migration from `old` to this node's hostname, loaded.
    async fn migrated_from(&self, id: &str, old: &str) {
        self.state
            .store
            .prepare_hostname_migration(id, old, &self.state.hostname, "test")
            .await
            .unwrap();
        assert!(
            self.state
                .store
                .confirm_hostname_migration(id, 0)
                .await
                .unwrap()
        );
        self.reload_hosts().await;
    }

    /// Points an account's PLC document at `endpoint`, optionally with
    /// rotation keys this PDS doesn't hold.
    fn plc_edit(&self, did: &str, endpoint: &str, foreign_keys: bool) {
        let mut plc = self.plc.lock().unwrap();
        let op = plc.log.get_mut(did).unwrap().last_mut().unwrap();
        op["services"]["atproto_pds"]["endpoint"] = json!(endpoint);
        if foreign_keys {
            op["rotationKeys"] = json!([proto_blue_crypto::K256Keypair::generate().did()]);
        }
    }

    /// A GET with an OAuth token to `host`, its DPoP proof for
    /// `https://<htu_host><path>`; retried once for a fresh nonce.
    async fn oauth_get_at(
        &self,
        client: &DpopClient,
        token: &str,
        host: &str,
        htu_host: &str,
        path: &str,
    ) -> (StatusCode, Value) {
        let auth = format!("DPoP {token}");
        for _ in 0..2 {
            let proof = client.proof("GET", &format!("https://{htu_host}{path}"), Some(token));
            let req = Request::builder()
                .method("GET")
                .uri(path)
                .header("host", host)
                .header("authorization", &auth)
                .header("dpop", proof)
                .body(Body::empty())
                .unwrap();
            let res = self.app.clone().oneshot(req).await.unwrap();
            if let Some(n) = res.headers().get("dpop-nonce") {
                *client.nonce.lock().unwrap() = Some(n.to_str().unwrap().to_owned());
            }
            let st = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            if v["error"] == "use_dpop_nonce" {
                continue;
            }
            return (st, v);
        }
        panic!("the nonce retry didn't succeed");
    }
}

#[tokio::test]
async fn the_window_outlasts_the_job_and_stragglers_do_not_block_the_next_migration() {
    let h = harness(false).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("oto", "oto-password-1234").await;
    let old = "old.example.net";
    let (ann, _) = h.user("ann.pds.example.com").await;
    let (bo, _) = h.user("bob.pds.example.com").await;
    // Both documents still name the old host; bo's lists none of this
    // PDS's rotation keys, so the job can't move it.
    h.plc_edit(&ann, &format!("https://{old}"), false);
    h.plc_edit(&bo, &format!("https://{old}"), true);
    // A zero-day window: only the job holds the migration open.
    h.migrated_from("m1", old).await;
    let close_path = ns("owner.hostname.close");
    let close = || h.admin_call("POST", &close_path, &owner, Some(json!({})));
    let prepare_path = ns("owner.hostname.prepare");
    let prepare = |host: &str| {
        h.admin_call(
            "POST",
            &prepare_path,
            &owner,
            Some(json!({"host": host})),
        )
    };

    // While the job runs, the migration can't close and no other starts.
    let (st, v) = close().await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(prepare("third.example.net").await.0.is_client_error());
    // The job's pass moves ann and lists bo; with bo left the window
    // doesn't end however late it is.
    assert!(crate::hostnames::step(&h.state).await.unwrap());
    assert!(!crate::hostnames::step(&h.state).await.unwrap());
    let m = &h.state.store.hostname_migrations().await.unwrap()[0];
    assert_eq!((m.state.as_str(), m.job_done_at.is_some()), ("running", true));
    let stragglers = h.state.store.hostname_stragglers("m1", 10).await.unwrap();
    assert_eq!(
        stragglers.iter().map(|x| x.did.as_str()).collect::<Vec<_>>(),
        [bo.as_str()]
    );
    assert_eq!(stragglers[0].state, "cannot");

    // Closed with bo still listed: the slot frees, the old host keeps full
    // service (and its audience and protected resource) for bo.
    let (st, v) = close().await;
    assert_eq!((st, &v["stragglers"]), (StatusCode::OK, &json!(1)), "{v}");
    h.reload_hosts().await;
    assert_eq!(h.state.hosts().old_full, [old]);
    let (st, _) = h
        .at_host(old, "GET", "/xrpc/com.atproto.server.describeServer", &[])
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(h.state.accepts_aud(&format!("did:web:{old}")));
    let (_, asm) = h
        .at_host(
            "pds.example.com",
            "GET",
            "/.well-known/oauth-authorization-server",
            &[],
        )
        .await;
    assert!(
        asm["protected_resources"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("https://{old}")))
    );
    let (_, v) = h
        .admin_call("GET", &ns("viewer.hostname.status"), &owner, None)
        .await;
    assert_eq!(v["migrations"][0]["stragglers"][0]["did"], json!(bo), "{v}");
    let (st, v) = prepare("third.example.net").await;
    assert_eq!(st, StatusCode::OK, "a new migration can be prepared: {v}");
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.cancel"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);

    // bo updates its own document; the next daily recheck sees it, and the
    // old host becomes former: did.json only, no audience, no resource.
    h.plc_edit(&bo, &h.state.public_url, false);
    assert!(!crate::hostnames::step(&h.state).await.unwrap(), "not rechecked yet");
    sqlx::query("UPDATE hostname_migration SET rechecked_at = NULL")
        .execute(match &h.state.store {
            Store::Sqlite(p) => p,
            Store::Postgres(_) => unreachable!(),
        })
        .await
        .unwrap();
    assert!(crate::hostnames::step(&h.state).await.unwrap());
    h.reload_hosts().await;
    assert_eq!(h.state.hosts().former, [old]);
    assert!(h.state.hosts().old_full.is_empty());
    assert!(!h.state.accepts_aud(&format!("did:web:{old}")));
    let (_, asm) = h
        .at_host(
            "pds.example.com",
            "GET",
            "/.well-known/oauth-authorization-server",
            &[],
        )
        .await;
    assert_eq!(
        asm["protected_resources"],
        json!(["https://pds.example.com"])
    );
    let (st, _) = h
        .at_host(old, "GET", "/xrpc/com.atproto.server.describeServer", &[])
        .await;
    assert_eq!(st, StatusCode::PERMANENT_REDIRECT);
    let (st, doc) = h.at_host(old, "GET", "/.well-known/did.json", &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        doc["verificationMethod"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == json!(format!("did:web:{old}#kryphocron_audience"))),
        "{doc}"
    );
}

/// The id of the service document `host` serves, if it serves one.
async fn served_doc(h: &Harness, host: &str) -> Option<Value> {
    let (st, doc) = h.at_host(host, "GET", "/.well-known/did.json", &[]).await;
    (st == StatusCode::OK).then(|| doc["id"].clone())
}

#[tokio::test]
async fn migrating_back_to_a_former_hostname() {
    let h = harness_at("social.example.com", false, true).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let owner = h.owner_with_password("oto", "oto-password-1234").await;
    let (_, access) = h.user("ida.social.example.com").await;
    let back = "pds.example.com";
    // pds.example.com was the hostname; it is former now.
    h.migrated_from("m1", back).await;
    h.state
        .store
        .finish_hostname_migration("m1", true)
        .await
        .unwrap();
    h.reload_hosts().await;
    let describe = "/xrpc/com.atproto.server.describeServer";
    assert_eq!(served_doc(&h, back).await, Some(json!(format!("did:web:{back}"))));

    // Prepared again: a candidate, which keeps its former did.json and
    // gets health and /tls-check but nothing else (no redirect either).
    let prepare_path = ns("owner.hostname.prepare");
    let prepare = || {
        h.admin_call(
            "POST",
            &prepare_path,
            &owner,
            Some(json!({"host": back})),
        )
    };
    let (st, v) = prepare().await;
    assert_eq!(st, StatusCode::OK, "{v}");
    h.reload_hosts().await;
    assert_eq!(h.state.hosts().candidate.as_deref(), Some(back));
    assert_eq!(served_doc(&h, back).await, Some(json!(format!("did:web:{back}"))));
    let (st, _) = h.at_host(back, "GET", describe, &[]).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(crate::health::approved(&h.state, back).await);

    // Cancelled: former again, and still reserved.
    let (st, _) = h
        .admin_call(
            "POST",
            &ns("owner.hostname.cancel"),
            &owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    h.reload_hosts().await;
    assert_eq!(h.state.hosts().former, [back]);
    assert!(h.state.hosts().candidate.is_none());
    let (st, _) = h.at_host(back, "GET", describe, &[]).await;
    assert_eq!(st, StatusCode::PERMANENT_REDIRECT);
    assert!(h.state.store.pds_host_reserved(back).await.unwrap());
    let (st, v) = h.update_handle(&access, back).await;
    assert_eq!(v["error"], json!("HandleNotAvailable"), "{st} {v}");

    // Confirmed: on the node then serving it, it is current and never
    // former (no redirect to itself), and social.example.com is the old
    // host in full service.
    let (st, v) = prepare().await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let id = h.state.store.open_hostname_migration().await.unwrap().unwrap().id;
    assert!(h.state.store.confirm_hostname_migration(&id, 0).await.unwrap());
    let there = crate::hostnames::Hosts::load(&h.state.store, back)
        .await
        .unwrap();
    assert_eq!(
        there,
        crate::hostnames::Hosts {
            old_full: vec!["social.example.com".into()],
            candidate: None,
            former: vec![],
        }
    );
    assert!(h.state.store.pds_host_reserved("social.example.com").await.unwrap());
}

#[tokio::test]
async fn oauth_and_assertions_across_a_hostname_migration() {
    use locus_auth::oauth::jose::Jws;
    let h = harness(false).await;
    let old = "old.example.net";
    let new = "pds.example.com";
    let (did, _) = h.user("oli.pds.example.com").await;
    let (member, _) = h.user("moe.pds.example.com").await;
    let client = DpopClient::new();
    let tok = h
        .oauth_grant(&client, "http://localhost", "atproto", &did)
        .await;
    let at = tok["access_token"].as_str().unwrap().to_owned();
    // A token as the old host minted it before the switch: the old issuer
    // and the old service DID.
    let jws = Jws::decode(&at).unwrap();
    let mut claims = Value::Object(jws.payload.clone());
    claims["iss"] = json!(format!("https://{old}"));
    claims["aud"] = json!(format!("did:web:{old}"));
    let pre_switch = h.state.secrets.oauth_key.sign(jws.header.clone(), &claims);
    // A token from an issuer that was never this PDS.
    claims["iss"] = json!("https://elsewhere.example.net");
    let foreign = h.state.secrets.oauth_key.sign(jws.header.clone(), &claims);
    h.migrated_from("m1", old).await;
    let session = "/xrpc/com.atproto.server.getSession";

    // Each host's protected-resource metadata names its own origin and only
    // the new issuer; the issuer lists both while a document names the old.
    for host in [new, old] {
        let (_, pr) = h
            .at_host(host, "GET", "/.well-known/oauth-protected-resource", &[])
            .await;
        assert_eq!(pr["resource"], json!(format!("https://{host}")));
        assert_eq!(pr["authorization_servers"], json!([format!("https://{new}")]));
    }
    let (_, asm) = h
        .at_host(new, "GET", "/.well-known/oauth-authorization-server", &[])
        .await;
    assert_eq!(asm["issuer"], json!(format!("https://{new}")));
    assert_eq!(
        asm["protected_resources"],
        json!([format!("https://{new}"), format!("https://{old}")])
    );

    // Post-switch and pre-switch tokens work on both hosts, with DPoP's
    // htu the host each request came to; another host's htu, or another
    // issuer, is refused.
    for token in [&at, &pre_switch] {
        for host in [new, old] {
            let (st, v) = h.oauth_get_at(&client, token, host, host, session).await;
            assert_eq!(st, StatusCode::OK, "{host}: {v}");
            assert_eq!(v["did"], json!(did));
        }
    }
    for (token, host, htu, why) in [
        (&at, old, new, "htu for another host"),
        (&at, new, old, "htu for another host"),
        (&foreign, new, new, "another issuer"),
    ] {
        let (st, v) = h.oauth_get_at(&client, token, host, htu, session).await;
        assert!(st.is_client_error(), "{why}: {st} {v}");
    }

    // Membership assertions for either service DID verify during the window.
    let audience = format!("at://{did}/tools.kryphocron.policy.audience/3m6aaaaaaaaaa");
    let key = h.state.secrets.audience_key.did();
    h.state.kr.membership.seed_issuer_key(new, &key);
    let assertion = |aud: &str| {
        use base64::Engine;
        use proto_blue_crypto::Signer;
        let b64 = |b: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
        let now = locus_core::time::now_ms() / 1000;
        let header = json!({"typ": crate::membership::ASSERTION_TYP, "alg": "ES256K"});
        let claims = json!({"iss": format!("did:web:{new}"), "sub": member, "audienceUri": audience,
            "aud": aud, "iat": now, "exp": now + 120, "jti": "j1"});
        let input = format!(
            "{}.{}",
            b64(header.to_string().as_bytes()),
            b64(claims.to_string().as_bytes())
        );
        let sig = h.state.secrets.audience_key.sign(input.as_bytes()).unwrap();
        format!("{input}.{}", b64(&sig))
    };
    for aud in [new, old] {
        let t = assertion(&format!("did:web:{aud}"));
        assert!(
            crate::membership::verify_assertion(&h.state, &t, &member, &audience).await,
            "aud {aud}"
        );
    }

    // Once no document names the old host: the old issuer's tokens, the
    // old audience and the old resource are gone; the old host redirects.
    h.state
        .store
        .finish_hostname_migration("m1", true)
        .await
        .unwrap();
    h.reload_hosts().await;
    let (st, v) = h.oauth_get_at(&client, &pre_switch, new, new, session).await;
    assert!(st.is_client_error(), "the old issuer: {st} {v}");
    let (st, _) = h.oauth_get_at(&client, &at, new, new, session).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = h.oauth_get_at(&client, &at, old, old, session).await;
    assert_eq!(st, StatusCode::PERMANENT_REDIRECT);
    let t = assertion(&format!("did:web:{old}"));
    assert!(!crate::membership::verify_assertion(&h.state, &t, &member, &audience).await);
    let (_, asm) = h
        .at_host(new, "GET", "/.well-known/oauth-authorization-server", &[])
        .await;
    assert_eq!(asm["protected_resources"], json!([format!("https://{new}")]));
}

#[tokio::test]
async fn hostname_migrations_are_refused_in_the_entryway_and_shard_roles() {
    let ew = harness_build(HarnessOpts {
        entryway_key: Some(proto_blue_crypto::K256Keypair::generate()),
        ..HarnessOpts::default()
    })
    .await;
    let shard = shard_harness(crate::shard::Entryway::for_test(
        "https://entryway.example.net",
        "did:web:entryway.example.net",
        &[proto_blue_crypto::K256Keypair::generate().did()],
        &[proto_blue_crypto::K256Keypair::generate().did()],
    ))
    .await;
    for h in [&ew, &shard] {
        let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
        let issuer = crate::oauth::issuer(&h.state);
        let owner = h.owner_with_password("oto", "oto-password-1234").await;
        let (st, v) = h
            .admin_call(
                "POST",
                &ns("owner.hostname.prepare"),
                &owner,
                Some(json!({"host": "next.example.org"})),
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
        assert!(
            v["message"].as_str().unwrap().contains("standalone role"),
            "{v}"
        );
        assert!(h.state.store.open_hostname_migration().await.unwrap().is_none());
        assert!(!crate::health::approved(&h.state, "next.example.org").await);
        assert_eq!(crate::oauth::issuer(&h.state), issuer);
    }
}
