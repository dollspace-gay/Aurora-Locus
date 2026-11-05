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
                            (StatusCode::OK, axum::Json(plc_document(&did, &op))).into_response()
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

/// The DID document the directory serves for an op's state.
fn plc_document(did: &str, op: &Value) -> Value {
    let key = op["verificationMethods"]["atproto"]
        .as_str()
        .unwrap_or_default();
    json!({
        "id": did,
        "alsoKnownAs": op["alsoKnownAs"],
        "verificationMethod": [{
            "id": format!("{did}#atproto"),
            "type": "Multikey",
            "controller": did,
            "publicKeyMultibase": key.strip_prefix("did:key:").unwrap_or_default(),
        }],
        "service": [{
            "id": "#atproto_pds",
            "type": "AtprotoPersonalDataServer",
            "serviceEndpoint": op["services"]["atproto_pds"]["endpoint"],
        }],
    })
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
    harness_at("pds.example.com", invite_required, email).await
}

/// A PDS at `hostname`.
async fn harness_at(hostname: &str, invite_required: bool, email: bool) -> Harness {
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
    let hostname = hostname.to_owned();
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
        blobs: locus_blob::DiskStore::new(dir.path().join("blobs")),
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
        let host = self.state.hostname.clone();
        self.call_host(method, path, auth, body, &host).await
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

#[tokio::test]
async fn an_account_migrates_in_with_service_auth_and_activates_once_its_did_points_here() {
    let old = harness_at("pds.example.com", false, true).await;
    let new = harness_at("pds2.example.com", false, true).await;
    let acct = old
        .create_account("vic.pds.example.com", "vic@example.com", "pw")
        .await;
    let did = acct["did"].as_str().unwrap().to_owned();
    let old_access = acct["accessJwt"].as_str().unwrap().to_owned();
    // The new PDS reads the same directory.
    let log = old.plc.lock().unwrap().log[&did].clone();
    new.plc.lock().unwrap().log.insert(did.clone(), log);

    let (st, v) = old
        .service_token(
            &old_access,
            "did:web:pds2.example.com",
            "com.atproto.server.createAccount",
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let token = v["token"].as_str().unwrap().to_owned();
    let (st, v) = old
        .service_token(
            &old_access,
            "did:web:pds2.example.com",
            "com.atproto.server.getSession",
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "protected method: {v}");

    let body = json!({"did": did, "handle": "vic.pds2.example.com", "email": "vic@example.com", "password": "pw"});
    let (st, v) = new
        .post("com.atproto.server.createAccount", None, body.clone())
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "no auth: {v}");
    let (_, wrong_aud) = old
        .service_token(
            &old_access,
            "did:web:pds.example.com",
            "com.atproto.server.createAccount",
        )
        .await;
    let (st, v) = new
        .post(
            "com.atproto.server.createAccount",
            wrong_aud["token"].as_str(),
            body.clone(),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("BadJwtAudience")),
        "{v}"
    );
    let (st, created) = new
        .post("com.atproto.server.createAccount", Some(&token), body)
        .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    let new_access = created["accessJwt"].as_str().unwrap().to_owned();
    assert!(
        new.outbox_kinds(&did).await.is_empty(),
        "nothing emitted while migrating"
    );
    let (_, status) = new
        .call(
            "GET",
            "/xrpc/com.atproto.server.checkAccountStatus",
            Some(&new_access),
            None,
        )
        .await;
    assert_eq!(
        (status["activated"].as_bool(), status["validDid"].as_bool()),
        (Some(false), Some(false))
    );
    let (st, _) = new
        .post(
            "com.atproto.server.activateAccount",
            Some(&new_access),
            json!({}),
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "the DID still points at the old PDS"
    );

    // Move the DID: the new PDS recommends, the old one signs, the new one submits.
    let (_, rec) = new
        .call(
            "GET",
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            Some(&new_access),
            None,
        )
        .await;
    old.post(
        "com.atproto.identity.requestPlcOperationSignature",
        Some(&old_access),
        json!({}),
    )
    .await;
    let (_, code) = old.last_mail("vic@example.com");
    let mut sign = rec.clone();
    sign["token"] = json!(code);
    let (st, signed) = old
        .post(
            "com.atproto.identity.signPlcOperation",
            Some(&old_access),
            sign,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{signed}");
    // The op names the new PDS's key; the old PDS signed it with its own.
    let (st, v) = new
        .post(
            "com.atproto.identity.submitPlcOperation",
            Some(&new_access),
            json!({"operation": signed["operation"]}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");

    let (st, v) = new
        .post(
            "com.atproto.server.activateAccount",
            Some(&new_access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(
        new.outbox_kinds(&did).await,
        vec!["account", "identity", "sync"]
    );
}

impl Harness {
    async fn service_token(&self, access: &str, aud: &str, lxm: &str) -> (StatusCode, Value) {
        self.call(
            "GET",
            &format!("/xrpc/com.atproto.server.getServiceAuth?aud={aud}&lxm={lxm}"),
            Some(access),
            None,
        )
        .await
    }
}

#[tokio::test]
async fn accounts_earn_invite_codes_on_an_interval() {
    let h = harness(true).await;
    let mut tx = h.state.store.begin().await.unwrap();
    tx.insert_invite_code("seed-code", 1, "admin", "admin")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    h.state
        .settings
        .set("accounts.invite_interval_ms", json!(1), "test")
        .await
        .unwrap();
    let (st, acct) = h
        .post(
            "com.atproto.server.createAccount",
            None,
            json!({"handle": "wes.pds.example.com", "email": "wes@example.com", "password": "pw", "inviteCode": "seed-code"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{acct}");
    let access = acct["accessJwt"].as_str().unwrap().to_owned();
    let list = |used: bool| {
        let access = access.clone();
        let h = &h;
        async move {
            let (st, v) = h
                .call(
                    "GET",
                    &format!("/xrpc/com.atproto.server.getAccountInviteCodes?includeUsed={used}"),
                    Some(&access),
                    None,
                )
                .await;
            assert_eq!(st, StatusCode::OK, "{v}");
            v["codes"].as_array().unwrap().clone()
        }
    };
    let codes = list(true).await;
    assert_eq!(codes.len(), 5, "capped at 5 unused");
    assert!(
        codes[0]["code"]
            .as_str()
            .unwrap()
            .starts_with("pds-example-com-")
    );
    assert_eq!(list(true).await.len(), 5, "no more while 5 are unused");

    // Using one makes room for another.
    let code = codes[0]["code"].as_str().unwrap().to_owned();
    let (st, v) = h
        .post(
            "com.atproto.server.createAccount",
            None,
            json!({"handle": "xan.pds.example.com", "email": "xan@example.com", "password": "pw", "inviteCode": code}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let all = list(true).await;
    assert_eq!(all.len(), 6);
    let used = all.iter().find(|c| c["code"] == code.as_str()).unwrap();
    assert_eq!(used["uses"][0]["usedBy"], v["did"]);
    assert_eq!(
        list(false).await.len(),
        5,
        "used codes hidden without includeUsed"
    );
}

#[tokio::test]
async fn routes_depend_on_the_host() {
    let h = harness(false).await;
    h.create_account("yan.pds.example.com", "yan@example.com", "pw")
        .await;
    let describe = "/xrpc/com.atproto.server.describeServer";
    for (host, expected) in [
        ("pds.example.com", StatusCode::OK),
        ("localhost", StatusCode::OK),
        ("127.0.0.1", StatusCode::OK),
        ("locus", StatusCode::OK),
        ("yan.pds.example.com", StatusCode::NOT_FOUND),
        ("evil.example.net", StatusCode::NOT_FOUND),
    ] {
        let (st, _) = h.call_host("GET", describe, None, None, host).await;
        assert_eq!(st, expected, "{host}");
    }
    // A handle host gets its identity documents; health answers anywhere.
    let (st, _) = h
        .call_host(
            "GET",
            "/.well-known/atproto-did",
            None,
            None,
            "yan.pds.example.com",
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = h
        .call_host("GET", "/xrpc/_health", None, None, "evil.example.net")
        .await;
    assert_eq!(st, StatusCode::OK);
    // Listed internal names get everything.
    h.state
        .settings
        .set("http.internal_hosts", json!(["pds.internal.lan"]), "test")
        .await
        .unwrap();
    let (st, _) = h
        .call_host("GET", describe, None, None, "pds.internal.lan")
        .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn disposable_addresses_are_refused() {
    let h = harness(false).await;
    let create = |handle: &'static str, email: &'static str| {
        h.post(
            "com.atproto.server.createAccount",
            None,
            json!({"handle": handle, "email": email, "password": "pw"}),
        )
    };
    let (st, v) = create("zed.pds.example.com", "zed@0-mail.com").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    assert!(v["message"].as_str().unwrap().contains("not supported"));
    h.state
        .settings
        .set(
            "accounts.disposable_email_domains",
            json!(["throwaway.example.net"]),
            "test",
        )
        .await
        .unwrap();
    let (st, _) = create("zed.pds.example.com", "zed@throwaway.example.net").await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "operator-added domains too");
    let (st, v) = create("zed.pds.example.com", "zed@example.net").await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

impl Harness {
    /// A request with a raw body; returns status, headers and body bytes.
    async fn raw(
        &self,
        method: &str,
        path: &str,
        auth: Option<&str>,
        body: Vec<u8>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.state.hostname.clone());
        if let Some(t) = auth {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let (status, headers) = (res.status(), res.headers().clone());
        let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
        (status, headers, bytes)
    }

    async fn upload(&self, access: &str, bytes: &[u8], content_type: &str) -> Value {
        let (st, _, body) = self
            .raw(
                "POST",
                "/xrpc/com.atproto.repo.uploadBlob",
                Some(access),
                bytes.to_vec(),
                &[("content-type", content_type)],
            )
            .await;
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(st, StatusCode::OK, "{v}");
        v["blob"].clone()
    }
}

fn post_record(text: &str) -> Value {
    json!({"$type": "app.bsky.feed.post", "text": text, "createdAt": "2026-11-05T00:00:00.000Z"})
}

#[tokio::test]
async fn records_are_written_read_listed_and_deleted() {
    let h = harness(false).await;
    let acct = h
        .create_account("abe.pds.example.com", "abe@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );

    let (st, created) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("first")}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{created}");
    assert_eq!(created["validationStatus"], "valid");
    let uri = created["uri"].as_str().unwrap().to_owned();
    let rkey = uri.rsplit('/').next().unwrap().to_owned();
    assert!(proto_blue_syntax::Tid::new(&rkey).is_ok(), "a TID rkey");
    assert_eq!(h.outbox_kinds(&did).await.last().unwrap(), "commit");

    let (st, got) = h
        .call(
            "GET",
            &format!("/xrpc/com.atproto.repo.getRecord?repo={did}&collection=app.bsky.feed.post&rkey={rkey}"),
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{got}");
    assert_eq!(got["value"], post_record("first"));
    assert_eq!(got["cid"], created["cid"]);

    // Validation, $type and rkey refusals.
    for (body, why) in [
        (
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": {"text": "no date"}}),
            "invalid",
        ),
        (
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": {"$type": "app.bsky.feed.like"}}),
            "type",
        ),
        (
            json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": "self", "record": post_record("x")}),
            "rkey",
        ),
        (
            json!({"repo": did, "collection": "com.example.unknown", "record": {"a": 1}, "validate": true}),
            "unknown",
        ),
    ] {
        let (st, v) = h
            .post("com.atproto.repo.createRecord", Some(&access), body)
            .await;
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::BAD_REQUEST, Some("InvalidRequest")),
            "{why}: {v}"
        );
    }
    let (_, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "com.example.unknown", "rkey": "k1", "record": {"a": 1}}),
        )
        .await;
    assert_eq!(v["validationStatus"], "unknown");

    // putRecord: create, update, and a no-op for the same record.
    let put = |text: &'static str| {
        h.post(
            "com.atproto.repo.putRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.actor.profile", "rkey": "self", "record": {"displayName": text}}),
        )
    };
    let (st, p1) = put("Abe").await;
    assert_eq!(st, StatusCode::OK, "{p1}");
    assert!(p1["commit"].is_object());
    let (_, p2) = put("Abe").await;
    assert!(
        p2.get("commit").is_none(),
        "identical put commits nothing: {p2}"
    );
    let (_, p3) = put("Abe 2").await;
    assert!(p3["commit"].is_object());
    assert_ne!(p3["cid"], p1["cid"]);

    // A stale swapCommit is refused.
    let (st, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("x"),
                   "swapCommit": created["commit"]["cid"]}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidSwap"))
    );

    // applyWrites: several ops, one commit, typed results.
    let (st, applied) = h
        .post(
            "com.atproto.repo.applyWrites",
            Some(&access),
            json!({"repo": did, "writes": [
                {"$type": "com.atproto.repo.applyWrites#create", "collection": "app.bsky.feed.post", "rkey": "3m2aaaaaaaaaa", "value": post_record("a")},
                {"$type": "com.atproto.repo.applyWrites#create", "collection": "app.bsky.feed.post", "rkey": "3m2aaaaaaaaab", "value": post_record("b")},
                {"$type": "com.atproto.repo.applyWrites#update", "collection": "app.bsky.feed.post", "rkey": "3m2aaaaaaaaab", "value": post_record("b2")},
                {"$type": "com.atproto.repo.applyWrites#delete", "collection": "app.bsky.feed.post", "rkey": rkey},
            ]}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{applied}");
    let types: Vec<&str> = applied["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["$type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types.last(),
        Some(&"com.atproto.repo.applyWrites#deleteResult")
    );

    // listRecords: newest first, with a cursor; reverse is oldest first.
    let list = |extra: &str| {
        h.get(format!(
            "/xrpc/com.atproto.repo.listRecords?repo={did}&collection=app.bsky.feed.post{extra}"
        ))
    };
    let (_, page) = list("&limit=1").await;
    assert_eq!(page["records"][0]["value"]["text"], "b2");
    assert_eq!(page["cursor"], "3m2aaaaaaaaab");
    let (_, next) = list("&limit=1&cursor=3m2aaaaaaaaab").await;
    assert_eq!(next["records"][0]["value"]["text"], "a");
    let (_, rev) = list("&reverse=true").await;
    assert_eq!(rev["records"][0]["value"]["text"], "a");
    assert_eq!(
        rev["records"].as_array().unwrap().len(),
        2,
        "the deleted post is gone"
    );

    // deleteRecord of a missing record commits nothing.
    let (st, v) = h
        .post(
            "com.atproto.repo.deleteRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": rkey}),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert!(v.get("commit").is_none(), "{v}");

    let (_, desc) = h
        .call(
            "GET",
            &format!("/xrpc/com.atproto.repo.describeRepo?repo={did}"),
            None,
            None,
        )
        .await;
    assert_eq!(
        desc["collections"],
        json!([
            "app.bsky.actor.profile",
            "app.bsky.feed.post",
            "com.example.unknown"
        ])
    );
    assert_eq!(desc["handleIsCorrect"], true);

    // Another account can't write here; a deactivated one can't write at all.
    let other = h
        .create_account("bea.pds.example.com", "bea@example.com", "pw")
        .await;
    let (st, _) = h
        .post(
            "com.atproto.repo.createRecord",
            other["accessJwt"].as_str(),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("x")}),
        )
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    h.post(
        "com.atproto.server.deactivateAccount",
        Some(&access),
        json!({}),
    )
    .await;
    let (st, v) = h
        .post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": post_record("x")}),
        )
        .await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::UNAUTHORIZED, Some("AccountDeactivated"))
    );
}

#[tokio::test]
async fn create_record_replaces_duplicate_follows_and_likes() {
    let h = harness(false).await;
    let acct = h
        .create_account("cal.pds.example.com", "cal@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let follow = json!({"$type": "app.bsky.graph.follow", "subject": "did:plc:abcdefghijklmnopqrstuvwx", "createdAt": "2026-11-05T00:00:00.000Z"});
    let make = |record: Value| {
        h.post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.graph.follow", "record": record}),
        )
    };
    let (_, first) = make(follow.clone()).await;
    let (st, second) = make(follow.clone()).await;
    assert_eq!(st, StatusCode::OK, "{second}");
    let (_, list) = h
        .call(
            "GET",
            &format!(
                "/xrpc/com.atproto.repo.listRecords?repo={did}&collection=app.bsky.graph.follow"
            ),
            None,
            None,
        )
        .await;
    let uris: Vec<&str> = list["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        vec![second["uri"].as_str().unwrap()],
        "the first follow went in the same commit"
    );
    assert_ne!(first["uri"], second["uri"]);
}

#[tokio::test]
async fn blobs_upload_get_referenced_promoted_served_and_collected() {
    let h = harness(false).await;
    let acct = h
        .create_account("dee.pds.example.com", "dee@example.com", "pw")
        .await;
    let (did, access) = (
        acct["did"].as_str().unwrap().to_owned(),
        acct["accessJwt"].as_str().unwrap().to_owned(),
    );
    let mut png = vec![
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R',
    ];
    png.extend(std::iter::repeat_n(7u8, 1000));
    let blob = h.upload(&access, &png, "application/octet-stream").await;
    assert_eq!(blob["mimeType"], "image/png", "sniffed type wins");
    assert_eq!(blob["size"], png.len());
    let cid = blob["ref"]["$link"].as_str().unwrap().to_owned();
    assert_eq!(
        cid,
        proto_blue_lex_data::Cid::for_raw(&png).to_string_base32()
    );
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    assert_eq!(row.state, "temp");

    let post_with = |b: Value| {
        let mut p = post_record("look");
        p["embed"] =
            json!({"$type": "app.bsky.embed.images", "images": [{"image": b, "alt": "a picture"}]});
        p
    };
    let create = |record: Value| {
        h.post(
            "com.atproto.repo.createRecord",
            Some(&access),
            json!({"repo": did, "collection": "app.bsky.feed.post", "record": record}),
        )
    };
    // References must match the stored blob.
    let mut wrong_mime = blob.clone();
    wrong_mime["mimeType"] = json!("image/jpeg");
    let (st, v) = create(post_with(wrong_mime)).await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidMimeType")),
        "{v}"
    );
    let mut wrong_size = blob.clone();
    wrong_size["size"] = json!(1);
    let (_, v) = create(post_with(wrong_size)).await;
    assert_eq!(v["error"], "InvalidSize");
    let mut missing = blob.clone();
    missing["ref"]["$link"] =
        json!(proto_blue_lex_data::Cid::for_raw(b"never uploaded").to_string_base32());
    let (_, v) = create(post_with(missing)).await;
    assert_eq!(v["error"], "BlobNotFound");

    let (st, post) = create(post_with(blob.clone())).await;
    assert_eq!(st, StatusCode::OK, "{post}");
    let row = h.state.store.blob(&did, &cid).await.unwrap().unwrap();
    assert_eq!(row.state, "permanent");
    assert!(
        row.storage_key.starts_with(&format!("blobs/{did}/{cid}.")),
        "moved: {}",
        row.storage_key
    );
    assert!(row.pending_move.is_none());

    // Served with the reference's headers, and in ranges.
    let path = format!("/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}");
    let (st, headers, body) = h.raw("GET", &path, None, Vec::new(), &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, png);
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["content-security-policy"],
        "default-src 'none'; sandbox"
    );
    let (st, headers, body) = h
        .raw(
            "GET",
            &format!("/blob/{did}/{cid}"),
            None,
            Vec::new(),
            &[("range", "bytes=0-3")],
        )
        .await;
    assert_eq!(st, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, &png[..4]);
    assert_eq!(headers["content-range"], format!("bytes 0-3/{}", png.len()));

    // Dropping the last reference starts the grace period; GC then removes it.
    let rkey = post["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    h.post(
        "com.atproto.repo.deleteRecord",
        Some(&access),
        json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": rkey}),
    )
    .await;
    assert!(
        h.state
            .store
            .blob(&did, &cid)
            .await
            .unwrap()
            .unwrap()
            .unreferenced_since
            .is_some()
    );
    let mut tx = h.state.store.begin().await.unwrap();
    assert_eq!(tx.gc_blob_batch(3_600_000, 0, 100, 1).await.unwrap(), 1);
    tx.commit().await.unwrap();
    let (st, _, _) = h.raw("GET", &path, None, Vec::new(), &[]).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "gone");
    // Its bytes go after the delete grace.
    assert!(
        locus_blob::run_deletes(&h.state.store, &h.state.blobs, 0, 100)
            .await
            .unwrap()
            >= 1
    );
    assert!(
        h.state
            .blobs
            .open(&row.storage_key)
            .await
            .unwrap()
            .is_none()
    );

    // A temp blob nobody references is collected after its TTL.
    let other = h.upload(&access, b"just some text", "text/plain").await;
    let other_cid = other["ref"]["$link"].as_str().unwrap();
    let mut tx = h.state.store.begin().await.unwrap();
    assert_eq!(
        tx.gc_blob_batch(3_600_000, 3_600_000, 100, 1)
            .await
            .unwrap(),
        0,
        "young temp blob kept"
    );
    assert_eq!(tx.gc_blob_batch(-1, 3_600_000, 100, 1).await.unwrap(), 1);
    tx.commit().await.unwrap();
    assert!(h.state.store.blob(&did, other_cid).await.unwrap().is_none());
}

impl Harness {
    async fn get(&self, path: String) -> (StatusCode, Value) {
        self.call("GET", &path, None, None).await
    }
}
