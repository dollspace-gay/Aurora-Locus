//! End-to-end handler tests on SQLite, with a mock PLC directory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use http_body_util::BodyExt;
use locus_auth::session::LegacySessions;
use locus_core::bootstrap::DatabaseUrl;
use locus_core::keys::{MasterKey, SourceKind};
use locus_identity::plc::SignedOp;
use locus_repo::{RepoEngine, TidClock};
use locus_store::Store;
use locus_store::runtime::Settings;
use proto_blue_crypto::Keypair;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::state::{AppState, ServerSecrets};

/// A PLC directory: each DID's log of ops. With `fail_after_apply` set, it
/// applies an op and then answers 500, like a timeout after the write.
#[derive(Default)]
struct PlcMock {
    log: HashMap<String, Vec<Value>>,
    fail_after_apply: bool,
}

impl PlcMock {
    fn last(&self, did: &str) -> Option<Value> {
        self.log.get(did).and_then(|l| l.last()).cloned()
    }
}

type Plc = Arc<Mutex<PlcMock>>;

async fn mock_plc() -> (String, Plc) {
    let plc: Plc = Arc::default();
    let app = Router::new()
        .route(
            "/{did}",
            post(
                |State(plc): State<Plc>,
                 Path(did): Path<String>,
                 axum::Json(op): axum::Json<Value>| async move {
                    let mut p = plc.lock().unwrap();
                    p.log.entry(did).or_default().push(op);
                    if p.fail_after_apply {
                        StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        StatusCode::OK
                    }
                },
            )
            .get(
                |State(plc): State<Plc>, Path(did): Path<String>| async move {
                    match plc.lock().unwrap().last(&did) {
                        Some(op) if op["type"] != "plc_tombstone" => {
                            (StatusCode::OK, axum::Json(json!({"id": did}))).into_response()
                        }
                        _ => (StatusCode::NOT_FOUND, axum::Json(json!({}))).into_response(),
                    }
                },
            ),
        )
        .route(
            "/{did}/log/last",
            get(
                |State(plc): State<Plc>, Path(did): Path<String>| async move {
                    match plc.lock().unwrap().last(&did) {
                        Some(op) => (StatusCode::OK, axum::Json(op)).into_response(),
                        None => (StatusCode::NOT_FOUND, axum::Json(json!({}))).into_response(),
                    }
                },
            ),
        )
        .with_state(plc.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), plc)
}

struct Harness {
    app: Router,
    state: Arc<AppState>,
    plc: Plc,
    /// Mail the capturing mailer recorded (empty without email).
    mail: Arc<Mutex<Vec<locus_mail::Sent>>>,
    _dir: tempfile::TempDir,
}

async fn harness(invite_required: bool) -> Harness {
    harness_with(invite_required, true).await
}

/// With `email` false, no sender is configured.
async fn harness_with(invite_required: bool, email: bool) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
        .await
        .unwrap();
    let (plc_url, plc) = mock_plc().await;
    store
        .set_setting("identity.plc_url", json!(plc_url), "test")
        .await
        .unwrap();
    store
        .set_setting("accounts.invite_required", json!(invite_required), "test")
        .await
        .unwrap();
    let master_key = Arc::new(MasterKey::generate(SourceKind::Env));
    let secrets = ServerSecrets::load_or_create(&store, &master_key)
        .await
        .unwrap();
    let settings = Settings::load(store.clone()).await.unwrap();
    let hostname = "pds.example.com".to_owned();
    let service_did = format!("did:web:{hostname}");
    let sessions = LegacySessions::new(store.clone(), secrets.jwt.clone(), service_did.clone());
    let (mailer, mail) = if email {
        locus_mail::Mailer::capturing(store.clone(), Arc::clone(&master_key))
    } else {
        (
            locus_mail::Mailer::new(store.clone(), Arc::clone(&master_key)),
            Arc::default(),
        )
    };
    let state = Arc::new(AppState {
        repo: RepoEngine::new(store.clone(), Arc::new(TidClock::new(0))),
        store,
        settings,
        master_key,
        secrets,
        sessions,
        mailer,
        public_url: format!("https://{hostname}"),
        hostname,
        service_did,
        node_id: "test-node".into(),
        ssrf_protection: true,
        resolvers: crate::state::ResolverCache::default(),
    });
    Harness {
        app: crate::router(state.clone()),
        state,
        plc,
        mail,
        _dir: dir,
    }
}

impl Harness {
    async fn call(
        &self,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.call_host(method, path, auth, body, "pds.example.com")
            .await
    }

    async fn call_host(
        &self,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: Option<Value>,
        host: &str,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", host);
        if let Some(t) = auth {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
        (status, v)
    }

    async fn create_account(&self, handle: &str, email: &str, password: &str) -> Value {
        let (st, v) = self
            .call(
                "POST",
                "/xrpc/com.atproto.server.createAccount",
                None,
                Some(json!({"handle": handle, "email": email, "password": password})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v
    }
}

#[tokio::test]
async fn account_creation_registers_a_verifiable_did_and_writes_everything() {
    let h = harness(false).await;
    let (st, desc) = h
        .call("GET", "/xrpc/com.atproto.server.describeServer", None, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(desc["did"], "did:web:pds.example.com");
    assert_eq!(desc["availableUserDomains"], json!([".pds.example.com"]));
    assert_eq!(desc["inviteCodeRequired"], false);

    let acct = h
        .create_account("alice.pds.example.com", "Alice@Example.com", "hunter22")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    assert!(did.starts_with("did:plc:"));
    assert_eq!(acct["handle"], "alice.pds.example.com");

    // The directory got the genesis op that derives this DID, signed by our rotation key.
    let op = h.plc.lock().unwrap().last(&did).expect("op submitted");
    let signed = SignedOp::from_json(&op).unwrap();
    assert_eq!(signed.genesis_did().unwrap(), did);
    let rotation = h.state.secrets.plc_keys.current().did();
    assert!(signed.verify(&rotation));
    assert_eq!(
        h.state.store.plc_key_listing(&did).await.unwrap(),
        Some(vec![rotation])
    );
    assert_eq!(op["alsoKnownAs"], json!(["at://alice.pds.example.com"]));
    assert_eq!(
        op["services"]["atproto_pds"]["endpoint"],
        "https://pds.example.com"
    );

    // Account, repo and events were written; the email was lowercased.
    let account = h.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!(account.email.as_deref(), Some("alice@example.com"));
    let kinds: Vec<String> = h
        .state
        .store
        .outbox_for(&did)
        .await
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(kinds, vec!["identity", "account", "commit", "sync"]);

    // The returned session works.
    let (st, sess) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            acct["accessJwt"].as_str(),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{sess}");
    assert_eq!(sess["did"], did.as_str());
    assert_eq!(sess["active"], true);
}

#[tokio::test]
async fn sessions_login_refresh_and_logout() {
    let h = harness(false).await;
    let acct = h
        .create_account("bob.pds.example.com", "bob@example.com", "pw-bob-123")
        .await;

    // Wrong password, unknown user: the same 401.
    for (id, pw) in [
        ("bob.pds.example.com", "nope"),
        ("nobody.pds.example.com", "pw"),
    ] {
        let (st, v) = h
            .call(
                "POST",
                "/xrpc/com.atproto.server.createSession",
                None,
                Some(json!({"identifier": id, "password": pw})),
            )
            .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "{v}");
        assert_eq!(v["error"], "AuthenticationRequired");
    }
    // Log in by handle and by email.
    for id in [
        "bob.pds.example.com",
        "BOB@example.com",
        acct["did"].as_str().unwrap(),
    ] {
        let (st, v) = h
            .call(
                "POST",
                "/xrpc/com.atproto.server.createSession",
                None,
                Some(json!({"identifier": id, "password": "pw-bob-123"})),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{id}: {v}");
    }
    let (_, login) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createSession",
            None,
            Some(json!({"identifier": "bob.pds.example.com", "password": "pw-bob-123"})),
        )
        .await;
    let (st, refreshed) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.refreshSession",
            login["refreshJwt"].as_str(),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{refreshed}");
    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.deleteSession",
            refreshed["refreshJwt"].as_str(),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            refreshed["accessJwt"].as_str(),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    // No header and a wrong scheme get the reference's errors.
    let (st, v) = h
        .call("GET", "/xrpc/com.atproto.server.getSession", None, None)
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("AuthMissing"))
    );
}

#[tokio::test]
async fn app_passwords_log_in_with_their_scope_and_revoke() {
    let h = harness(false).await;
    let acct = h
        .create_account("carol.pds.example.com", "carol@example.com", "pw-carol-1")
        .await;
    let access = acct["accessJwt"].as_str().unwrap();
    let (st, ap) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAppPassword",
            Some(access),
            Some(json!({"name": "bot"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{ap}");
    let pw = ap["password"].as_str().unwrap().to_owned();

    let (st, login) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createSession",
            None,
            Some(json!({"identifier": "carol.pds.example.com", "password": pw})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{login}");
    let app_access = login["accessJwt"].as_str().unwrap().to_owned();
    // An app-password session can't make app passwords (full access only).
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAppPassword",
            Some(&app_access),
            Some(json!({"name": "x"})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");

    let (_, list) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.listAppPasswords",
            Some(access),
            None,
        )
        .await;
    assert_eq!(list["passwords"][0]["name"], "bot");
    assert!(
        list["passwords"][0].get("password").is_none(),
        "never listed"
    );

    let (st, _) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.revokeAppPassword",
            Some(access),
            Some(json!({"name": "bot"})),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&app_access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "revoked on the next request");
}

#[tokio::test]
async fn account_creation_refusals() {
    let h = harness(true).await;
    let base =
        |handle: &str, email: &str| json!({"handle": handle, "email": email, "password": "pw"});
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(base("dave.pds.example.com", "d@example.com")),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidInviteCode"))
    );

    let h = harness(false).await;
    h.create_account("erin.pds.example.com", "erin@example.com", "pw")
        .await;
    for (body, error) in [
        (
            base("erin.pds.example.com", "other@example.com"),
            "HandleNotAvailable",
        ),
        (
            base("frank.pds.example.com", "ERIN@example.com"),
            "InvalidRequest",
        ),
        (
            base("admin.pds.example.com", "a@example.com"),
            "HandleNotAvailable",
        ),
        (
            base("frank.elsewhere.com", "f@example.com"),
            "UnsupportedDomain",
        ),
        (base("fr.pds.example.com", "f@example.com"), "InvalidHandle"),
        (
            base("frank.pds.example.com", "not-an-email"),
            "InvalidRequest",
        ),
    ] {
        let (st, v) = h
            .call(
                "POST",
                "/xrpc/com.atproto.server.createAccount",
                None,
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some(error)),
            "{body}: {v}"
        );
    }
}

#[tokio::test]
async fn handles_resolve_and_documents_are_served_by_host() {
    let h = harness(false).await;
    let acct = h
        .create_account("gina.pds.example.com", "gina@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap();

    let (st, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.resolveHandle?handle=Gina.pds.example.com",
            None,
            None,
        )
        .await;
    assert_eq!((st, v["did"].as_str()), (StatusCode::OK, Some(did)));
    let (st, v) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.resolveHandle?handle=nobody.pds.example.com",
            None,
            None,
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("HandleNotFound"))
    );

    let (st, v) = h
        .call_host(
            "GET",
            "/.well-known/atproto-did",
            None,
            None,
            "gina.pds.example.com",
        )
        .await;
    assert_eq!((st, v.as_str()), (StatusCode::OK, Some(did)));
    let (st, _) = h
        .call_host(
            "GET",
            "/.well-known/atproto-did",
            None,
            None,
            "nobody.pds.example.com",
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, doc) = h.call("GET", "/.well-known/did.json", None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(doc["id"], "did:web:pds.example.com");
    assert_eq!(
        doc["service"][0]["serviceEndpoint"],
        "https://pds.example.com"
    );
    let (st, _) = h
        .call_host(
            "GET",
            "/.well-known/did.json",
            None,
            None,
            "gina.pds.example.com",
        )
        .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "not on a handle host");
}

impl Harness {
    async fn outbox_kinds(&self, did: &str) -> Vec<String> {
        self.state
            .store
            .outbox_for(did)
            .await
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect()
    }

    async fn update_handle(&self, access: &str, handle: &str) -> (StatusCode, Value) {
        self.call(
            "POST",
            "/xrpc/com.atproto.identity.updateHandle",
            Some(access),
            Some(json!({ "handle": handle })),
        )
        .await
    }
}

#[tokio::test]
async fn plc_handle_changes_submit_an_op_then_update_the_account() {
    let h = harness(false).await;
    let acct = h
        .create_account("hana.pds.example.com", "hana@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let genesis = h.plc.lock().unwrap().last(&did).unwrap();

    let (st, v) = h.update_handle(&access, "Hana2.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // The op chains to the genesis, names the new handle and verifies.
    let op = h.plc.lock().unwrap().last(&did).unwrap();
    let genesis_cid = SignedOp::from_json(&genesis).unwrap().cid().unwrap();
    assert_eq!(op["prev"], genesis_cid.to_string_base32());
    assert_eq!(op["alsoKnownAs"], json!(["at://hana2.pds.example.com"]));
    let signed = SignedOp::from_json(&op).unwrap();
    assert!(signed.verify(&h.state.secrets.plc_keys.current().did()));

    // The account, events and lookups follow; nothing is left to repair.
    let a = h.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!(a.handle, "hana2.pds.example.com");
    assert_eq!(h.outbox_kinds(&did).await.last().unwrap(), "identity");
    assert!(
        h.state
            .store
            .due_identity_repairs(10)
            .await
            .unwrap()
            .is_empty()
    );
    let (st, _) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.resolveHandle?handle=hana.pds.example.com",
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the old handle is free");

    // Another account can't take it; nobody can take a non-service handle
    // that doesn't resolve to them.
    let other = h
        .create_account("ivan.pds.example.com", "ivan@example.com", "pw")
        .await;
    let (st, v) = h
        .update_handle(
            other["accessJwt"].as_str().unwrap(),
            "hana2.pds.example.com",
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("HandleNotAvailable"))
    );
    let (st, v) = h.update_handle(&access, "pds.example.com").await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("HandleNotAvailable")),
        "{v}"
    );
}

#[tokio::test]
async fn an_unclear_plc_answer_is_repaired_later() {
    let h = harness(false).await;
    let acct = h
        .create_account("jun.pds.example.com", "jun@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    h.plc.lock().unwrap().fail_after_apply = true;
    let (st, v) = h
        .update_handle(acct["accessJwt"].as_str().unwrap(), "jun2.pds.example.com")
        .await;
    // The op applied but the answer was a 500: the follow-up read shows the
    // handle, so the change goes through.
    assert_eq!(st, StatusCode::OK, "{v}");
    let a = h.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!(a.handle, "jun2.pds.example.com");

    // A change PLC really didn't take stays queued, then converges once
    // the directory answers.
    h.plc.lock().unwrap().fail_after_apply = false;
    h.state
        .store
        .queue_identity_repair(
            &did,
            locus_store::identity::RepairKind::Handle,
            "jun3.pds.example.com",
        )
        .await
        .unwrap();
    assert_eq!(
        crate::identity::run_identity_repairs(&h.state)
            .await
            .unwrap(),
        1
    );
    let a = h.state.store.account_by_did(&did).await.unwrap().unwrap();
    assert_eq!(a.handle, "jun3.pds.example.com");
    let op = h.plc.lock().unwrap().last(&did).unwrap();
    assert_eq!(op["alsoKnownAs"], json!(["at://jun3.pds.example.com"]));
    assert!(
        h.state
            .store
            .due_identity_repairs(10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_repair_job_tombstones_orphaned_dids_but_never_live_accounts() {
    use locus_identity::plc::{Operation, Service};
    use std::collections::BTreeMap;
    let h = harness(false).await;
    // A DID registered for a sign-up that never finished.
    let op = Operation {
        rotation_keys: vec![h.state.secrets.plc_keys.current().did()],
        verification_methods: BTreeMap::from([(
            "atproto".to_owned(),
            proto_blue_crypto::K256Keypair::generate().did(),
        )]),
        also_known_as: vec!["at://orphan.pds.example.com".into()],
        services: BTreeMap::from([(
            "atproto_pds".to_owned(),
            Service {
                kind: "AtprotoPersonalDataServer".into(),
                endpoint: "https://pds.example.com".into(),
            },
        )]),
        prev: None,
    };
    let genesis = op.sign(h.state.secrets.plc_keys.current()).unwrap();
    let orphan = genesis.genesis_did().unwrap();
    h.plc
        .lock()
        .unwrap()
        .log
        .insert(orphan.clone(), vec![genesis.to_json()]);
    let live = h
        .create_account("kim.pds.example.com", "kim@example.com", "pw")
        .await["did"]
        .as_str()
        .unwrap()
        .to_owned();
    for did in [&orphan, &live] {
        h.state
            .store
            .queue_identity_repair(did, locus_store::identity::RepairKind::Tombstone, did)
            .await
            .unwrap();
    }
    assert_eq!(
        crate::identity::run_identity_repairs(&h.state)
            .await
            .unwrap(),
        2
    );
    let tomb = h.plc.lock().unwrap().last(&orphan).unwrap();
    assert_eq!(tomb["type"], "plc_tombstone");
    assert_eq!(tomb["prev"], genesis.cid().unwrap().to_string_base32());
    assert!(
        SignedOp::from_json(&tomb)
            .unwrap()
            .verify(&h.state.secrets.plc_keys.current().did())
    );
    let live_last = h.plc.lock().unwrap().last(&live).unwrap();
    assert_eq!(
        live_last["type"], "plc_operation",
        "the live account is untouched"
    );
}

#[tokio::test]
async fn did_web_accounts_are_served_here_and_their_hosts_stay_reserved() {
    let h = harness(false).await;
    let (st, acct) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(
                json!({"handle": "wendy.pds.example.com", "email": "wendy@example.com",
                "password": "pw", "locusDidMethod": "web"}),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{acct}");
    assert_eq!(acct["did"], "did:web:wendy.pds.example.com");
    assert!(h.plc.lock().unwrap().log.is_empty(), "no PLC involved");
    assert_eq!(
        h.outbox_kinds("did:web:wendy.pds.example.com").await,
        vec!["identity", "account", "commit", "sync"]
    );

    // The document is served on the account's host.
    let (st, doc) = h
        .call_host(
            "GET",
            "/.well-known/did.json",
            None,
            None,
            "wendy.pds.example.com",
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{doc}");
    assert_eq!(doc["id"], "did:web:wendy.pds.example.com");
    assert_eq!(doc["alsoKnownAs"], json!(["at://wendy.pds.example.com"]));
    assert_eq!(
        doc["service"][0]["serviceEndpoint"],
        "https://pds.example.com"
    );

    // A handle change edits the served document at once.
    let access = acct["accessJwt"].as_str().unwrap();
    let (st, v) = h.update_handle(access, "wendy2.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, doc) = h
        .call_host(
            "GET",
            "/.well-known/did.json",
            None,
            None,
            "wendy.pds.example.com",
        )
        .await;
    assert_eq!(doc["alsoKnownAs"], json!(["at://wendy2.pds.example.com"]));

    // The old host stays tied to the DID: nobody else can take it as a
    // handle, though the account itself can take it back.
    let (st, v) = h
        .call(
            "POST",
            "/xrpc/com.atproto.server.createAccount",
            None,
            Some(json!({"handle": "wendy.pds.example.com", "email": "other@example.com", "password": "pw"})),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("HandleNotAvailable")),
        "{v}"
    );
    let (st, v) = h.update_handle(access, "wendy.pds.example.com").await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

impl Harness {
    /// The token in the last email sent to `to`, and its subject.
    fn last_mail(&self, to: &str) -> (String, String) {
        let mail = self.mail.lock().unwrap();
        let m = mail
            .iter()
            .rev()
            .find(|m| m.to == to)
            .expect("an email was sent");
        let token = m
            .email
            .text
            .lines()
            .map(str::trim)
            .find(|l| l.len() == 11 && l.as_bytes()[5] == b'-')
            .expect("the email holds a token")
            .to_owned();
        (m.email.subject.clone(), token)
    }

    fn mail_count(&self) -> usize {
        self.mail.lock().unwrap().len()
    }

    async fn post(&self, path: &str, auth: Option<&str>, body: Value) -> (StatusCode, Value) {
        self.call("POST", &format!("/xrpc/{path}"), auth, Some(body))
            .await
    }
}

#[tokio::test]
async fn email_confirmation_and_change() {
    let h = harness(false).await;
    let acct = h
        .create_account("lea.pds.example.com", "lea@example.com", "pw")
        .await;
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let (st, v) = h
        .post(
            "com.atproto.server.requestEmailConfirmation",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (subject, token) = h.last_mail("lea@example.com");
    assert_eq!(subject, "Email Confirmation");

    let (st, v) = h
        .post(
            "com.atproto.server.confirmEmail",
            Some(&access),
            json!({"email": "lea@example.com", "token": "AAAAA-BBBBB"}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidToken"))
    );
    let (st, v) = h
        .post(
            "com.atproto.server.confirmEmail",
            Some(&access),
            json!({"email": "other@example.com", "token": token}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidEmail"))
    );
    let (st, v) = h
        .post(
            "com.atproto.server.confirmEmail",
            Some(&access),
            json!({"email": "LEA@example.com", "token": token.to_lowercase()}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, sess) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(sess["emailConfirmed"], true);
    // A token works once.
    let (st, _) = h
        .post(
            "com.atproto.server.confirmEmail",
            Some(&access),
            json!({"email": "lea@example.com", "token": token}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A confirmed address must approve a change.
    let (st, v) = h
        .post(
            "com.atproto.server.requestEmailUpdate",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(
        (st, v["tokenRequired"].as_bool()),
        (StatusCode::OK, Some(true)),
        "{v}"
    );
    let (subject, token) = h.last_mail("lea@example.com");
    assert_eq!(subject, "Email Update Requested");
    let (st, v) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "lea2@example.com"}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("TokenRequired"))
    );
    h.create_account("max.pds.example.com", "max@example.com", "pw")
        .await;
    let (st, v) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "max@example.com", "token": token}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "taken: {v}");
    let (st, v) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "lea2@example.com", "token": token}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, sess) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(
        (sess["email"].as_str(), sess["emailConfirmed"].as_bool()),
        (Some("lea2@example.com"), Some(false))
    );
    // An unconfirmed address changes without a token.
    let (st, v) = h
        .post(
            "com.atproto.server.requestEmailUpdate",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(
        (st, v["tokenRequired"].as_bool()),
        (StatusCode::OK, Some(false))
    );
    let (st, _) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "lea3@example.com"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn password_reset_by_email() {
    let h = harness(false).await;
    let acct = h
        .create_account("nia.pds.example.com", "nia@example.com", "old-pw")
        .await;
    let (st, _) = h
        .post(
            "com.atproto.server.requestPasswordReset",
            None,
            json!({"email": "nobody@example.com"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "same answer for an unknown address");
    assert_eq!(h.mail_count(), 0);
    let (st, _) = h
        .post(
            "com.atproto.server.requestPasswordReset",
            None,
            json!({"email": "NIA@example.com"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (subject, token) = h.last_mail("nia@example.com");
    assert_eq!(subject, "Password Reset Requested");

    let (st, v) = h
        .post(
            "com.atproto.server.resetPassword",
            None,
            json!({"token": token, "password": "new-pw"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let login = |pw: &'static str| {
        h.post(
            "com.atproto.server.createSession",
            None,
            json!({"identifier": "nia.pds.example.com", "password": pw}),
        )
    };
    assert_eq!(login("old-pw").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(login("new-pw").await.0, StatusCode::OK);
    let (st, _) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            acct["accessJwt"].as_str(),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "old sessions end");
    let (st, v) = h
        .post(
            "com.atproto.server.resetPassword",
            None,
            json!({"token": token, "password": "again"}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidToken"))
    );
}

#[tokio::test]
async fn sign_in_codes_by_email() {
    let h = harness(false).await;
    let acct = h
        .create_account("ola.pds.example.com", "ola@example.com", "pw")
        .await;
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    // The factor needs a confirmed address.
    let (st, v) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "ola@example.com", "emailAuthFactor": true}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    h.post(
        "com.atproto.server.requestEmailConfirmation",
        Some(&access),
        json!({}),
    )
    .await;
    let (_, token) = h.last_mail("ola@example.com");
    h.post(
        "com.atproto.server.confirmEmail",
        Some(&access),
        json!({"email": "ola@example.com", "token": token}),
    )
    .await;
    let (st, v) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "ola@example.com", "emailAuthFactor": true}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, ap) = h
        .post(
            "com.atproto.server.createAppPassword",
            Some(&access),
            json!({"name": "bot"}),
        )
        .await;

    let login = |body: Value| h.post("com.atproto.server.createSession", None, body);
    let (st, v) = login(json!({"identifier": "ola.pds.example.com", "password": "pw"})).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("AuthFactorTokenRequired")),
        "{v}"
    );
    let (subject, code) = h.last_mail("ola@example.com");
    assert_eq!(subject, "Sign-in Confirmation");
    let (st, v) = login(
        json!({"identifier": "ola.pds.example.com", "password": "pw", "authFactorToken": code}),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["emailAuthFactor"], true);
    // App passwords skip the factor, as the reference.
    let (st, _) =
        login(json!({"identifier": "ola.pds.example.com", "password": ap["password"]})).await;
    assert_eq!(st, StatusCode::OK);

    // Turning it off takes a code from the inbox.
    let (st, v) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "ola@example.com", "emailAuthFactor": false}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("TokenRequired"))
    );
    let (_, token) = h.last_mail("ola@example.com");
    let (st, _) = h
        .post(
            "com.atproto.server.updateEmail",
            Some(&access),
            json!({"email": "ola@example.com", "emailAuthFactor": false, "token": token}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = login(json!({"identifier": "ola.pds.example.com", "password": "pw"})).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn without_email_the_features_say_so() {
    let h = harness_with(false, false).await;
    let acct = h
        .create_account("pia.pds.example.com", "pia@example.com", "pw")
        .await;
    for (path, auth, body) in [
        (
            "com.atproto.server.requestEmailConfirmation",
            acct["accessJwt"].as_str(),
            json!({}),
        ),
        (
            "com.atproto.server.requestPasswordReset",
            None,
            json!({"email": "pia@example.com"}),
        ),
    ] {
        let (st, v) = h.post(path, auth, body).await;
        assert_eq!(
            (st, v["message"].as_str()),
            (
                StatusCode::BAD_REQUEST,
                Some("Email is not configured on this server")
            ),
            "{path}"
        );
    }
}

#[tokio::test]
async fn plc_operations_are_recommended_signed_with_a_code_and_submitted() {
    let h = harness(false).await;
    let acct = h
        .create_account("quinn.pds.example.com", "quinn@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let current = h.state.secrets.plc_keys.current().did();
    let genesis = h.plc.lock().unwrap().last(&did).unwrap();

    let (st, rec) = h
        .call(
            "GET",
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{rec}");
    assert_eq!(rec["rotationKeys"], json!([current]));
    assert_eq!(rec["verificationMethods"], genesis["verificationMethods"]);
    assert_eq!(rec["alsoKnownAs"], json!(["at://quinn.pds.example.com"]));
    assert_eq!(
        rec["services"]["atproto_pds"]["endpoint"],
        "https://pds.example.com"
    );

    // Signing needs the emailed code, which works once.
    let (st, v) = h
        .post(
            "com.atproto.identity.signPlcOperation",
            Some(&access),
            json!({"rotationKeys": [current]}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    h.post(
        "com.atproto.identity.requestPlcOperationSignature",
        Some(&access),
        json!({}),
    )
    .await;
    let (subject, code) = h.last_mail("quinn@example.com");
    assert_eq!(subject, "PLC Update Operation Requested");
    let extra = proto_blue_crypto::K256Keypair::generate().did();
    let (st, signed) = h
        .post(
            "com.atproto.identity.signPlcOperation",
            Some(&access),
            json!({"token": code, "rotationKeys": [extra, current]}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{signed}");
    let op = &signed["operation"];
    assert_eq!(op["rotationKeys"], json!([extra, current]));
    assert_eq!(
        op["alsoKnownAs"], genesis["alsoKnownAs"],
        "unnamed fields carry over"
    );
    let genesis_cid = SignedOp::from_json(&genesis)
        .unwrap()
        .cid()
        .unwrap()
        .to_string_base32();
    assert_eq!(op["prev"], genesis_cid);
    assert!(SignedOp::from_json(op).unwrap().verify(&current));
    assert_eq!(
        h.plc.lock().unwrap().log[&did].len(),
        1,
        "signed, not submitted"
    );
    let (st, _) = h
        .post(
            "com.atproto.identity.signPlcOperation",
            Some(&access),
            json!({"token": code}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "the code is spent");

    // Bad fields are refused before signing (locus validates them).
    h.post(
        "com.atproto.identity.requestPlcOperationSignature",
        Some(&access),
        json!({}),
    )
    .await;
    let (_, code) = h.last_mail("quinn@example.com");
    let (st, v) = h
        .post(
            "com.atproto.identity.signPlcOperation",
            Some(&access),
            json!({"token": code, "rotationKeys": ["not-a-key"]}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");

    // Submitting checks the op keeps the account here.
    let (st, v) = h
        .post(
            "com.atproto.identity.submitPlcOperation",
            Some(&access),
            json!({"operation": op}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(h.plc.lock().unwrap().log[&did].len(), 2);
    assert_eq!(h.outbox_kinds(&did).await.last().unwrap(), "identity");
    assert_eq!(
        h.state.store.plc_key_listing(&did).await.unwrap(),
        Some(vec![current.clone()])
    );
    let mut moved = op.clone();
    moved["services"]["atproto_pds"]["endpoint"] = json!("https://elsewhere.example.net");
    let (st, v) = h
        .post(
            "com.atproto.identity.submitPlcOperation",
            Some(&access),
            json!({"operation": moved}),
        )
        .await;
    assert_eq!(
        (st, v["message"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some("Incorrect endpoint on atproto_pds service")
        )
    );
    let mut keyless = op.clone();
    keyless["rotationKeys"] = json!([extra]);
    let (st, v) = h
        .post(
            "com.atproto.identity.submitPlcOperation",
            Some(&access),
            json!({"operation": keyless}),
        )
        .await;
    assert_eq!(
        (st, v["message"].as_str()),
        (
            StatusCode::BAD_REQUEST,
            Some("Rotation keys do not include server's rotation key")
        )
    );
}

#[tokio::test]
async fn deactivation_and_activation_check_the_did_document() {
    let h = harness(false).await;
    let acct = h
        .create_account("rae.pds.example.com", "rae@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let access = acct["accessJwt"].as_str().unwrap().to_owned();

    let (st, status) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.checkAccountStatus",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{status}");
    assert_eq!(
        (status["activated"].as_bool(), status["validDid"].as_bool()),
        (Some(true), Some(true))
    );
    assert!(status["repoBlocks"].as_i64().unwrap() >= 2, "{status}");
    assert_eq!(status["indexedRecords"], 0);

    let (st, v) = h
        .post(
            "com.atproto.server.deactivateAccount",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (_, sess) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(
        (sess["active"].as_bool(), sess["status"].as_str()),
        (Some(false), Some("deactivated"))
    );
    assert_eq!(h.outbox_kinds(&did).await.last().unwrap(), "account");

    // The document must point here to come back.
    let genesis = h.plc.lock().unwrap().last(&did).unwrap();
    let mut moved = SignedOp::from_json(&genesis).unwrap();
    if let SignedOp::Operation { op, .. } = &mut moved {
        op.services.get_mut("atproto_pds").unwrap().endpoint =
            "https://elsewhere.example.net".into();
        op.prev = Some(
            SignedOp::from_json(&genesis)
                .unwrap()
                .cid()
                .unwrap()
                .to_string_base32(),
        );
    }
    let SignedOp::Operation { op, .. } = &moved else {
        unreachable!()
    };
    let moved = op.sign(h.state.secrets.plc_keys.current()).unwrap();
    h.plc
        .lock()
        .unwrap()
        .log
        .get_mut(&did)
        .unwrap()
        .push(moved.to_json());
    let (st, v) = h
        .post(
            "com.atproto.server.activateAccount",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains("endpoint"), "{v}");
    let (_, status) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.checkAccountStatus",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(
        (status["activated"].as_bool(), status["validDid"].as_bool()),
        (Some(false), Some(false))
    );

    // Point it back; activation emits #account, #identity and #sync.
    h.plc.lock().unwrap().log.get_mut(&did).unwrap().pop();
    let (st, v) = h
        .post(
            "com.atproto.server.activateAccount",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let kinds = h.outbox_kinds(&did).await;
    assert_eq!(&kinds[kinds.len() - 3..], ["account", "identity", "sync"]);
    let (_, sess) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_eq!(sess["active"], true);
}

#[tokio::test]
async fn account_deletion_runs_to_the_end_and_frees_the_handle() {
    let h = harness(false).await;
    let acct = h
        .create_account("sam.pds.example.com", "sam@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let (st, _) = h
        .post(
            "com.atproto.server.requestAccountDelete",
            Some(&access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (subject, token) = h.last_mail("sam@example.com");
    assert_eq!(subject, "Account Deletion Requested");

    let (st, _) = h
        .post(
            "com.atproto.server.deleteAccount",
            None,
            json!({"did": did, "password": "nope", "token": token}),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, v) = h
        .post(
            "com.atproto.server.deleteAccount",
            None,
            json!({"did": did, "password": "pw", "token": token}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    // Gone at once for logins and tokens; the rest goes in the background.
    let (st, _) = h
        .call(
            "GET",
            "/xrpc/com.atproto.server.getSession",
            Some(&access),
            None,
        )
        .await;
    assert_ne!(st, StatusCode::OK);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while h.state.store.account_by_did(&did).await.unwrap().is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "deletion never finished"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(h.state.store.repo_stats(&did).await.unwrap().is_none());
    let kinds = h.outbox_kinds(&did).await;
    assert_eq!(
        kinds.last().unwrap(),
        "account",
        "#account deleted was emitted"
    );
    h.create_account("sam.pds.example.com", "sam@example.com", "pw2")
        .await;
}

#[tokio::test]
async fn delete_after_is_enforced() {
    let h = harness(false).await;
    let acct = h
        .create_account("tia.pds.example.com", "tia@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let (st, v) = h
        .post(
            "com.atproto.server.deactivateAccount",
            acct["accessJwt"].as_str(),
            json!({"deleteAfter": "2000-01-01T00:00:00Z"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let other = h
        .create_account("uma.pds.example.com", "uma@example.com", "pw")
        .await;
    assert_eq!(
        crate::account::delete_due_accounts(&h.state).await.unwrap(),
        1
    );
    assert!(h.state.store.account_by_did(&did).await.unwrap().is_none());
    assert!(
        h.state
            .store
            .account_by_did(other["did"].as_str().unwrap())
            .await
            .unwrap()
            .is_some()
    );
}
